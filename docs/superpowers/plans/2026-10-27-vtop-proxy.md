# vtop 프록시 + Requests 탭 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `vtop --proxy 127.0.0.1:8112`로 OpenAI 호환 통과 프록시를 열어 요청별 TTFT·토큰·tok/s를 기록하고, Overview 하단 표와 Requests 탭(3)에서 본다.

**Architecture:** 프록시는 전용 std 스레드 위의 tokio 런타임에서 axum으로 돌고, 요청·응답 바디를 바꾸지 않고 reqwest로 `--url`에 전달한다. 기록은 `Arc<Mutex<RequestLog>>`(1,000건 링)에 쌓이고 UI가 매 틱 최근분만 읽는다(스냅샷엔 넣지 않아 클론 비용 없음). 스트리밍 응답은 바이트를 그대로 흘리면서 SSE `data:` 줄만 엿봐 `usage`와 청크 수를 센다.

**Tech Stack:** axum 0.8, tokio, reqwest 0.13(async, stream), serde_json, ratatui 0.30.

**Spec:** `docs/superpowers/specs/2026-10-27-vtop-design.md` — "프록시", "Requests", "단축키" 절.

## Global Constraints

- 프록시는 상태 코드·헤더·바디·SSE 형식을 바꾸지 않는다. vtop 자체 응답은 업스트림 불통 시 502 한 가지뿐.
- 토큰 수는 `usage`를 우선, 없으면 SSE 청크 수로 출력 토큰을 근사하고 `approx = true`(표에 `~` 접두). 프롬프트 토큰은 그때 `None`.
- 기록은 최근 1,000건 링버퍼. 프록시가 꺼져 있으면 Overview 표 자리에 `start with --proxy to see per-request rows` 한 줄(현행 유지).
- 수집기·프록시는 패닉하지 않는다(`panic = "abort"`가 걸려 있으므로 `unwrap`은 잠금에만).
- UI 문자열은 영어. 커밋 끝에 `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

## Review Focus

1. 업스트림이 죽어 있을 때(연결 거부·타임아웃) 클라이언트는 502와 본문 한 줄을 받고, 기록엔 `error` 행이 남아야 한다. → Task 3 `upstream_down_gives_502_and_error_row`.
2. SSE 청크가 바이트 경계에서 잘려 와도(`da` + `ta: {...}\n`) 줄 단위 파서가 토큰을 정확히 세야 한다. → Task 2 `sse_lines_split_across_chunks`.
3. `stream_options.include_usage` 없이 스트리밍하면 `usage`가 없다 — 출력 토큰은 청크 수 근사에 `~`, 프롬프트는 `n/a`. → Task 2 `stream_without_usage_is_approximate`.
4. 이미지가 든 큰 요청 바디(수백 KB base64)도 그대로 전달돼야 한다(paddlex OCR 경로). → Task 3 `large_body_passes_through_unchanged`.
5. 필터 입력 중에는 `q`가 종료가 아니라 글자여야 하고 `Esc`가 필터를 닫아야 한다. → Task 6 `typing_in_filter_does_not_quit`.

---

### Task 1: 기록 구조와 로그 링버퍼

**Files:**
- Create: `src/requests.rs`
- Modify: `src/main.rs` (`mod requests;`), `Cargo.toml` (`serde` 이미 있음; `chrono` 추가 안 함 — 시각은 `std::time::SystemTime` 초로 저장하고 표시 때 `%H:%M:%S`를 직접 계산)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Debug, Clone, Serialize)] pub enum Status { InFlight, Done(u16), Error(String) }
  #[derive(Debug, Clone, Serialize)] pub struct Record {
      pub id: u64, pub started_unix_ms: u64, pub path: String, pub model: Option<String>, pub stream: bool,
      pub status: Status, pub prompt_tokens: Option<u64>, pub output_tokens: Option<u64>, pub approx: bool,
      pub ttft_ms: Option<f64>, pub duration_ms: Option<f64>,
  }
  impl Record { pub fn tok_s(&self) -> Option<f64> }  // output / ((duration - ttft) s); 분모 ≤ 0이면 None
  #[derive(Default)] pub struct RequestLog { .. }    // VecDeque<Record>, 용량 1000
  impl RequestLog {
      pub fn push(&mut self, r: Record) -> u64;       // id 부여(단조 증가), 가득 차면 가장 오래된 것 제거
      pub fn update(&mut self, id: u64, f: impl FnOnce(&mut Record));
      pub fn latest(&self, n: usize) -> Vec<Record>;  // 최신 먼저
      pub fn clear(&mut self);
      pub fn len(&self) -> usize;
  }
  pub type SharedLog = Arc<Mutex<RequestLog>>;
  pub fn hhmmss_utc(unix_ms: u64) -> String;          // "15:33:51". shortcut: UTC (std has no local TZ); 열 제목에 (UTC) 표기, 로컬 시각이 필요해지면 chrono 추가
  ```

- [ ] **Step 1: 실패하는 테스트**
  - `log_keeps_last_1000`: 1,100건 push 후 `len() == 1000`, `latest(1)[0].id == 1100`.
  - `update_by_id`: push 두 건, 첫 건 `update`로 `status = Done(200)` → `latest(2)[1].status`가 Done(200).
  - `tok_s_uses_decode_time`: output 300, ttft 1000ms, duration 4000ms → `Some(100.0)`; duration ≤ ttft → None.
- [ ] **Step 2: `cargo test requests` 실패 확인**
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: 커밋** — `feat: request record and ring log`

---

### Task 2: SSE 엿보기 파서

**Files:**
- Create: `src/proxy/sse.rs`, `src/proxy/mod.rs` (`pub mod sse;`)
- Modify: `src/main.rs` (`mod proxy;`)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Default)] pub struct SsePeek { buf: Vec<u8>, pub chunks: u64, pub usage: Option<(u64, u64)> /* (prompt, completion) */ }
  impl SsePeek {
      pub fn feed(&mut self, bytes: &[u8]);   // 완성된 줄만 처리, 나머지는 buf에 보관. `data: ` 접두 줄 중 `[DONE]` 제외.
                                              // JSON 파싱 성공 시: `usage`가 객체면 prompt_tokens/completion_tokens 저장;
                                              // `choices[0].delta.content`가 비어 있지 않은 문자열이면 chunks += 1.
      pub fn finish(&mut self);               // buf에 남은 마지막 줄 처리
  }
  pub fn parse_usage(body: &[u8]) -> Option<(u64, u64)>;  // 비스트리밍 응답 JSON의 usage
  ```

- [ ] **Step 1: 실패하는 테스트**
  - `sse_lines_split_across_chunks`: `b"da"`, `b"ta: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\nda"`, `b"ta: [DONE]\n"` → feed 3번 + finish → `chunks == 1`.
  - `stream_without_usage_is_approximate`: content 청크 3개, usage 없음 → `chunks == 3`, `usage == None`.
  - `usage_chunk_wins`: 청크 2개 뒤 `data: {"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":7}}` → `usage == Some((20,7))`.
  - `parse_usage_non_stream`: `{"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}}` → `Some((20,2))`; `{}` → None.
- [ ] **Step 2: 실패 확인** → **Step 3: 구현** → **Step 4: `cargo test sse` 통과**
- [ ] **Step 5: 커밋** — `feat: sse peek parser for streamed usage and chunk counts`

---

### Task 3: 통과 프록시 서버

**Files:**
- Create: `src/proxy/server.rs`
- Modify: `src/proxy/mod.rs`, `src/main.rs`, `Cargo.toml` (`futures-util = "0.3"` 추가: `StreamExt::map`용; `tokio` features에 `net` 추가; `axum` 기본 피처 유지)

**Interfaces:**
- Produces: `pub fn spawn(bind: String, upstream: String, log: SharedLog) -> std::thread::JoinHandle<()>` — 스레드 안에서 `tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all()` 런타임을 만들고 `axum::serve(TcpListener::bind(bind), router)`. 바인드 실패는 `eprintln!` 후 `std::process::exit(2)`(TUI가 뜨기 전이므로 허용).
- Produces(테스트용): `pub fn router(upstream: String, log: SharedLog) -> axum::Router` — `fallback(any(handler))`로 모든 경로·메서드.
- 핸들러 동작:
  1. 요청 바디 전부 읽음(`axum::body::to_bytes`, 상한 64 MiB). JSON이면 `model`, `stream`(bool) 추출.
  2. `log.push(Record { started_unix_ms, path, model, stream, status: InFlight, .. })` → id.
  3. reqwest `Client`(프로세스당 하나, `OnceLock`)로 `{upstream}{path}{?query}`에 같은 메서드·헤더(`host`, `content-length`, `transfer-encoding`, `connection` 제외)·바디 전송.
  4. 실패(연결 거부·타임아웃 60 s) → `log.update(id, status = Error(e.to_string()), duration)` 후 `502` + 본문 `vtop: upstream unreachable: <e>`.
  5. 성공: 상태·헤더(`transfer-encoding`, `connection` 제외) 복사. `first_byte`는 첫 바디 청크 시각.
     - `stream == true`: `Body::from_stream(bytes_stream().map(...))` — 각 청크를 `SsePeek::feed`, 첫 청크에 `ttft_ms`, 스트림 종료 시 `finish` 후 `log.update(id, Done(status), output_tokens = usage.1 or chunks, prompt_tokens = usage.0, approx = usage.is_none(), duration_ms)`.
     - 아니면: 바디 전부 받은 뒤 `parse_usage`로 토큰, `ttft_ms = duration_ms`, 같은 바이트를 그대로 응답.
- `main.rs`: `--proxy` 있으면 `proxy::spawn(addr, cfg.url.clone(), log.clone())`; `ui::run(state, cfg, log)`.

- [ ] **Step 1: 실패하는 테스트** (`#[tokio::test]`, 가짜 업스트림도 axum으로 `127.0.0.1:0`에 띄움; 클라이언트는 reqwest)
  - `stream_passes_through_and_records_usage`: 업스트림이 `text/event-stream`으로 content 청크 3개 + usage 청크 + `[DONE]`을 순서대로 보냄 → 클라이언트가 받은 바이트가 업스트림이 보낸 바이트와 동일, 기록 `Done(200)`, `output_tokens == Some(7)`, `prompt_tokens == Some(20)`, `approx == false`, `ttft_ms.is_some()`.
  - `non_stream_records_usage_and_body_unchanged`: JSON 응답 그대로, `prompt_tokens == Some(20)`, `output_tokens == Some(2)`.
  - `upstream_down_gives_502_and_error_row`: upstream을 닫힌 포트로 → 502, 본문에 `upstream unreachable`, 기록 `Error(_)`.
  - `large_body_passes_through_unchanged`: 300 KB 바디 POST → 업스트림이 받은 길이 == 300 KB(업스트림 핸들러가 길이를 응답으로 돌려줌).
- [ ] **Step 2: 실패 확인** → **Step 3: 구현** → **Step 4: `cargo test proxy` 통과** (+ 전체 suite)
- [ ] **Step 5: 수동 확인** — `vtop --url http://127.0.0.1:8111 --proxy 127.0.0.1:8112` 띄우고 `curl 127.0.0.1:8112/v1/models`가 vLLM 응답과 같은지.
- [ ] **Step 6: 커밋** — `feat: openai-compatible pass-through proxy recording per-request stats`

---

### Task 4: Overview 하단 요청 표

**Files:**
- Modify: `src/ui/overview.rs`, `src/ui/app.rs`, `src/ui/mod.rs`
- Create: `src/ui/table.rs`

**Interfaces:**
- `App`에 `pub requests: Vec<Record>`(최신 먼저, `ingest_log(&mut self, log: &SharedLog)`가 `latest(1000)`로 채움 — 틱당 한 번, 1,000건 클론은 수 µs), `pub proxy_on: bool`.
- Produces: `table::request_rows(records: &[Record], width: u16, max_rows: usize) -> Vec<Line>` — 헤더 `TIME(UTC)  STATUS  MODEL  PROMPT  OUTPUT  TTFT  TOK/S  TIME` + 행. 상태색: Done 2xx 녹, 4xx/5xx·Error 적, InFlight 황. `approx`면 OUTPUT 앞에 `~`. 폭 < 100이면 MODEL 열 생략.
- Overview: `proxy_on`이면 요청 영역에 `request_rows(.., area.height - 1)`; 아니면 현행 안내 줄.

- [ ] **Step 1: 실패하는 테스트** — `overview_shows_recent_requests_when_proxy_on`: Record 2건(Done 200, output 261/ttft 420/duration 8700 → tok/s 31.5; InFlight) → 버퍼에 `"261"`, `"31.5"`, `"in flight"` 포함, 안내 줄 없음. `approx_marks_output_with_tilde`.
- [ ] **Step 2~4: RED → 구현 → `cargo test overview` GREEN**
- [ ] **Step 5: 커밋** — `feat: recent requests table on overview`

---

### Task 5: Requests 탭 (목록·선택·상세)

**Files:**
- Create: `src/ui/requests_tab.rs`
- Modify: `src/ui/overview.rs` (`draw`에서 `app.tab == 2`면 `requests_tab::draw`), `src/ui/app.rs`

**Interfaces:**
- `App`에 `pub selected: usize`(필터 적용 후 인덱스), `pub filter: String`, `pub filter_editing: bool`.
- Produces: `requests_tab::draw(f, app, area)` — 상단 1행 `N requests · filter: <text>`(필터 편집 중이면 커서 `▏` 표시), 중간 표(`table::request_rows` 재사용, 선택 행 반전), 하단 4행 상세(`id, path, model, status 전문, prompt/output/approx, ttft, duration, tok/s`).
- Produces: `App::visible_requests(&self) -> Vec<&Record>` — 필터 규칙: 공백으로 나눈 토큰 전부 만족. 토큰이 `>N`이면 `output_tokens >= N`, 아니면 model/status 문자열에 대소문자 무시 부분 일치.
- 키(탭 3에서만): `j`/`k` 선택 이동(경계 clamp), `c` `log.clear()` + `requests.clear()`, `/` `filter_editing = true`.

- [ ] **Step 1: 실패하는 테스트** — `filter_tokens_all_match`: 기록 3건(model "a", "b"; output 10, 500) → 필터 `"a >100"` → 0건, `">100"` → 1건, `"error"` → Error 행만. `j_k_clamp_selection`. `requests_tab_renders_detail_of_selected`.
- [ ] **Step 2~4: RED → 구현 → GREEN**
- [ ] **Step 5: 커밋** — `feat: requests tab with selection, detail and filter`

---

### Task 6: 필터 입력 모드와 `e` 내보내기

**Files:**
- Modify: `src/ui/app.rs`, `src/snapshot.rs`(`Serialize` 파생, `sampled_at`은 `#[serde(skip)]`), `src/ui/mod.rs`

**Interfaces:**
- `on_key`: `filter_editing`이면 문자는 `filter`에 추가, `Backspace` 삭제, `Enter`/`Esc`는 편집 종료(Esc는 필터 비움), 그 외 키 무시 — 종료 키 포함.
- `e`: `vtop-<YYYYmmdd-HHMMSS>.json`(UTC)에 `{"snapshot": .., "requests": [..]}` 저장. 결과 메시지는 `App.notice: Option<(String, Instant)>`에 넣고 헤더 오른쪽에 5초 표시(저장 실패도 같은 자리).

- [ ] **Step 1: 실패하는 테스트** — `typing_in_filter_does_not_quit`: `/` → `q` → `on_key`가 false이고 `filter == "q"`; `Esc` → 편집 종료·필터 비움. `export_writes_json`: 임시 디렉터리로 `cwd` 대신 `App::export_to(&self, dir)`를 호출해 파일이 생기고 `requests` 배열 길이가 맞음.
- [ ] **Step 2~4: RED → 구현 → GREEN**
- [ ] **Step 5: 커밋** — `feat: filter input mode and json export`

---

### Task 7: README·힌트 갱신, 라이브 확인

- [ ] README의 "Next version" 문단을 실제 사용법으로 교체(`--proxy`, paddlex 예: `VL_REC_SERVER_URL=http://localhost:8112/v1`), 힌트 줄에 `c clear  / filter  j/k select  e export` 추가.
- [ ] 라이브: 플레이그라운드 OCR 요청을 프록시로 보내 Overview 표와 Requests 탭을 캡처해 확인.
- [ ] 커밋 — `docs: proxy usage`

## 다음 계획(범위 밖)
Engine 탭, Hardware 탭(프로세스 표·GPU별 행), 유휴 CPU 1% 목표(현재 ~3%), GitHub Releases.
