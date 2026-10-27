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
    /// Record per-request stats through an OpenAI-compatible proxy on 127.0.0.1:<PORT> (default 30800)
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "30800")]
    proxy: Option<u16>,
    /// Draw sparklines with ASCII instead of block characters
    #[arg(long)]
    ascii: bool,
}

impl Args {
    /// Localhost only: the proxy forwards anything it receives to vLLM, so it is never exposed on the network.
    fn proxy_addr(&self) -> Option<String> {
        self.proxy.map(|p| format!("127.0.0.1:{p}"))
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = config::Config::new(args.url.clone(), args.ascii);
    let log = match args.proxy_addr() {
        Some(bind) => {
            let log = requests::SharedLog::default();
            proxy::server::spawn(&bind, cfg.url.clone(), log.clone())?;
            Some(log)
        }
        None => None,
    };
    let state = collectors::spawn(cfg.clone());
    ui::run(state, cfg, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_flag_is_localhost_with_a_default_port() {
        assert_eq!(Args::parse_from(["vtop"]).proxy_addr(), None);
        assert_eq!(Args::parse_from(["vtop", "--proxy"]).proxy_addr().as_deref(), Some("127.0.0.1:30800"));
        assert_eq!(Args::parse_from(["vtop", "--proxy", "9000"]).proxy_addr().as_deref(), Some("127.0.0.1:9000"));
    }
}
