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
    /// Port of the OpenAI-compatible proxy on 127.0.0.1 that records per-request stats
    #[arg(long, value_name = "PORT", default_value_t = 30800)]
    proxy: u16,
    /// Draw sparklines with ASCII instead of block characters
    #[arg(long)]
    ascii: bool,
}

impl Args {
    /// Localhost only: the proxy forwards anything it receives to vLLM, so it is never exposed on the network.
    fn proxy_addr(&self) -> String {
        format!("127.0.0.1:{}", self.proxy)
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = config::Config::new(args.url.clone(), args.ascii);
    let log = requests::SharedLog::default();
    proxy::server::spawn(&args.proxy_addr(), cfg.url.clone(), log.clone())?;
    let state = collectors::spawn(cfg.clone());
    ui::run(state, cfg, Some(log))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_flag_is_localhost_with_a_default_port() {
        assert_eq!(Args::parse_from(["vtop"]).proxy_addr(), "127.0.0.1:30800");
        assert_eq!(Args::parse_from(["vtop", "--proxy", "8888"]).proxy_addr(), "127.0.0.1:8888");
    }
}
