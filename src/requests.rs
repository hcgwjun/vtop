//! Per-request records built from vLLM's `llm_request` spans; a 1,000-slot ring shared behind a mutex.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::otlp::proto::Span;

pub const CAPACITY: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: u64,
    pub started_unix_ms: u64,
    pub request_id: String,
    pub prompt_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub ttft_ms: Option<f64>,
    pub queue_ms: Option<f64>,
    pub prefill_ms: Option<f64>,
    pub decode_ms: Option<f64>,
    pub e2e_ms: Option<f64>,
    pub max_tokens: Option<u64>,
}

impl Record {
    /// Only `llm_request` spans describe a request; vLLM also emits loading spans at startup.
    pub fn from_span(s: &Span) -> Option<Record> {
        if s.name != "llm_request" {
            return None;
        }
        let ms = |key: &str| s.f64(key).map(|v| v * 1000.0);
        Some(Record {
            id: 0,
            started_unix_ms: s.start_time_unix_nano / 1_000_000,
            request_id: s.str("gen_ai.request.id").unwrap_or("-").to_string(),
            prompt_tokens: s.u64("gen_ai.usage.prompt_tokens"),
            output_tokens: s.u64("gen_ai.usage.completion_tokens"),
            ttft_ms: ms("gen_ai.latency.time_to_first_token"),
            queue_ms: ms("gen_ai.latency.time_in_queue"),
            prefill_ms: ms("gen_ai.latency.time_in_model_prefill"),
            decode_ms: ms("gen_ai.latency.time_in_model_decode"),
            e2e_ms: ms("gen_ai.latency.e2e")
                .or(Some(s.end_time_unix_nano.saturating_sub(s.start_time_unix_nano) as f64 / 1e6)),
            max_tokens: s.u64("gen_ai.request.max_tokens"),
        })
    }

    /// Output tokens per second of decode time; `None` until both are known or when decode took no time.
    pub fn tok_s(&self) -> Option<f64> {
        let decode_s = self.decode_ms? / 1000.0;
        let out = self.output_tokens? as f64;
        (decode_s > 0.0).then(|| out / decode_s)
    }
}

#[cfg(test)]
pub fn sample(request_id: &str, output: u64) -> Record {
    Record {
        id: 0,
        started_unix_ms: 0,
        request_id: request_id.into(),
        prompt_tokens: None,
        output_tokens: Some(output),
        ttft_ms: None,
        queue_ms: None,
        prefill_ms: None,
        decode_ms: None,
        e2e_ms: None,
        max_tokens: None,
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

    /// Newest first.
    pub fn latest(&self, n: usize) -> Vec<Record> {
        self.items.iter().rev().take(n).cloned().collect()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

pub type SharedLog = Arc<Mutex<RequestLog>>;

/// Broken-down local time (the machine's zone, via libc so no chrono dependency).
fn local_tm(unix_ms: u64) -> libc::tm {
    let t = (unix_ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call; a null return leaves tm zeroed (1900-01-00 00:00:00)
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

/// "15:33:51" in local time.
pub fn hhmmss_local(unix_ms: u64) -> String {
    let t = local_tm(unix_ms);
    format!("{:02}:{:02}:{:02}", t.tm_hour, t.tm_min, t.tm_sec)
}

/// "20261027-153351" in local time, for export file names.
pub fn stamp_local(unix_ms: u64) -> String {
    let t = local_tm(unix_ms);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otlp::proto::parse_spans;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/otlp_llm_request.bin");

    #[test]
    fn log_keeps_last_1000() {
        let mut log = RequestLog::default();
        for _ in 0..1100 {
            log.push(sample("x", 1));
        }
        assert_eq!(log.latest(2000).len(), 1000);
        assert_eq!(log.latest(1)[0].id, 1100);
    }

    #[test]
    fn record_from_fixture_span() {
        let r = Record::from_span(&parse_spans(FIXTURE).unwrap()[0]).unwrap();
        assert!(r.request_id.starts_with("chatcmpl-"), "{}", r.request_id);
        assert_eq!((r.prompt_tokens, r.output_tokens), (Some(419), Some(266)));
        let decode = r.decode_ms.unwrap();
        assert!((500.0..3000.0).contains(&decode), "{decode}");
        let tok_s = r.tok_s().unwrap();
        assert!((50.0..400.0).contains(&tok_s), "{tok_s}");
        assert!(r.started_unix_ms > 1_700_000_000_000);
        assert!(r.e2e_ms.unwrap() > r.ttft_ms.unwrap());
    }

    #[test]
    fn other_spans_are_ignored() {
        let mut s = parse_spans(FIXTURE).unwrap()[0].clone();
        s.name = "Overall Loading".into();
        assert!(Record::from_span(&s).is_none());
    }

    #[test]
    fn tok_s_none_when_decode_is_zero() {
        let mut r = sample("x", 1);
        r.decode_ms = Some(0.0);
        assert_eq!(r.tok_s(), None);
        r.decode_ms = Some(500.0);
        r.output_tokens = Some(100);
        assert_eq!(r.tok_s(), Some(200.0));
    }

    /// Runs in a child process: setting TZ in this process would race the other tests' localtime_r calls.
    #[test]
    fn local_time_uses_tz() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "requests::tests::local_time_child"])
            .env("TZ", "Asia/Seoul")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }

    #[test]
    #[ignore = "driven by local_time_uses_tz with TZ=Asia/Seoul"]
    fn local_time_child() {
        assert_eq!(hhmmss_local(0), "09:00:00");
        assert_eq!(stamp_local(0), "19700101-090000");
    }
}
