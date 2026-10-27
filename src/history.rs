//! Per-metric ring buffers the UI fills from snapshots; 3,600 slots covers 30 min at the 0.5 s minimum interval.

use std::collections::{HashMap, VecDeque};

pub const CAPACITY: usize = 3600;

#[derive(Default)]
pub struct History {
    series: HashMap<&'static str, VecDeque<Option<f32>>>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, key: &'static str, v: Option<f32>) {
        let s = self.series.entry(key).or_insert_with(|| VecDeque::with_capacity(CAPACITY));
        if s.len() == CAPACITY {
            s.pop_front();
        }
        s.push_back(v);
    }

    /// Latest `n` values, oldest first, front-padded with `None`.
    pub fn window(&self, key: &str, n: usize) -> Vec<Option<f32>> {
        let s = self.series.get(key);
        let have = s.map_or(0, |s| s.len().min(n));
        let mut out = vec![None; n - have];
        if let Some(s) = s {
            out.extend(s.iter().skip(s.len() - have).copied());
        }
        out
    }

    pub fn clear(&mut self) {
        self.series.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_returns_latest_n_oldest_first() {
        let mut h = History::new();
        for v in [1.0, 2.0, 3.0] {
            h.push("x", Some(v));
        }
        assert_eq!(h.window("x", 2), vec![Some(2.0), Some(3.0)]);
        assert_eq!(h.window("missing", 2), vec![None, None]);
    }

    #[test]
    fn capacity_is_3600() {
        let mut h = History::new();
        for i in 0..4000 {
            h.push("x", Some(i as f32));
        }
        assert_eq!(h.window("x", 3600)[0], Some(400.0));
    }
}
