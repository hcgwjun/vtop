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

    /// Sleeps one interval, re-reading it every 100 ms so a `+`/`-` press takes effect at once.
    pub fn sleep_interval(&self) {
        let start = std::time::Instant::now();
        while start.elapsed() < self.interval() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_interval_wakes_early_when_the_interval_shrinks() {
        let cfg = Config::new("http://x".into(), false);
        cfg.interval_ms.store(10_000, Ordering::Relaxed);
        let c = cfg.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            c.interval_ms.store(500, Ordering::Relaxed);
        });
        let t = std::time::Instant::now();
        cfg.sleep_interval();
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }
}
