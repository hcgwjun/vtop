# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`vtop` is a single-binary terminal monitor for one vLLM server (macOS vllm-metal, Linux NVIDIA). Rust 2024, ratatui 0.30. It polls vLLM's `/metrics`, samples hardware, and receives vLLM's OpenTelemetry request spans; four tabs render it all. The design spec is `docs/superpowers/specs/2026-10-27-vtop-design.md`; per-feature plans live in `docs/superpowers/plans/`.

## Commands

Rust was installed with `--no-modify-path`, so every cargo command in this environment needs:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

```sh
cargo test                                  # whole suite (~70 tests, < 1 s)
cargo test otlp::proto                      # one module
cargo test -- --exact requests::tests::tok_s_none_when_decode_is_zero
cargo clippy --all-targets                  # 3 known field_reassign_with_default warnings in tests are accepted
cargo build --release                       # target/release/vtop
cargo install --path .                      # installs to ~/.cargo/bin/vtop
dist plan                                   # cargo-dist release preview (brew install cargo-dist)
dist build --artifacts local --target aarch64-apple-darwin
```

Linux cannot be cross-checked from macOS (reqwest's rustls pulls a C library); verify on Linux with Docker instead:

```sh
docker run --rm -v "$PWD":/src:ro -w /src rust:1-bookworm sh -c 'cp -r /src /b && cd /b && cargo test --target-dir /tmp/t'
```

Live check against a real server: `vtop --url http://127.0.0.1:8111` (this machine's OCR model via the playground `serve.sh`). The UI cannot be driven from a pipe; run it in a pty (`python3 -c 'import pty'`-style driver) and reconstruct frames from the ANSI output, or rely on the `TestBackend` tests.

## Architecture

Three collector threads write one `Arc<RwLock<Snapshot>>` (`src/snapshot.rs`); the UI thread clones it every tick and draws. A fourth thread runs an axum server on a tokio runtime that receives spans into `Arc<Mutex<RequestLog>>` (`src/requests.rs`). Nothing else is shared; `Config` (`src/config.rs`) carries the only runtime knob (sampling interval, an atomic the keys change and collectors read via `sleep_interval()`).

- `src/collectors/vllm.rs` — `/metrics` text → `Metrics` (name → label sets) → `Vllm`. Counters become rates through `Rate`; histograms are read both as quantiles and per-bucket counts. Label sets are summed (gauges averaged) so data-parallel engines add up. Three failures in a row mark the server unreachable. `/v1/models` is asked once per connection for the model name.
- `src/collectors/system.rs` — sysinfo: CPU (global and per core), RAM, disk/net byte deltas, the serving process (prefers a `vllm serve` command line, falls back to any command containing `vllm`), top-5 processes. Process table refreshes every 5th tick because walking processes is the expensive part.
- `src/collectors/hw_macos.rs` — macmon as a library (IOReport/SMC, no sudo): GPU utilization, power, temperature, clock, fans. GPU memory is "Alloc system memory" from `ioreg`, refreshed every 5th tick. `hw_linux.rs` uses NVML; it only compiles on Linux.
- `src/otlp/` — `proto.rs` is a hand-written prost subset of the OTLP trace protobuf (only the fields vtop reads; unknown fields are skipped). `server.rs` answers `POST /v1/traces`: 400 if the body is not protobuf, else 200 with an empty body. Only `llm_request` spans become `Record`s. vLLM must run with `--otlp-traces-endpoint http://127.0.0.1:4318/v1/traces`, `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf` and ideally `OTEL_BSP_SCHEDULE_DELAY=1000` (see README §2).
- `src/ui/` — `mod.rs` is the loop (poll keys at 100 ms, ingest+draw every `min(interval, 1 s)`). `app.rs` holds state, key handling and the request filter/selection (selection follows a record by id as rows arrive). `overview.rs` owns the header, hints, help overlay, tab dispatch and the shared `Card`/`card()`/`sparkline()` used by `engine.rs`; `hardware.rs` carves fixed-height sections top-down; `requests_tab.rs` and `table.rs` render the request log. `history.rs` keeps 3,600 samples per sparkline key; `format.rs` has the number/bytes/bar helpers.

## Conventions that matter here

- `panic = "abort"` in both profiles: collectors and the receiver must not panic. `unwrap` is for mutex locks only; everything else is `Option`/`Result` and renders `n/a`.
- UI strings are English on purpose: CJK double-width glyphs break ratatui column math. Request times are local time (libc `localtime_r`), not UTC.
- Known shortcuts are marked `shortcut: <limit>, <when to upgrade>` in code comments; treat them as decisions.
- UI tests render into `ratatui::backend::TestBackend` and assert on the buffer text; histogram/table helpers that take `width`/`height` are tested directly. `tests/fixtures/` holds real captures (`metrics_live_vllm_metal.txt`, `otlp_llm_request.bin`) plus hand-made metric files; tests that need a time zone run in a child process rather than setting `TZ` in-process.
- Releases are cargo-dist (`dist-workspace.toml`, `.github/workflows/release.yml`): tag `vX.Y.Z` → macOS arm64 and Linux x86_64/aarch64 archives, shell installer, Homebrew formula pushed to `hcgwjun/homebrew-tap` (needs the `HOMEBREW_TAP_TOKEN` secret).
- `.superpowers/` is git-ignored scratch for plan execution ledgers; `docs/superpowers/` is committed.
