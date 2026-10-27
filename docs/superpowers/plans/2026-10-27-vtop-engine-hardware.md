# vtop Engine · Hardware 탭 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 탭 2(Engine)에 스케줄러·캐시·지연 히스토그램을, 탭 4(Hardware)에 GPU별 행·코어별 CPU·팬·디스크/넷·프로세스 표를 그린다. 수집은 이미 대부분 있고, 없는 것(히스토그램 버킷, 코어별 CPU)만 스냅샷에 더한다.

**Architecture:** 스냅샷에 `Vllm.ttft_buckets`/`e2e_buckets`(버킷별 개수, 누적 아님)와 `System.cpu_cores`를 추가한다. `History`에 waiting·disk·net 키를 더 넣는다. 화면은 `src/ui/engine.rs`, `src/ui/hardware.rs` 두 파일이고 `overview::draw`가 탭 번호로 분기한다. 공통 가로 막대는 `format::bar`로 옮겨 세 화면이 쓴다.

**Tech Stack:** ratatui 0.30(Sparkline, Paragraph), sysinfo 0.39(`cpus()`).

**Spec:** `docs/superpowers/specs/2026-10-27-vtop-design.md` — "Engine", "Hardware" 절.

## Global Constraints

- 상태색 3개(녹/황/적)와 밝기 2단계만. 유니코드 블록 문자, `--ascii`면 `#`/`.`.
- 값이 없으면 `n/a`(Linux 전용·macOS 전용 값 포함). 패닉 없음: 작은 터미널에서도 그리다 말지 죽지 않는다.
- UI 문자열 영어. 커밋 끝 `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

## Review Focus

1. 히스토그램 버킷이 없거나(서버 막 시작, count 전부 0) `+Inf`만 있을 때 빈 영역에 `no samples yet`을 그리고 나누기 0이 없어야 한다. → Task 1 `empty_histogram_gives_no_rows`, Task 2 `engine_renders_without_samples`.
2. 코어가 0개(테스트·컨테이너)이거나 64개 이상이어도 코어 격자가 영역 안에서 잘린다. → Task 3 `hardware_fits_many_cores_in_small_area`.
3. GPU가 0개면 GPU 표 대신 `no GPU` 한 줄. → Task 3 `hardware_without_gpu`.
4. 프로세스 표의 긴 이름은 열 폭에서 잘려 다음 열을 밀지 않는다. → Task 3 `process_names_are_truncated`.
5. 20×5 같은 극소 터미널에서 두 탭 모두 패닉 없음. → Task 2·3 `tiny_terminal_does_not_panic`.

---

### Task 1: 수집 보강 — 히스토그램 버킷, 코어별 CPU, 히스토리 키

**Files:**
- Modify: `src/snapshot.rs`, `src/collectors/vllm.rs`, `src/collectors/system.rs`, `src/ui/app.rs`(`ingest`)

**Interfaces:**
- `Vllm { pub ttft_buckets: Vec<(f64, u64)>, pub e2e_buckets: Vec<(f64, u64)>, .. }` — `(le, count_in_this_bucket)`, `le` 오름차순, 누적 아님, 라벨 세트 합산. `+Inf`는 마지막 원소(`f64::INFINITY`).
- `Metrics::buckets(&self, name) -> Vec<(f64, u64)>` — `histogram_quantile`이 쓰는 병합 로직을 꺼내 공유; 누적 → 차분.
- `System { pub cpu_cores: Vec<f64>, .. }` — 코어별 % (sysinfo `cpus()`); 첫 샘플은 빈 Vec.
- `App::ingest`가 history 키 `waiting`, `disk_r`, `disk_w`, `net_rx`, `net_tx`를 추가로 push.

- [ ] **Step 1: 실패하는 테스트** (`src/collectors/vllm.rs`)
  - `buckets_are_per_bucket_counts`: V1 픽스처의 TTFT 버킷 → 길이 = 버킷 수, 각 count = 누적 차분, 마지막 le가 `INFINITY`, 합계 = `_count`.
  - `empty_histogram_gives_no_rows`: `parse("vllm:x_bucket{le=\"+Inf\"} 0\n")` → `buckets("vllm:x")`가 `[(inf, 0)]`, `to_vllm` 후 `ttft_buckets`는 비어 있음(합계 0이면 빈 Vec).
  - (`system.rs`) `cpu_cores_present_after_second_refresh`: `Sys::new()` + refresh 2회 → `cores(&sys).len() == sys.cpus().len()`.
- [ ] **Step 2~4: RED → 구현 → `cargo test` GREEN**
- [ ] **Step 5: 커밋** — `feat: histogram buckets and per-core cpu in the snapshot`

---

### Task 2: Engine 탭

**Files:**
- Create: `src/ui/engine.rs`
- Modify: `src/ui/overview.rs`(탭 1 분기, `bar`를 `format.rs`로 이동), `src/ui/format.rs`(`pub fn bar(ratio, width, ascii)`), `src/ui/mod.rs`

**Interfaces:**
- `engine::draw(f, app, cfg, area)`.
- 레이아웃: 위 1/3 — 카드 3개 `Scheduler`(`N run · N wait`, 부제 `N preemptions`, 스파크라인 running), `KV cache`(`%`, 스파크라인 kv), `Prefix cache`(`hit %`, 부제 `n/a` 허용). 카드는 overview의 `card()`를 `pub(super)`로 열어 재사용. 아래 2/3 — 좌 `TTFT`, 우 `E2E latency`: 제목줄 `TTFT  p50 420 ms · p99 1,980 ms · since start`, 그 아래 버킷 행 `  0.25–0.5 s  ████████░░  1,204` (라벨 11자, 막대는 남는 폭, 개수 우측). count가 0인 행은 숨기되 처음 보이는 행부터 끝까지는 0도 표시. 행이 영역보다 많으면 count가 가장 큰 행을 포함하도록 앞뒤를 잘라낸다(단순히 앞에서부터 `height`개).
- 버킷이 모두 비면 `no samples yet` 한 줄.

- [ ] **Step 1: 실패하는 테스트** (`engine.rs` tests, TestBackend 120×40)
  - `engine_renders_scheduler_and_histograms`: Vllm{running 3, waiting 1, preemptions 2, kv 0.61, ttft_buckets [(0.25,0),(0.5,10),(1.0,30),(inf,2)], e2e_buckets [(1,5),(inf,0)]} → 출력에 `3 run · 1 wait`, `2 preemptions`, `61 %`, `0.25–0.5 s`, `30`, `TTFT`, `E2E`.
  - `engine_renders_without_samples`: 빈 버킷 → `no samples yet` 2회.
  - `tiny_terminal_does_not_panic`: 20×5 렌더.
- [ ] **Step 2~4: RED → 구현 → GREEN**
- [ ] **Step 5: 커밋** — `feat: engine tab with scheduler, caches and latency histograms`

---

### Task 3: Hardware 탭

**Files:**
- Create: `src/ui/hardware.rs`
- Modify: `src/ui/overview.rs`(탭 3 분기), `src/ui/mod.rs`, `src/collectors/system.rs`(프로세스 표에 `threads` 대신 이름만; 변경 없으면 생략)

**Interfaces:**
- `hardware::draw(f, app, cfg, area)`.
- 레이아웃(세로 순): ① GPU 표 — 헤더 `GPU                      UTIL     MEM               POWER    TEMP   CLOCK`, GPU당 1행(이름 24자 절단, `96 %`, `10.4 / 24.0 GB`, `18.2 W`(limit 있으면 `/ 300 W`), `59 °C`, `1,980 MHz`), 없으면 `no GPU`. ② CPU — `CPU  62 %  ·  cpu 3.1 W  ·  ane 0.0 W`(없는 값 생략) 다음 줄부터 코어 막대 격자: 코어당 `c0 ████░░ 62`(폭 12), 한 줄에 `width / 14`개, 행 수는 영역에 맞게 잘림. ③ 한 줄 요약 `RAM 15.9 / 24.0 GB · swap 0 B · fans Fan1 1,200 rpm`. ④ I/O — 좌 `Disk  R 12.3 MB/s  W 0.0 B/s` + 2행 스파크라인(disk_r), 우 `Net  ↓ 1.2 MB/s  ↑ 80 KB/s` + 스파크라인(net_rx). ⑤ 프로세스 표 — `PID     NAME                  CPU %     RSS    GPU MEM`, `top_processes` 최대 5행, 이름 20자 절단, GPU MEM은 `None`이면 `n/a`.
- 각 섹션은 `Constraint::Length`로 고정 높이(GPU: 행수+1, CPU: 1 + 코어 행수(최대 4), 요약 1, I/O 3, 프로세스 6) 순서대로 넣고 남는 공간은 `Min(0)`; 영역이 모자라면 아래 섹션부터 사라진다(ratatui가 높이 0을 주면 그리지 않음).

- [ ] **Step 1: 실패하는 테스트** (`hardware.rs` tests)
  - `hardware_renders_gpu_rows_cores_and_processes`: GPU 2개(이름 "A100", util 96/40, mem 10.4/40 GB, power 250/limit 300, temp 59, clock 1980), cpu_cores 8개(그중 하나 62.0), top_processes 2개 → 출력에 `A100` 2회, `96 %`, `250.0 / 300 W`, `1,980 MHz`, `c3`, `62`, 프로세스 이름, `n/a`(gpu_mem None).
  - `hardware_without_gpu`: gpu 빈 Vec → `no GPU`.
  - `hardware_fits_many_cores_in_small_area`: 코어 96개, 100×12 → 패닉 없음, 출력 줄 수 = 12.
  - `process_names_are_truncated`: 이름 40자 → 출력에 처음 20자는 있고 전체는 없음.
  - `tiny_terminal_does_not_panic`: 20×5.
- [ ] **Step 2~4: RED → 구현 → GREEN**
- [ ] **Step 5: 커밋** — `feat: hardware tab with per-gpu rows, cores, io and process table`

---

### Task 4: 마무리 — 힌트·README·라이브 확인

- [ ] `overview.rs`의 "not implemented yet" 분기 삭제, README "Next version" 문장 삭제, Screens 절에 Engine·Hardware 한 줄씩, Keys 표 `1`–`4` 설명에서 "(tabs 2 and 4: next version)" 제거.
- [ ] 라이브: OCR 서버에 요청 2건 보낸 뒤 `2`·`4` 탭 캡처(TTFT 히스토그램에 행이 생기는지, GPU 행·코어 격자·프로세스 표).
- [ ] `cargo install --path .`로 로컬 바이너리 갱신. 커밋 — `docs: engine and hardware tabs`

## 다음 계획(범위 밖)
GitHub 푸시·첫 릴리스 태그, `vtop setup` 도우미.
