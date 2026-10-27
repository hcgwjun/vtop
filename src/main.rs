mod collectors;
mod snapshot;
mod ui;

use clap::Parser;

/// vLLM terminal monitor for macOS and Linux.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// vLLM base URL (the /metrics and /v1 endpoints live here)
    #[arg(long, default_value = "http://127.0.0.1:8000")]
    url: String,
    /// Bind an OpenAI-compatible proxy here to record per-request stats
    #[arg(long)]
    proxy: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let state = collectors::spawn(&args.url);
    ui::run(state)
}
