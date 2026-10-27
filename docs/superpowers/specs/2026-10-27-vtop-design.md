# vtop 설계 — vLLM 터미널 모니터

2026-10-27

## 목적

맥(vllm-metal)과 리눅스(vLLM + NVIDIA)에서 **같은 화면**으로 추론 서버 상태와 하드웨어 수치를 한눈에 보는 단일 바이너리 TUI. 각 기계에서 vtop이 자기 자신을 모니터링한다(원격 수집 없음). Strata Monitor의 화면 구성을 참고하되 코드는 가져오지 않는다.

성공 기준
- `vtop --url http://127.0.0.1:8000` 한 줄로 실행. 외부 바이너리(macmon, nvidia-smi) 설치 불필요.
- 유휴 시 CPU 1% 미만, RSS 20MB 미만, 1초 주기 갱신에서 화면 끊김 없음.
- 한쪽 OS에만 있는 값은 숨기지 않고 `n/a`로 표시해 두 OS의 레이아웃이 동일하다.

## 범위

포함: Overview 탭(첫 버전), Engine / Requests / Hardware 탭(이후), 선택 기능인 OpenAI 호환 프록시.
제외: Strata의 Connect·Logs 탭, 원격 에이전트, 설정 파일, 테마 전환, vLLM 외 서버 어댑터.

## 스택

Rust 2024, 단일 프로세스.

| 역할 | 크레이트 |
|---|---|
| TUI | ratatui 0.30 + crossterm |
| HTTP 클라이언트 / 프록시 | reqwest(rustls) / axum + tokio |
| CPU·RAM·디스크·넷 | sysinfo |
| 맥 전력·주파수·온도·팬 | macmon 0.10 (라이브러리, `target_os = "macos"`) |
| 맥 GPU 메모리 | `ioreg -r -d1 -c IOAccelerator`의 `PerformanceStatistics` (IOReport에 없음) |
| 리눅스 GPU | nvml-wrapper (`target_os = "linux"`) |
| CLI | clap |

## 아키텍처

```
main ─ clap ─┬─ collectors::spawn(url, interval)  ── Arc<RwLock<Snapshot>> ──┐
             │     ├ vllm 스레드     GET /metrics → 파싱 → 차분 → Snapshot.vllm │
             │     ├ system 스레드   sysinfo → Snapshot.system                  │
             │     └ hw 스레드       macmon | NVML → Snapshot.gpu, power, temp  │
             ├─ proxy (선택, tokio)  /v1/* 통과 + 요청 기록 → RequestLog        │
             └─ ui::run             1초(또는 설정 주기)마다 Snapshot 읽어 그림 ◄┘
```

- 수집기는 각자 스레드에서 `interval`마다 샘플링하고 자기 영역만 덮어쓴다. UI는 읽기만 하므로 수집 지연이 화면을 막지 않는다.
- `interval`은 `Arc<AtomicU64>`(ms)로 공유하고 UI의 `+`/`-`가 바꾼다. 수집기는 매 루프 시작에 읽는다.
- 히스토리(스파크라인용)는 UI 스레드가 Snapshot을 읽을 때 링버퍼(30분 × 최소 주기 0.5s = 3,600칸, 값당 f32)에 쌓는다. 수집기는 히스토리를 모른다.
- 수집 실패는 해당 영역을 `None`으로 두고 `Snapshot.errors`에 한 줄 메시지를 남긴다. 패닉하지 않는다.

## 스냅샷 스키마 (`snapshot.rs`)

```rust
Snapshot { vllm: Vllm, gpu: Vec<Gpu>, system: System, process: Option<Process>,
           top_processes: Vec<Process>, errors: Vec<String>, sampled_at: Instant }
Process { pid, name, uptime_s, cpu_pct, rss, threads, gpu_mem: Option<u64> }
Vllm   { reachable, model, running, waiting, kv_cache_usage, prefix_cache_hit_rate,
         prompt_tok_s, generation_tok_s, ttft_p50_ms, ttft_p99_ms, preemptions_total }
Gpu    { name, utilization, mem_used, mem_total, power_w, power_limit_w, temp_c, freq_mhz }
System { cpu_pct, cpu_power_w, ane_power_w, mem_used, mem_total, swap_used,
         disk_read_bps, disk_write_bps, net_rx_bps, net_tx_bps, fans: Vec<(name, rpm)> }
```
모든 수치는 `Option`. 맥의 GPU는 `Vec<Gpu>`에 한 개(통합 메모리이므로 `mem_total`은 `ioreg`의 `Alloc system memory`, `mem_used`는 `In use system memory`). 리눅스는 NVML 장치 수만큼.

## 수집기

### vllm
- `GET {url}/metrics` 텍스트를 직접 파싱(의존성 없이 `name{labels} value` 줄만 처리).
- v0/v1 이름이 다른 메트릭은 둘 다 찾는다: `vllm:gpu_cache_usage_perc` | `vllm:kv_cache_usage_perc`.
- 사용 메트릭: `vllm:num_requests_running`, `vllm:num_requests_waiting`, KV 사용률, `vllm:prompt_tokens_total`, `vllm:generation_tokens_total`(차분/초 → tok/s), `vllm:time_to_first_token_seconds` 히스토그램(버킷으로 p50/p99 근사), `vllm:num_preemptions_total`, `vllm:prefix_cache_hits`/`queries`(v1).
- 모델명은 메트릭 라벨 `model_name`에서, 없으면 `GET /v1/models`에서 한 번만.
- 연결 실패 3회 연속이면 `reachable=false`, 헤더에 `DISCONNECTED`.

### system (공통)
sysinfo로 CPU 사용률, RAM, 스왑, 디스크·넷 바이트 차분. 같은 스레드에서 서빙 프로세스도 찾는다: 명령줄에 `vllm`이 포함된 프로세스 중 시작 시각이 가장 이른 것 → `Snapshot.process = Some(Process { pid, name, uptime, cpu_pct, rss, threads })`, 없으면 `None`(헤더에 `process: not found`). Hardware 탭용으로 CPU 상위 5개 프로세스도 담는다(`Snapshot.top_processes`, 리눅스는 NVML `running_compute_processes`로 프로세스별 GPU 메모리를 합쳐 넣는다).

### hw_macos
`macmon::Sampler::get_metrics(interval_ms)` → `cpu_power`, `gpu_power`, `ane_power`, `gpu_active_ratio`, `gpu_freq_mhz`, `temp.gpu_temp_avg`, `fans`. `get_metrics`는 구간 동안 블로킹하므로 이 스레드가 곧 타이머 역할을 한다. GPU 메모리는 같은 스레드에서 `ioreg` 한 번 실행해 두 필드만 읽는다. `power_limit_w`는 `None`.

### hw_linux
NVML `device_count` → 장치별 `utilization_rates().gpu`, `memory_info()`, `power_usage()`, `enforced_power_limit()`, `temperature()`, `clock_info(Graphics)`.

## 프록시 (선택, `--proxy 0.0.0.0:8001`)

- axum이 `/v1/*`를 받아 `{url}`로 그대로 전달. 헤더와 바디는 수정 없이 스트리밍 통과(복사 없이 `Body::from_stream`).
- 요청별 기록: 시작 시각, 모델, 상태(진행/완료/오류), 프롬프트 토큰, 출력 토큰, TTFT(첫 바이트까지), 총 시간, tok/s.
- 토큰 수: 응답의 `usage`(비스트리밍, 또는 `stream_options.include_usage`일 때 마지막 청크)를 우선 쓰고, 없으면 SSE `data:` 청크 수로 출력 토큰을 근사하고 표에 `~`를 붙인다. 프롬프트 토큰은 그 경우 `n/a`.
- OpenAI SDK 호환 규칙: 상태 코드·헤더·SSE 형식을 바꾸지 않는다. vtop 자체 응답은 내지 않는다(업스트림 불통이면 502 그대로).
- 기록은 최근 1,000건 링버퍼. 프록시가 꺼져 있으면 Overview의 요청 표 자리에 `--proxy로 켜면 요청별 기록이 보입니다` 한 줄.

## 화면

공통: 1행 헤더 `vtop  <model>  <url>  <LIVE|STALE|DISCONNECTED>  [1 Overview] 2 Engine 3 Requests 4 Hardware   ⟳1.0s`, 2행 서빙 프로세스 `vllm  PID 41233 · up 3h12m · CPU 180% · RSS 4.2 GB · 12 threads`(없으면 `process: not found`), 마지막 행 단축키 힌트. 최소 크기 80×24, 그보다 작으면 "터미널을 키워 주세요" 한 줄.

색: 터미널 기본 전경/배경을 따르고 **상태색 3개만** 쓴다 — 정상(녹), 주의(황: 사용률 80%↑, 온도 80°C↑, 대기열 1↑), 경고(적: 95%↑, 90°C↑, DISCONNECTED). 나머지 텍스트는 밝기 2단계(값은 굵게, 라벨은 흐리게). 유니코드 블록 문자(`▁▂▃▄▅▆▇█`)를 쓰되 `--ascii`면 `#`로 대체.

### Overview (첫 버전)
```
┌ Gen tok/s ─────┐┌ Prompt tok/s ──┐┌ Requests ──────┐┌ KV cache ──────┐
│ 45.1           ││ 1,204          ││ 3 run · 0 wait ││ 61 %           │
│ ▂▃▅▇▆▅▃▂▃▅▇▇▆▅ ││ ▁▁▂▇▁▁▁▅▁▁▁▁▂▁ ││ ▁▂▃▃▃▃▂▂▃▃▃▃▃▃ ││ ▃▄▅▆▆▆▆▆▆▆▆▆▆▆ │
└────────────────┘└────────────────┘└────────────────┘└────────────────┘
┌ GPU ───────────┐┌ GPU mem ───────┐┌ Power ─────────┐┌ GPU temp ──────┐
│ 96 %  M4 Pro   ││ 10.4 / 24 GB   ││ 18 W  cpu 3 W  ││ 59 °C          │
│ ▇▇▇▇▇▇▇▇▇▇▇▇▇▇ ││ ▆▆▆▆▆▆▆▆▆▆▆▆▆▆ ││ ▅▆▆▇▇▇▆▆▆▆▇▇▆▆ ││ ▄▄▅▅▅▅▅▅▅▅▅▅▅▅ │
└────────────────┘└────────────────┘└────────────────┘└────────────────┘
CPU  ████████░░░░ 62 %      RAM  ██████████░░ 15.9 / 24 GB
Recent requests (proxy)
TIME      STATUS  MODEL          PROMPT  OUTPUT  TTFT    TOK/S  TIME
15:33:51  done    qwen3-8b        18,187     261  0.42s   36.3   8.7s
```
카드는 폭에 맞춰 4열 → 2열로 접힌다. GPU가 여러 개면 GPU 카드들은 합계/평균을 보이고 Hardware 탭에서 개별로 본다.

### Engine
스케줄러(실행·대기·preemption), KV·prefix 캐시 사용률과 적중률, TTFT·E2E 지연 히스토그램을 가로 막대로.

### Requests
프록시 기록 전체 표, `/` 필터, `j/k` 이동, 선택 행 상세(요청 헤더 일부, 오류 본문).

### Hardware
GPU별 행(사용률·메모리·전력·온도·클럭), CPU 코어별 막대, 팬, 디스크·넷 I/O 스파크라인, 프로세스 표(CPU 상위 5개: PID·이름·CPU%·RSS·GPU 메모리(리눅스만)).

## 단축키

| 키 | 동작 |
|---|---|
| `1`–`4` | 탭 전환 |
| `+` / `-` | 수집 주기 0.5 / 1 / 2 / 5 / 10 s |
| `[` / `]` | 스파크라인 시간창 1 / 5 / 30 분 |
| `Space` | 화면 고정(수집은 계속) |
| `r` | tok/s 기준점·히스토리 초기화 |
| `c` | 요청 기록 비우기 |
| `/` | Requests 필터 |
| `j` / `k` | 행·GPU 선택 |
| `e` | 스냅샷을 `vtop-<YYYYmmdd-HHMMSS>.json`으로 저장 |
| `?` | 도움말 오버레이 |
| `q` / `Esc` | 종료 / 오버레이·필터 닫기 |

## 오류 처리

- 수집기 오류는 영역을 `None`으로 두고 헤더 오른쪽에 마지막 오류 한 줄(흐리게). 패닉은 UI 스레드에서만 잡아 터미널을 복구한 뒤 종료.
- `/metrics` 파싱은 모르는 줄을 무시한다. 카운터가 줄어들면(서버 재시작) 차분을 버리고 기준점을 다시 잡는다.
- 프록시 업스트림 오류는 그대로 통과시키고 기록에 `error <status>`로 남긴다.

## 테스트

- `collectors::vllm::parse`: 실제 vLLM `/metrics` 샘플 파일로 단위 테스트(v0, v1 각 하나).
- 차분 계산: 카운터 감소 시 기준점 리셋 테스트.
- 프록시: 가짜 업스트림(axum)으로 스트리밍 청크 통과와 `usage` 집계 테스트.
- hw: 각 OS에서 `Sampler`/NVML이 한 번 샘플링되는 스모크 테스트(이미 macOS용 1개 있음).
- UI: 스냅샷 하나를 `TestBackend`로 그려 카드 텍스트가 나오는지 스냅샷 테스트 1개.

## 구현 순서

1. vllm 수집기 + Overview 상단 카드 4개 (서버 없이도 `DISCONNECTED`로 뜸)
2. system + hw_macos / hw_linux 수집기 + 하단 카드 4개, CPU/RAM 게이지
3. 단축키 전부, 스파크라인 시간창
4. 프록시 + 요청 표
5. Engine, Hardware, Requests 탭
6. `cargo build --release` 바이너리를 GitHub Releases로 (macOS arm64, linux x86_64)
