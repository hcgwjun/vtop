//! The one shape every collector writes and every screen reads.
//! `None` means "this platform or server does not report it" and renders as n/a.

use std::sync::{Arc, RwLock};

#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub vllm: Vllm,
    pub gpu: Vec<Gpu>,
    pub system: System,
}

#[derive(Debug, Default, Clone)]
pub struct Vllm {
    pub reachable: bool,
    pub model: Option<String>,
    pub running: Option<u64>,
    pub waiting: Option<u64>,
    pub kv_cache_usage: Option<f64>,
    pub prompt_tok_s: Option<f64>,
    pub generation_tok_s: Option<f64>,
}

#[derive(Debug, Default, Clone)]
pub struct Gpu {
    pub name: String,
    pub utilization: Option<f64>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub temp_c: Option<f64>,
}

#[derive(Debug, Default, Clone)]
pub struct System {
    pub cpu_pct: Option<f64>,
    pub cpu_power_w: Option<f64>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
}

pub type Shared = Arc<RwLock<Snapshot>>;
