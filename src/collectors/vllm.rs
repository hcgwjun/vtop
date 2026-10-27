//! vLLM `/metrics` (Prometheus text) → `Vllm`. Only `name{labels} value` lines are read; the rest is ignored.

use std::collections::HashMap;
use std::time::Instant;

use std::thread;

use crate::config::SharedConfig;
use crate::snapshot::{Shared, Vllm};

pub type Labels = Vec<(String, String)>;

#[derive(Debug, Default)]
pub struct Metrics(HashMap<String, Vec<(Labels, f64)>>);

/// Counter → per-second rate between two samples. `None` on the first sample and after a counter reset.
#[derive(Debug, Default)]
pub struct Rate {
    last: Option<(Instant, f64)>,
}

/// Scheme added, trailing `/`, `/v1` (OpenAI base URLs) and `/metrics` removed.
fn base_url(base: &str) -> String {
    let b = if base.contains("://") { base.to_string() } else { format!("http://{base}") };
    let b = b.trim_end_matches('/');
    b.strip_suffix("/metrics").or_else(|| b.strip_suffix("/v1")).unwrap_or(b).trim_end_matches('/').to_string()
}

pub fn metrics_url(base: &str) -> String {
    format!("{}/metrics", base_url(base))
}

pub fn models_url(base: &str) -> String {
    format!("{}/v1/models", base_url(base))
}

/// Polls `/metrics` every interval; three straight failures mark the server unreachable.
pub fn spawn(state: Shared, cfg: SharedConfig) {
    thread::Builder::new()
        .name("vllm".into())
        .spawn(move || {
            let url = metrics_url(&cfg.url);
            let models_url = models_url(&cfg.url);
            let (mut generation, mut prompt) = (Rate::default(), Rate::default());
            let mut failures = 0u32;
            let mut model: Option<String> = None;
            let mut model_tried = false; // /v1/models is asked once per connection, never on every tick
            let client = reqwest::blocking::Client::new();
            // fixed: a loaded vLLM can take longer than a 0.5 s interval to answer /metrics
            let timeout = std::time::Duration::from_secs(5);
            loop {
                let body = client.get(&url).timeout(timeout).send().and_then(|r| r.error_for_status()).and_then(|r| r.text());
                // everything slow (HTTP, parsing) happens before the lock is taken
                let result = body.map(|text| {
                    failures = 0;
                    let now = Instant::now();
                    let mut v = to_vllm(&parse(&text), &mut generation, &mut prompt, now);
                    if v.model.is_none() {
                        if !model_tried {
                            model_tried = true;
                            model = model_from_api(&client, &models_url, timeout);
                        }
                        v.model = model.clone();
                    }
                    (v, now)
                });
                let mut snap = state.write().unwrap();
                match result {
                    Ok((v, now)) => {
                        snap.vllm = v;
                        snap.sampled_at = Some(now);
                        snap.errors.retain(|e| !e.starts_with("vllm:"));
                    }
                    Err(e) => {
                        failures += 1;
                        model_tried = false;
                        if failures >= 3 {
                            snap.vllm = Vllm::default();
                            snap.errors.retain(|e| !e.starts_with("vllm:"));
                            snap.errors.push(format!("vllm: {e}"));
                        }
                    }
                }
                drop(snap);
                cfg.sleep_interval();
            }
        })
        .expect("spawn vllm thread");
}

fn model_from_api(client: &reqwest::blocking::Client, url: &str, timeout: std::time::Duration) -> Option<String> {
    let v: serde_json::Value = client.get(url).timeout(timeout).send().ok()?.json().ok()?;
    v["data"][0]["id"].as_str().map(str::to_string)
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
    /// Sum over every label set (data-parallel vLLM reports one series per engine).
    pub fn get(&self, name: &str) -> Option<f64> {
        let series = self.0.get(name)?;
        (!series.is_empty()).then(|| series.iter().map(|(_, v)| v).sum())
    }

    /// Mean over label sets, for ratios such as KV cache usage.
    pub fn avg(&self, name: &str) -> Option<f64> {
        let series = self.0.get(name)?;
        (!series.is_empty()).then(|| series.iter().map(|(_, v)| v).sum::<f64>() / series.len() as f64)
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
        // one series per engine: add up the counts that share an `le`
        buckets.dedup_by(|next, prev| {
            let same = next.0 == prev.0;
            if same {
                prev.1 += next.1;
            }
            same
        });
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
    // prometheus_client appends _total to counters; older vLLM prefixed these with gpu_
    let hits = m.first_of(&["vllm:prefix_cache_hits_total", "vllm:prefix_cache_hits", "vllm:gpu_prefix_cache_hits_total"]);
    let queries = m
        .first_of(&["vllm:prefix_cache_queries_total", "vllm:prefix_cache_queries", "vllm:gpu_prefix_cache_queries_total"])
        .filter(|q| *q > 0.0);
    Vllm {
        reachable: true,
        model: m.label("vllm:num_requests_running", "model_name"),
        running: m.get("vllm:num_requests_running").map(|v| v as u64),
        waiting: m.get("vllm:num_requests_waiting").map(|v| v as u64),
        kv_cache_usage: ["vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc"].iter().find_map(|n| m.avg(n)),
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
    fn label_sets_are_summed_and_histograms_merged_by_le() {
        // data parallel: every series comes once per engine
        let two_engines: String = V1
            .lines()
            .flat_map(|l| {
                if l.starts_with("vllm:") && l.contains('{') {
                    vec![l.replace("{", "{engine=\"0\","), l.replace("{", "{engine=\"1\",")]
                } else {
                    vec![l.to_string()]
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let (one, two) = (parse(V1), parse(&two_engines));
        assert_eq!(two.get("vllm:num_requests_running"), Some(6.0), "3 per engine");
        let (p1, p2) = (one.histogram_quantile("vllm:time_to_first_token_seconds", 0.5).unwrap(), two.histogram_quantile("vllm:time_to_first_token_seconds", 0.5).unwrap());
        assert!((p1 - p2).abs() < 1e-9, "same distribution on both engines → same p50: {p1} vs {p2}");
    }

    #[test]
    fn histogram_p50_from_buckets() {
        let m = parse(V1);
        let p50 = m.histogram_quantile("vllm:time_to_first_token_seconds", 0.5).unwrap();
        assert!((p50 - 0.3).abs() < 1e-9, "{p50}");
        assert_eq!(m.histogram_quantile("vllm:nope", 0.5), None);
    }
}

#[cfg(test)]
mod url_tests {
    use super::{metrics_url, models_url};

    #[test]
    fn metrics_url_normalizes() {
        assert_eq!(metrics_url("127.0.0.1:8000"), "http://127.0.0.1:8000/metrics");
        assert_eq!(metrics_url("http://h:8000/"), "http://h:8000/metrics");
        assert_eq!(metrics_url("https://h"), "https://h/metrics");
        assert_eq!(metrics_url("http://h:8000/v1"), "http://h:8000/metrics", "OpenAI base URLs end in /v1");
        assert_eq!(metrics_url("http://h:8000/metrics"), "http://h:8000/metrics");
        assert_eq!(models_url("http://h:8000/v1/"), "http://h:8000/v1/models");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Real vllm-metal 0.x `/metrics` capture (PaddleOCR-VL, after a few requests, prefix caching off).
    #[test]
    fn live_vllm_metal_capture_maps() {
        let m = parse(include_str!("../../tests/fixtures/metrics_live_vllm_metal.txt"));
        let v = to_vllm(&m, &mut Rate::default(), &mut Rate::default(), Instant::now());
        assert_eq!(v.model.as_deref(), Some("PaddlePaddle/PaddleOCR-VL-1.6"));
        assert_eq!(v.running, Some(0));
        assert_eq!(v.waiting, Some(0));
        assert_eq!(v.kv_cache_usage, Some(0.0));
        assert_eq!(v.preemptions_total, Some(0));
        assert_eq!(v.prefix_cache_hit_rate, None, "prefix caching disabled → no queries → n/a");
        assert!(v.ttft_p50_ms.is_some_and(|t| t > 0.0), "{:?}", v.ttft_p50_ms);
        assert!(m.get("vllm:generation_tokens_total").is_some_and(|t| t > 0.0));
    }
}
