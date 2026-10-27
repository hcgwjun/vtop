//! The one shape every collector writes and every screen reads.
//! `None` means "this platform or server does not report it" and renders as n/a.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde::Serialize;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Snapshot {
    pub vllm: Vllm,
    pub gpu: Vec<Gpu>,
    pub system: System,
    pub process: Option<Process>,
    pub top_processes: Vec<Process>,
    pub errors: Vec<String>,
    #[serde(skip)]
    pub sampled_at: Option<Instant>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Vllm {
    pub reachable: bool,
    pub model: Option<String>,
    pub running: Option<u64>,
    pub waiting: Option<u64>,
    pub kv_cache_usage: Option<f64>,
    pub prefix_cache_hit_rate: Option<f64>,
    pub prompt_tok_s: Option<f64>,
    pub generation_tok_s: Option<f64>,
    pub ttft_p50_ms: Option<f64>,
    pub ttft_p99_ms: Option<f64>,
    pub preemptions_total: Option<u64>,
    /// `(le, count in this bucket)` ascending, last `le` is +Inf; empty until the server has samples.
    pub ttft_buckets: Vec<(f64, u64)>,
    pub e2e_buckets: Vec<(f64, u64)>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Gpu {
    pub name: String,
    pub utilization: Option<f64>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub temp_c: Option<f64>,
    pub freq_mhz: Option<u32>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct System {
    pub cpu_pct: Option<f64>,
    /// Per-core %, empty on the first sample.
    pub cpu_cores: Vec<f64>,
    pub cpu_power_w: Option<f64>,
    pub ane_power_w: Option<f64>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub swap_used: Option<u64>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    pub fans: Vec<(String, u32)>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub uptime_s: u64,
    pub cpu_pct: f64,
    pub rss: u64,
    pub threads: Option<u64>,
    pub gpu_mem: Option<u64>,
}

pub type Shared = Arc<RwLock<Snapshot>>;
