# vtop

A single-binary terminal monitor for [vLLM](https://github.com/vllm-project/vllm) on macOS (vllm-metal) and Linux (NVIDIA). One screen shows the server (tok/s, running/waiting requests, KV cache) next to the hardware (GPU load, memory, power, temperature, CPU, RAM), plus a per-request table (TTFT, queue, prefill, decode, tokens) fed by vLLM's own request traces. Same layout on both platforms; `n/a` where a platform has no value.

## Quick start

### 1. Install

Pick one. All three put a `vtop` binary on your PATH.

```sh
# Homebrew (macOS or Linux). Recent Homebrew asks you to trust a third-party tap once.
brew tap hcgwjun/tap && brew trust hcgwjun/tap
brew install hcgwjun/tap/vtop

# Installer script (macOS Apple Silicon, Linux x86_64/aarch64) — installs to ~/.cargo/bin
curl -LsSf https://github.com/hcgwjun/vtop/releases/latest/download/vtop-installer.sh | sh

# From source, with a Rust toolchain (https://rustup.rs)
cargo install vtop
```

Archives and checksums for each platform are on the [releases page](https://github.com/hcgwjun/vtop/releases).

- macOS: nothing else. Apple Silicon power and temperature come from the [macmon](https://github.com/vladkens/macmon) library compiled in — no sudo, no extra tools.
- Linux: an NVIDIA driver (NVML) for the GPU cards. Without one they show `n/a`; everything else works.

### 2. Tell vLLM to send request traces

Skip this if you only want the server and hardware cards. The per-request table (TTFT, queue, prefill, decode, tokens) comes from vLLM's OpenTelemetry tracing, which vtop receives on `127.0.0.1:4318`. Your clients and the vLLM port do not change.

In the environment that runs vLLM:

```sh
pip install opentelemetry-sdk opentelemetry-exporter-otlp     # once; vllm-metal already has them

export OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf        # vtop speaks HTTP, not gRPC
export OTEL_BSP_SCHEDULE_DELAY=1000                            # rows appear within 1 s instead of 5 s
vllm serve <model> --port 8000 --otlp-traces-endpoint http://127.0.0.1:4318/v1/traces
```

Put the two `export` lines in your shell profile or the script that starts vLLM. vLLM starts fine when vtop is not running; spans it cannot deliver are dropped after a few retries.

### 3. Run

```sh
vtop                                   # vLLM at http://127.0.0.1:8000
vtop --url http://127.0.0.1:8111       # another vLLM base URL (a /v1 suffix is fine)
vtop --otlp 4319                       # another span port (then use :4319 in step 2)
vtop --ascii                           # plain ASCII sparklines
```

### 4. Check

- The header shows `LIVE` and the model name: `/metrics` is reachable. `DISCONNECTED`: check `--url`.
- Send one request. A row appears under "Recent requests" within about a second. If not, check vLLM's startup log for `otlp_traces_endpoint` and the two `OTEL_` variables.
- `vtop` exits with `cannot bind otlp receiver`: port 4318 is taken (another vtop or an OpenTelemetry collector) — use `--otlp`.
- `?` lists the keys, `q` quits.

## Screens

**Overview** (`1`): eight cards with sparklines, CPU/RAM gauges, and the most recent requests.

**Engine** (`2`): scheduler (running, waiting, preemptions), KV and prefix cache, and the TTFT and end-to-end latency histograms vLLM has accumulated since it started.

**Requests** (`3`): every recorded request (last 1,000), a cursor, a filter line and the detail of the selected row.

| Column | Meaning |
|---|---|
| TIME | when the request arrived, local time |
| ID | vLLM request id without its `chatcmpl-` / `cmpl-` prefix |
| PROMPT / OUTPUT | prompt and generated tokens |
| TTFT ms | time to first token |
| QUEUE ms | waiting in the scheduler queue before the first forward pass |
| DECODE s | time spent generating after the first token |
| TOK/S | (OUTPUT − 1) ÷ DECODE — the first token belongs to prefill |
| E2E s | arrival to last token |

Requests that never reach the engine (bad model name, context overflow) produce no span and no row.

**Hardware** (`4`): one row per GPU (utilization, memory, power and limit, temperature, clock), per-core CPU bars, RAM/swap/fans, disk and network throughput with sparklines, and the five busiest processes (GPU memory per process on Linux only).

## Keys

| Key | Action |
|---|---|
| `1`–`4` | Overview / Engine / Requests / Hardware |
| `+` / `-` | sampling interval 0.5 / 1 / 2 / 5 / 10 s |
| `[` / `]` | sparkline window 1 / 5 / 30 min |
| `Space` | freeze the screen (collection continues) |
| `r` | reset sparklines |
| `j` / `k` | Requests: move the selection (it stays on its row as new requests arrive) |
| `/` | Requests: filter — space-separated terms must all match the request id; `>N` keeps rows with at least N output tokens; `Enter` keeps, `Esc` clears |
| `c` | Requests: clear the log |
| `e` | export the snapshot and all recorded requests to `vtop-<YYYYmmdd-HHMMSS>.json` in the current directory |
| `?` | help |
| `q` / `Esc` | quit / close |
