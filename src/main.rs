mod collectors;
mod config;
mod history;
mod proxy;
mod requests;
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
    /// Draw sparklines with ASCII instead of block characters
    #[arg(long)]
    ascii: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = config::Config::new(args.url, args.ascii);
    let log = requests::SharedLog::default();
    if let Some(bind) = &args.proxy {
        proxy::server::spawn(bind, cfg.url.clone(), log.clone())?;
    }
    let state = collectors::spawn(cfg.clone());
    ui::run(state, cfg)
}
