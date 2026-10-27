//! Per-request records the proxy fills and the UI reads; a 1,000-slot ring shared behind a mutex.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;

pub const CAPACITY: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub enum Status {
    InFlight,
    Done(u16),
    Error(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: u64,
    pub started_unix_ms: u64,
    pub path: String,
    pub model: Option<String>,
    pub stream: bool,
    pub status: Status,
    pub prompt_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Output tokens were counted from SSE chunks, not reported by the server.
    pub approx: bool,
    pub ttft_ms: Option<f64>,
    pub duration_ms: Option<f64>,
}

impl Record {
    /// Output tokens per second of decode time (duration minus TTFT); `None` until both are known.
    pub fn tok_s(&self) -> Option<f64> {
        let decode_s = (self.duration_ms? - self.ttft_ms.unwrap_or(0.0)) / 1000.0;
        let out = self.output_tokens? as f64;
        (decode_s > 0.0).then(|| out / decode_s)
    }
}

#[derive(Default)]
pub struct RequestLog {
    items: VecDeque<Record>,
    next_id: u64,
}

impl RequestLog {
    /// Assigns the next id (from 1), drops the oldest record when full, returns the id.
    pub fn push(&mut self, mut r: Record) -> u64 {
        self.next_id += 1;
        r.id = self.next_id;
        if self.items.len() == CAPACITY {
            self.items.pop_front();
        }
        self.items.push_back(r);
        self.next_id
    }

    pub fn update(&mut self, id: u64, f: impl FnOnce(&mut Record)) {
        if let Some(r) = self.items.iter_mut().rev().find(|r| r.id == id) {
            f(r);
        }
    }

    /// Newest first.
    pub fn latest(&self, n: usize) -> Vec<Record> {
        self.items.iter().rev().take(n).cloned().collect()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

pub type SharedLog = Arc<Mutex<RequestLog>>;

/// "15:33:51". shortcut: UTC (std has no local time zone); the column is labeled TIME(UTC), add chrono if local time is needed.
pub fn hhmmss_utc(unix_ms: u64) -> String {
    let s = unix_ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> Record {
        Record {
            id: 0,
            started_unix_ms: 0,
            path: "/v1/chat/completions".into(),
            model: None,
            stream: false,
            status: Status::InFlight,
            prompt_tokens: None,
            output_tokens: None,
            approx: false,
            ttft_ms: None,
            duration_ms: None,
        }
    }

    #[test]
    fn log_keeps_last_1000() {
        let mut log = RequestLog::default();
        for _ in 0..1100 {
            log.push(rec());
        }
        assert_eq!(log.len(), 1000);
        assert_eq!(log.latest(1)[0].id, 1100);
    }

    #[test]
    fn update_by_id() {
        let mut log = RequestLog::default();
        let first = log.push(rec());
        log.push(rec());
        log.update(first, |r| r.status = Status::Done(200));
        assert!(matches!(log.latest(2)[1].status, Status::Done(200)));
    }

    #[test]
    fn tok_s_uses_decode_time() {
        let mut r = rec();
        r.output_tokens = Some(300);
        r.ttft_ms = Some(1000.0);
        r.duration_ms = Some(4000.0);
        assert_eq!(r.tok_s(), Some(100.0));
        r.duration_ms = Some(1000.0);
        assert_eq!(r.tok_s(), None);
    }

    #[test]
    fn hhmmss_formats_utc() {
        assert_eq!(hhmmss_utc(0), "00:00:00");
        assert_eq!(hhmmss_utc(55_431_000), "15:23:51");
    }
}
