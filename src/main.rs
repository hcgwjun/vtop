mod collectors;
mod config;
mod history;
mod otlp;
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
    /// Port on 127.0.0.1 where vLLM's --otlp-traces-endpoint sends request spans
    #[arg(long, value_name = "PORT", default_value_t = 4318)]
    otlp: u16,
    /// Draw sparklines with ASCII instead of block characters
    #[arg(long)]
    ascii: bool,
}

impl Args {
    /// Localhost only: spans come from a vLLM on this machine.
    fn otlp_addr(&self) -> String {
        format!("127.0.0.1:{}", self.otlp)
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = config::Config::new(args.url.clone(), args.ascii);
    let log = requests::SharedLog::default();
    otlp::server::spawn(&args.otlp_addr(), log.clone())?;
    let state = collectors::spawn(cfg.clone());
    ui::run(state, cfg, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otlp_flag_is_localhost_with_a_default_port() {
        assert_eq!(Args::parse_from(["vtop"]).otlp_addr(), "127.0.0.1:4318");
        assert_eq!(Args::parse_from(["vtop", "--otlp", "4319"]).otlp_addr(), "127.0.0.1:4319");
    }
}
