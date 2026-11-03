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
    /// [default: first free of 4318-4327]
    #[arg(long, value_name = "PORT")]
    otlp: Option<u16>,
    /// Draw sparklines with ASCII instead of block characters
    #[arg(long)]
    ascii: bool,
}

impl Args {
    /// An explicit --otlp is that port or an error; otherwise the first free of ten from OTLP's 4318, so a
    /// second vtop (another vLLM) starts without flags.
    fn otlp_ports(&self) -> Vec<u16> {
        self.otlp.map_or_else(|| (4318..4328).collect(), |p| vec![p])
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let log = requests::SharedLog::default();
    let otlp_port = otlp::server::spawn(&args.otlp_ports(), log.clone())?;
    let cfg = config::Config::with_otlp(args.url.clone(), args.ascii, otlp_port);
    let state = collectors::spawn(cfg.clone());
    ui::run(state, cfg, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otlp_ports_default_to_ten_from_4318() {
        let auto = Args::parse_from(["vtop"]).otlp_ports();
        assert_eq!((auto.len(), auto[0], auto[9]), (10, 4318, 4327));
        assert_eq!(Args::parse_from(["vtop", "--otlp", "4400"]).otlp_ports(), vec![4400], "explicit means exactly that port");
    }
}
