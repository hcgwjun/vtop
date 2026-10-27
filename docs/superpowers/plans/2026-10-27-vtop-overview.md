# vtop Overview 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `vtop --url …` 하나로 맥/리눅스에서 vLLM 수치와 하드웨어 수치를 Overview 화면 한 장에 보여주고, 단축키로 수집 주기·시간창을 바꿀 수 있게 한다.

**Architecture:** 수집기 셋(vllm, system, hw)이 각자 std 스레드에서 `interval`마다 `Arc<RwLock<Snapshot>>`의 자기 영역을 덮어쓴다. UI 스레드는 스냅샷을 읽어 링버퍼 히스토리에 쌓고 ratatui로 그린다. 프록시·Engine·Requests·Hardware 탭은 이 계획 범위 밖(다음 계획).

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm, sysinfo 0.39, reqwest 0.13(blocking), macmon 0.10(macOS), nvml-wrapper 0.13(Linux), clap 4.

**Spec:** `docs/superpowers/specs/2026-10-27-vtop-design.md`

## Global Constraints

- 모든 수치는 `Option`, 없는 값은 화면에 `n/a`. 두 OS에서 레이아웃이 같다.
- 수집기는 절대 패닉하지 않는다. 실패는 `None` + `Snapshot.errors`에 한 줄.
- 수집 주기 단계: 0.5 / 1 / 2 / 5 / 10 s. 시간창: 1 / 5 / 30 분. 히스토리 링버퍼 3,600칸 `f32`.
- 색은 정상(녹)·주의(황)·경고(적) 3개만. 주의: 사용률 80%↑, 온도 80°C↑, 대기 1↑. 경고: 95%↑, 90°C↑, DISCONNECTED.
- 최소 터미널 80×24. `--ascii`면 블록 문자 대신 `#`.
- 외부 바이너리 의존 없음(단, 맥 GPU 메모리용 `ioreg`는 OS 기본 포함이라 허용).
- 커밋 메시지 끝에 `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

## Review Focus

1. `/metrics`에 `NaN`/`+Inf` 값이나 모르는 줄이 섞여 있어도 파서가 나머지를 정상 처리해야 한다. → Task 2 테스트 `parse_skips_unknown_and_nan`.
2. vLLM 재시작으로 카운터가 줄면 tok/s가 음수나 거대값이 아니라 `None`으로 한 번 비었다가 다시 채워져야 한다. → Task 2 `rate_resets_on_counter_decrease`.
3. 터미널이 80×24보다 작으면 패닉 없이 안내 한 줄만 그려야 한다. → Task 7 `too_small_terminal_shows_hint`.
4. `ioreg` 출력에 기대한 키가 없으면(다른 세대 칩) GPU 메모리만 `None`이고 나머지 GPU 값은 살아 있어야 한다. → Task 5 `ioreg_missing_keys_gives_none`.
5. `--url`에 scheme이 없거나 끝에 `/`가 있어도 `/metrics` 요청 URL이 올바라야 한다. → Task 3 `metrics_url_normalizes`.

---

### Task 1: 스냅샷 스키마와 히스토리 링버퍼

**Files:**
- Modify: `src/snapshot.rs`
- Create: `src/history.rs`
- Modify: `src/main.rs` (`mod history;`)

**Interfaces:**
- Produces: 스펙 "스냅샷 스키마" 절의 구조체 전부(`Snapshot`, `Vllm`, `Gpu`, `System`, `Process`), 모두 `#[derive(Debug, Default, Clone)]`. `Snapshot.sampled_at: Option<Instant>`.
- Produces: `History::new() -> History`, `History::push(&mut self, key: &'static str, v: Option<f32>)`, `History::window(&self, key: &str, n: usize) -> Vec<Option<f32>>`(최근 n개, 오래된 것부터), `History::clear(&mut self)`. 키당 3,600칸 고정 `VecDeque<Option<f32>>`.

- [ ] **Step 1: 실패하는 테스트 작성** (`src/history.rs` 하단 `mod tests`)

```rust
#[test]
fn window_returns_latest_n_oldest_first() {
    let mut h = History::new();
    for v in [1.0, 2.0, 3.0] { h.push("x", Some(v)); }
    assert_eq!(h.window("x", 2), vec![Some(2.0), Some(3.0)]);
    assert_eq!(h.window("missing", 2), vec![None, None]);
}
#[test]
fn capacity_is_3600() {
    let mut h = History::new();
    for i in 0..4000 { h.push("x", Some(i as f32)); }
    assert_eq!(h.window("x", 3600)[0], Some(400.0));
}
```

- [ ] **Step 2: 실패 확인** — `cargo test history` → 컴파일 오류
- [ ] **Step 3: 구현** — `snapshot.rs`에 스펙 필드 추가, `history.rs` 작성. `window`는 부족한 앞부분을 `None`으로 채운다.
- [ ] **Step 4: 통과 확인** — `cargo test history` → 2 passed
- [ ] **Step 5: 커밋** — `git add -A && git commit -m "feat: snapshot schema and history ring buffer"`

---

### Task 2: Prometheus 텍스트 파서와 vLLM 매핑

**Files:**
- Create: `src/collectors/vllm.rs`, `tests/fixtures/metrics_v1.txt`, `tests/fixtures/metrics_v0.txt`
- Modify: `src/collectors/mod.rs` (`pub mod vllm;`)

**Interfaces:**
- Produces: `pub fn parse(text: &str) -> Metrics` where `Metrics(HashMap<String, Vec<(Labels, f64)>>)`, `Labels = Vec<(String, String)>`. `Metrics::get(&self, name: &str) -> Option<f64>`(첫 샘플), `Metrics::first_of(&self, names: &[&str]) -> Option<f64>`, `Metrics::label(&self, name, key) -> Option<String>`, `Metrics::histogram_quantile(&self, name: &str, q: f64) -> Option<f64>`(`_bucket` 샘플의 `le`로 선형 근사).
- Produces: `pub struct Rate { last: Option<(Instant, f64)> }`, `Rate::update(&mut self, now: Instant, counter: Option<f64>) -> Option<f64>` — 카운터가 줄거나 첫 호출이면 `None`.
- Produces: `pub fn to_vllm(m: &Metrics, gen: &mut Rate, prompt: &mut Rate, now: Instant) -> Vllm` — 스펙 "vllm" 절의 메트릭 이름 매핑. KV 사용률은 `first_of(&["vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc"])`. `reachable = true`.

픽스처: `metrics_v1.txt`는 스펙의 v1 메트릭 이름으로 각 1줄씩(히스토그램은 `le="0.1"`, `"0.5"`, `"+Inf"` 버킷 3줄), 중간에 `# HELP` 줄, `foo_unknown 1`, `vllm:bar NaN` 포함. `metrics_v0.txt`는 `vllm:gpu_cache_usage_perc`만 다름.

- [ ] **Step 1: 실패하는 테스트 작성**

```rust
#[test] fn parse_skips_unknown_and_nan() {
    let m = parse(include_str!("../../tests/fixtures/metrics_v1.txt"));
    assert_eq!(m.get("vllm:num_requests_running"), Some(3.0));
    assert!(m.get("vllm:bar").is_none() || m.get("vllm:bar").unwrap().is_nan() == false);
    assert_eq!(m.label("vllm:num_requests_running", "model_name").as_deref(), Some("qwen3-8b"));
}
#[test] fn v0_and_v1_kv_cache_both_map() {
    for f in ["metrics_v0.txt", "metrics_v1.txt"] { /* to_vllm(...).kv_cache_usage == Some(0.61) */ }
}
#[test] fn rate_resets_on_counter_decrease() {
    let t0 = Instant::now(); let mut r = Rate::default();
    assert_eq!(r.update(t0, Some(100.0)), None);
    assert_eq!(r.update(t0 + Duration::from_secs(2), Some(190.0)), Some(45.0));
    assert_eq!(r.update(t0 + Duration::from_secs(3), Some(10.0)), None);
    assert_eq!(r.update(t0 + Duration::from_secs(4), Some(20.0)), Some(10.0));
}
#[test] fn histogram_p50_from_buckets() { /* 버킷 0.1:10, 0.5:30, +Inf:40 → p50 ≈ 0.3 */ }
```

- [ ] **Step 2: 실패 확인** — `cargo test vllm`
- [ ] **Step 3: 구현** — 줄 단위 파싱: `#`로 시작하면 건너뜀, `name{labels} value` 또는 `name value`, `value.parse::<f64>()` 실패나 `is_nan()`이면 건너뜀. 라벨 값의 `\"` 이스케이프는 지원하지 않는다(`shortcut:` 주석).
- [ ] **Step 4: 통과 확인** — `cargo test vllm` → 4 passed
- [ ] **Step 5: 커밋** — `feat: prometheus parser and vllm metric mapping`

---

### Task 3: vLLM 수집 스레드와 공유 설정

**Files:**
- Modify: `Cargo.toml` (reqwest features에 `blocking` 추가)
- Create: `src/config.rs`
- Modify: `src/collectors/vllm.rs`, `src/collectors/mod.rs`, `src/main.rs`

**Interfaces:**
- Produces: `pub struct Config { pub interval_ms: AtomicU64, pub url: String, pub ascii: bool }`, `pub type SharedConfig = Arc<Config>`. `Config::interval(&self) -> Duration`. `pub const INTERVALS_MS: [u64; 5] = [500, 1000, 2000, 5000, 10000]`.
- Produces: `pub fn metrics_url(base: &str) -> String` — scheme 없으면 `http://`, 끝 `/` 제거, `/metrics` 붙임.
- Produces: `collectors::spawn(cfg: SharedConfig) -> Shared` — vllm 스레드 시작. 스레드 루프: `GET metrics_url` (timeout = interval, 최대 5s) → 성공이면 `to_vllm` 결과를 `snapshot.vllm`에 쓰고 `sampled_at = Some(now)`; 실패 3회 연속이면 `vllm = Vllm { reachable: false, ..Default }` + `errors`에 메시지(기존 같은 접두사 메시지는 교체). `thread::sleep(cfg.interval())`.
- `main.rs`: clap에 `--ascii` 추가, `Config` 생성 후 `collectors::spawn(cfg.clone())`.

- [ ] **Step 1: 실패하는 테스트** — `metrics_url_normalizes`: `"127.0.0.1:8000"`, `"http://h:8000/"`, `"https://h"` → 각각 `http://127.0.0.1:8000/metrics`, `http://h:8000/metrics`, `https://h/metrics`.
- [ ] **Step 2: 실패 확인** — `cargo test metrics_url`
- [ ] **Step 3: 구현** — 위 인터페이스대로. 모델명은 `label("vllm:num_requests_running","model_name")`, 없으면 `GET {base}/v1/models`의 `data[0].id`를 한 번만.
- [ ] **Step 4: 통과 확인** — `cargo test` 전체 녹색, `cargo run -- --url http://127.0.0.1:9` 실행 후 `q`로 정상 종료(DISCONNECTED 경로).
- [ ] **Step 5: 커밋** — `feat: vllm collector thread and shared config`

---

### Task 4: system 수집기와 서빙 프로세스 탐지

**Files:**
- Create: `src/collectors/system.rs`
- Modify: `src/collectors/mod.rs`

**Interfaces:**
- Produces: `pub fn find_serving<'a, I>(procs: I) -> Option<&'a ProcInfo>` where `ProcInfo { pid: u32, cmd: String, start_time: u64 }` — `cmd`에 `vllm` 포함 중 `start_time` 최소. (sysinfo `Process`에서 `ProcInfo`로 변환하는 어댑터는 스레드 쪽에 둔다.)
- Produces: `collectors::spawn`이 system 스레드도 시작. 루프: `sys.refresh_cpu_usage(); refresh_memory(); refresh_processes(ProcessesToUpdate::All, true)` → `snapshot.system`의 `cpu_pct, mem_used, mem_total, swap_used`, 디스크·넷은 `Disks`/`Networks`의 바이트 차분을 interval로 나눔, `snapshot.process`, `snapshot.top_processes`(cpu_usage 상위 5, `gpu_mem: None`).

- [ ] **Step 1: 실패하는 테스트** — `find_serving_picks_oldest_vllm`: cmd `["python -m vllm.entrypoints.openai.api_server" start 50, "vllm serve x" start 10, "bash" start 1]` → pid of start 10; 빈 입력 → `None`.
- [ ] **Step 2: 실패 확인** — `cargo test find_serving`
- [ ] **Step 3: 구현** — sysinfo 첫 `refresh_cpu_usage` 직후 값은 0이므로 두 번째 샘플부터 쓴다(첫 루프에서 `cpu_pct = None`).
- [ ] **Step 4: 통과 확인** — `cargo test`
- [ ] **Step 5: 커밋** — `feat: system collector with serving process detection`

---

### Task 5: 하드웨어 수집기 (macOS / Linux)

**Files:**
- Modify: `src/collectors/hw_macos.rs`
- Create: `src/collectors/hw_linux.rs`
- Modify: `src/collectors/mod.rs`

**Interfaces:**
- Produces(macOS): `pub fn parse_ioreg(text: &str) -> (Option<u64>, Option<u64>)` — `"In use system memory"=N`, `"Alloc system memory"=N` 추출. `pub fn spawn(state: Shared, cfg: SharedConfig)` — 루프: `sampler.get_metrics(cfg.interval().as_millis())` → `snapshot.gpu = vec![Gpu { name: 칩명(sysinfo `System::cpu_arch`나 `macmon` SocInfo 중 있는 것, 없으면 "Apple GPU"), utilization: gpu_active_ratio*100, freq_mhz, power_w: gpu_power, temp_c: temp.gpu_temp_avg, mem_*: parse_ioreg(Command::new("ioreg").args(["-r","-d","1","-c","IOAccelerator"]) 출력) }]`, `snapshot.system.cpu_power_w = cpu_power`, `ane_power_w`, `fans`.
- Produces(Linux): `pub fn spawn(state: Shared, cfg: SharedConfig)` — `Nvml::init()` 실패 시 `errors`에 한 줄 남기고 스레드 종료. 루프: 장치별 스펙 "hw_linux" 절의 NVML 호출 → `snapshot.gpu`; `running_compute_processes()`의 `(pid, used_gpu_memory)`를 `top_processes`의 같은 pid에 `gpu_mem`으로 합침.
- `mod.rs`: `#[cfg(target_os = "macos")] hw_macos::spawn(...)`, `#[cfg(target_os = "linux")] hw_linux::spawn(...)`.

- [ ] **Step 1: 실패하는 테스트** (macOS 파일) — `ioreg_parses_two_fields`(이 대화에서 본 실제 출력 형식 2줄), `ioreg_missing_keys_gives_none`(빈 문자열 → `(None, None)`).
- [ ] **Step 2: 실패 확인** — `cargo test ioreg`
- [ ] **Step 3: 구현** — 두 파일. 리눅스 파일은 맥에서 컴파일되지 않으므로 `cfg` 밖 코드를 두지 않는다.
- [ ] **Step 4: 통과 확인** — `cargo test`, `cargo run` 후 GPU 카드에 값이 뜨는지 눈으로 확인(Task 7 이후 재확인).
- [ ] **Step 5: 커밋** — `feat: macos (macmon) and linux (nvml) hardware collectors`

---

### Task 6: 표시용 포맷 헬퍼

**Files:**
- Create: `src/ui/format.rs`

**Interfaces:**
- Produces: `pub fn na<T: Display>(v: Option<T>, f: impl Fn(T) -> String) -> String`("n/a"), `pub fn bytes(b: u64) -> String`(`15.9 GB`, `512 MB`, 소수 1자리, 1024 기준), `pub fn num(f: f64, digits: usize) -> String`(천 단위 콤마), `pub fn uptime(secs: u64) -> String`(`3h12m`, `45s`, `2d 3h`), `pub fn spark(values: &[Option<f32>], ascii: bool) -> String`(값 범위를 `▁▂▃▄▅▆▇█` 8단계로, `None`은 공백, ascii면 ` .:-=+*#`).
- Produces: `pub fn level(v: Option<f64>, warn: f64, crit: f64) -> Level` where `enum Level { Ok, Warn, Crit, Unknown }`.

- [ ] **Step 1: 실패하는 테스트** — `bytes_formats`(`17_079_123_968 → "15.9 GB"`), `uptime_formats`, `spark_scales_to_eight_levels`(`[0,0.5,1]` → `"▁▅█"`), `level_thresholds`(79.9→Ok, 80→Warn, 95→Crit, None→Unknown).
- [ ] **Step 2: 실패 확인** → **Step 3: 구현** → **Step 4: `cargo test format` 통과**
- [ ] **Step 5: 커밋** — `feat: display format helpers`

---

### Task 7: Overview 화면과 단축키

**Files:**
- Modify: `src/ui/mod.rs`
- Create: `src/ui/overview.rs`, `src/ui/app.rs`

**Interfaces:**
- Produces: `pub struct App { pub snapshot: Snapshot, pub history: History, pub window_min: u8 /*1,5,30*/, pub frozen: bool, pub help: bool, pub tab: u8 }`, `App::on_key(&mut self, key: KeyEvent, cfg: &Config) -> bool`(true면 종료), `App::ingest(&mut self, snap: Snapshot)` — 히스토리 키: `gen_tok_s, prompt_tok_s, running, kv, gpu_util, gpu_mem, power, gpu_temp`.
- Produces: `pub fn draw(f: &mut Frame, app: &App, cfg: &Config)` — 스펙 "Overview" 절 레이아웃. 세로 분할: 헤더 2행 / 카드 행 2개(각 4행, 폭 < 100이면 2열×4행) / CPU·RAM 게이지 1행 / 요청 표 자리(프록시 안내 1줄) / 힌트 1행. 폭<80 또는 높이<24면 `Paragraph("터미널을 80x24 이상으로 키워 주세요")`만.
- 키: `1`–`4` tab, `+`/`-` `INTERVALS_MS` 인덱스 이동 후 `cfg.interval_ms.store`, `[`/`]` window 1→5→30, `Space` frozen 토글, `r` history.clear, `e` `serde_json` 없이 `format!("{:?}")`로 `vtop-<ts>.json` 저장은 하지 않고 — **`e`는 이 계획에서 제외**(스냅샷에 `Serialize`를 붙이는 Task를 Requests 계획으로 미룸), `?` help 토글, `q`/`Esc`.
- 루프: `event::poll(100ms)`로 입력 즉시 반응, 그리기는 마지막 그리기로부터 `min(interval, 1s)` 지났을 때(frozen이면 입력만).

- [ ] **Step 1: 실패하는 테스트** (`overview.rs` tests, `ratatui::backend::TestBackend` 120×40)
  - `overview_renders_cards_with_values`: `Vllm { generation_tok_s: Some(45.1), running: Some(3), waiting: Some(0), kv_cache_usage: Some(0.61) }`, `Gpu { utilization: Some(96.0), mem_used: Some(10.4 GiB), mem_total: Some(24 GiB), temp_c: Some(59.0) }` → 버퍼 문자열에 `"45.1"`, `"3 run · 0 wait"`, `"61 %"`, `"96 %"`, `"59 °C"` 포함, `"n/a"`는 Prompt tok/s 카드에 포함.
  - `too_small_terminal_shows_hint`: 60×20 → `"80x24"` 포함.
  - `keys_change_interval_and_window`: `+` 두 번 → `cfg.interval_ms == 2000`, `-` 세 번 → 500(하한); `]` 두 번 → `window_min == 30`, 한 번 더 → 30(상한).
- [ ] **Step 2: 실패 확인** — `cargo test overview`
- [ ] **Step 3: 구현** — 카드는 `Block::bordered().title()` + 2행 `Paragraph`(값 굵게 + 부제 흐리게, 스파크라인). 색은 `format::level`로 테두리 색만 바꾼다. 헤더 2행째는 `snapshot.process`에서 `uptime`, `cpu_pct`, `bytes(rss)`, `threads`.
- [ ] **Step 4: 통과 확인** — `cargo test` 전체, `cargo run`으로 실제 터미널에서 `+`/`-`/`[`/`]`/`Space`/`?`/`q` 동작 눈으로 확인.
- [ ] **Step 5: 커밋** — `feat: overview screen with sparkline cards and key bindings`

---

### Task 8: 릴리스 빌드와 README

**Files:**
- Create: `README.md`
- Modify: `Cargo.toml` (`[profile.release] lto = true, codegen-units = 1, strip = true`)

- [ ] **Step 1: 빌드** — `cargo build --release` → `target/release/vtop` 크기와 `ls -la`로 확인, 실행 후 `ps -o rss=` 로 RSS 20MB 미만인지 기록.
- [ ] **Step 2: README** — 설치(`cargo install --path .`), 실행 예, 단축키 표(스펙에서 복사, `e`와 `c`, `/`, `j/k`는 "다음 버전"으로 표시), 맥은 macmon 라이브러리 내장이라 추가 설치 없음, 리눅스는 NVIDIA 드라이버 필요.
- [ ] **Step 3: 커밋** — `chore: release profile and README`

---

## 다음 계획(이 문서 범위 밖)
- 프록시 + Requests 탭(`c`, `/`, `j/k`, `e` 포함)
- Engine 탭, Hardware 탭(프로세스 표, GPU별 행)
- GitHub Releases 바이너리
