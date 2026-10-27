# vtop

A single-binary terminal monitor for [vLLM](https://github.com/vllm-project/vllm) on macOS (vllm-metal) and Linux (NVIDIA). One screen shows the server (tok/s, running/waiting requests, KV cache) next to the hardware (GPU load, memory, power, temperature, CPU, RAM) — the same layout on both platforms, with `n/a` where a platform has no value.

## Install

```sh
cargo install --path .
```

- macOS: nothing else. Apple Silicon power and temperature come from the [macmon](https://github.com/vladkens/macmon) library compiled in — no sudo, no extra tools.
- Linux: an NVIDIA driver (NVML). Without one the GPU cards show `n/a`.

## Run

```sh
vtop --url http://127.0.0.1:8000     # vLLM base URL (default)
vtop --ascii                         # plain ASCII sparklines
vtop --proxy 127.0.0.1:8001          # also record per-request stats (see below)
```

## Per-request stats (`--proxy`)

vLLM's `/metrics` has no per-request numbers, so vtop can sit in front of the server as an OpenAI-compatible pass-through: it forwards every request and response byte-for-byte to `--url` (same path, method, headers, status, SSE stream) and records, per request, the model, status, prompt/output tokens, TTFT, duration and decode tok/s. Point clients at the proxy instead of vLLM:

```sh
vtop --url http://127.0.0.1:8000 --proxy 127.0.0.1:8001
export OPENAI_BASE_URL=http://127.0.0.1:8001/v1        # OpenAI SDK
export VL_REC_SERVER_URL=http://localhost:8001/v1      # paddlex OCR
```

Rows appear at the bottom of Overview and in the Requests tab (`3`). Output tokens come from `usage`; streams without `stream_options.include_usage` fall back to counting content chunks, shown as `~N`. If vLLM is unreachable the client gets `502 vtop: upstream unreachable: ...` and the row shows `error`. Times are UTC.

## Keys

| Key | Action |
|---|---|
| `1`–`4` | Overview / Engine / Requests / Hardware (tabs 2 and 4: next version) |
| `+` / `-` | sampling interval 0.5 / 1 / 2 / 5 / 10 s |
| `[` / `]` | sparkline window 1 / 5 / 30 min |
| `Space` | freeze the screen (collection continues) |
| `r` | reset sparklines and the tok/s baseline |
| `j` / `k` | Requests: move the selection |
| `/` | Requests: filter — space-separated terms must all match the model or status; `>N` keeps rows with at least N output tokens; `Enter` keeps, `Esc` clears |
| `c` | Requests: clear the log |
| `e` | export the snapshot and all recorded requests to `vtop-<YYYYmmdd-HHMMSS>.json` in the current directory |
| `?` | help |
| `q` / `Esc` | quit / close |

Next version: Engine tab, Hardware tab (process table, per-GPU rows).
