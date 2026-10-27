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
```

## Keys

| Key | Action |
|---|---|
| `1`–`4` | Overview / Engine / Requests / Hardware (tabs 2–4: next version) |
| `+` / `-` | sampling interval 0.5 / 1 / 2 / 5 / 10 s |
| `[` / `]` | sparkline window 1 / 5 / 30 min |
| `Space` | freeze the screen (collection continues) |
| `r` | reset sparklines and the tok/s baseline |
| `?` | help |
| `q` / `Esc` | quit / close |

Next version: `--proxy` (OpenAI-compatible pass-through that records per-request TTFT, tokens and tok/s), `c` clear requests, `/` filter, `j`/`k` select, `e` export snapshot.
