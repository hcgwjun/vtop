//! Runtime knobs shared between the UI (which changes them) and the collectors (which read them each loop).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const INTERVALS_MS: [u64; 5] = [500, 1000, 2000, 5000, 10000];

pub struct Config {
    pub interval_ms: AtomicU64,
    pub url: String,
    pub ascii: bool,
}

pub type SharedConfig = Arc<Config>;

impl Config {
    pub fn new(url: String, ascii: bool) -> SharedConfig {
        Arc::new(Self { interval_ms: AtomicU64::new(1000), url, ascii })
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms.load(Ordering::Relaxed))
    }
}
