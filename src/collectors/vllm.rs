//! vLLM `/metrics` (Prometheus text) → `Vllm`. Only `name{labels} value` lines are read; the rest is ignored.

use std::collections::HashMap;
use std::time::Instant;

use crate::snapshot::Vllm;

pub type Labels = Vec<(String, String)>;

#[derive(Debug, Default)]
pub struct Metrics(HashMap<String, Vec<(Labels, f64)>>);

/// Counter → per-second rate between two samples. `None` on the first sample and after a counter reset.
#[derive(Debug, Default)]
pub struct Rate {
    last: Option<(Instant, f64)>,
}

pub fn parse(text: &str) -> Metrics {
    let mut out: HashMap<String, Vec<(Labels, f64)>> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((head, value)) = line.rsplit_once(' ') else { continue };
        let Ok(value) = value.parse::<f64>() else { continue };
        if value.is_nan() {
            continue;
        }
        let (name, labels) = match head.split_once('{') {
            Some((n, rest)) => (n, parse_labels(rest.trim_end_matches('}'))),
            None => (head, Vec::new()),
        };
        out.entry(name.to_string()).or_default().push((labels, value));
    }
    Metrics(out)
}

// shortcut: label values with escaped quotes (\") are split wrongly; upgrade if a model name ever contains one
fn parse_labels(s: &str) -> Labels {
    s.split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        .collect()
}

impl Metrics {
    pub fn get(&self, name: &str) -> Option<f64> {
        self.0.get(name)?.first().map(|(_, v)| *v)
    }

    pub fn first_of(&self, names: &[&str]) -> Option<f64> {
        names.iter().find_map(|n| self.get(n))
    }

    pub fn label(&self, name: &str, key: &str) -> Option<String> {
        let (labels, _) = self.0.get(name)?.first()?;
        labels.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// Linear interpolation inside the bucket that crosses quantile `q` of the cumulative counts.
    pub fn histogram_quantile(&self, name: &str, q: f64) -> Option<f64> {
        let mut buckets: Vec<(f64, f64)> = self
            .0
            .get(&format!("{name}_bucket"))?
            .iter()
            .filter_map(|(labels, count)| {
                let le = labels.iter().find(|(k, _)| k == "le")?.1.parse::<f64>().ok()?;
                Some((le, *count))
            })
            .collect();
        buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
        let total = buckets.last()?.1;
        if total <= 0.0 {
            return None;
        }
        let target = q * total;
        let mut prev = (0.0, 0.0);
        for &(le, count) in &buckets {
            if count >= target {
                if le.is_infinite() {
                    return Some(prev.0);
                }
                let span = count - prev.1;
                let frac = if span > 0.0 { (target - prev.1) / span } else { 1.0 };
                return Some(prev.0 + (le - prev.0) * frac);
            }
            prev = (le, count);
        }
        None
    }
}

impl Rate {
    pub fn update(&mut self, now: Instant, counter: Option<f64>) -> Option<f64> {
        let c = counter?;
        let rate = match self.last {
            Some((t, prev)) if c >= prev && now > t => Some((c - prev) / (now - t).as_secs_f64()),
            _ => None,
        };
        self.last = Some((now, c));
        rate
    }
}

pub fn to_vllm(m: &Metrics, generation: &mut Rate, prompt: &mut Rate, now: Instant) -> Vllm {
    let hits = m.get("vllm:prefix_cache_hits");
    let queries = m.get("vllm:prefix_cache_queries").filter(|q| *q > 0.0);
    Vllm {
        reachable: true,
        model: m.label("vllm:num_requests_running", "model_name"),
        running: m.get("vllm:num_requests_running").map(|v| v as u64),
        waiting: m.get("vllm:num_requests_waiting").map(|v| v as u64),
        kv_cache_usage: m.first_of(&["vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc"]),
        prefix_cache_hit_rate: hits.zip(queries).map(|(h, q)| h / q),
        prompt_tok_s: prompt.update(now, m.get("vllm:prompt_tokens_total")),
        generation_tok_s: generation.update(now, m.get("vllm:generation_tokens_total")),
        ttft_p50_ms: m.histogram_quantile("vllm:time_to_first_token_seconds", 0.5).map(|s| s * 1000.0),
        ttft_p99_ms: m.histogram_quantile("vllm:time_to_first_token_seconds", 0.99).map(|s| s * 1000.0),
        preemptions_total: m.get("vllm:num_preemptions_total").map(|v| v as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const V1: &str = include_str!("../../tests/fixtures/metrics_v1.txt");
    const V0: &str = include_str!("../../tests/fixtures/metrics_v0.txt");

    #[test]
    fn parse_skips_unknown_and_nan() {
        let m = parse(V1);
        assert_eq!(m.get("vllm:num_requests_running"), Some(3.0));
        assert_eq!(m.get("vllm:bar"), None);
        assert_eq!(m.get("foo_unknown"), Some(1.0));
        assert_eq!(m.label("vllm:num_requests_running", "model_name").as_deref(), Some("qwen3-8b"));
    }

    #[test]
    fn v0_and_v1_kv_cache_both_map() {
        for text in [V0, V1] {
            let v = to_vllm(&parse(text), &mut Rate::default(), &mut Rate::default(), Instant::now());
            assert_eq!(v.kv_cache_usage, Some(0.61));
            assert_eq!(v.running, Some(3));
            assert_eq!(v.prefix_cache_hit_rate, Some(0.25));
            assert_eq!(v.preemptions_total, Some(2));
            assert_eq!(v.model.as_deref(), Some("qwen3-8b"));
            assert!(v.reachable);
        }
    }

    #[test]
    fn rate_resets_on_counter_decrease() {
        let t0 = Instant::now();
        let mut r = Rate::default();
        assert_eq!(r.update(t0, Some(100.0)), None);
        assert_eq!(r.update(t0 + Duration::from_secs(2), Some(190.0)), Some(45.0));
        assert_eq!(r.update(t0 + Duration::from_secs(3), Some(10.0)), None);
        assert_eq!(r.update(t0 + Duration::from_secs(4), Some(20.0)), Some(10.0));
    }

    #[test]
    fn histogram_p50_from_buckets() {
        let m = parse(V1);
        let p50 = m.histogram_quantile("vllm:time_to_first_token_seconds", 0.5).unwrap();
        assert!((p50 - 0.3).abs() < 1e-9, "{p50}");
        assert_eq!(m.histogram_quantile("vllm:nope", 0.5), None);
    }
}
