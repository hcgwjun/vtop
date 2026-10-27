//! Tab 2: scheduler and cache cards on top, TTFT and end-to-end latency histograms below.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::app::App;
use super::format::{Level, bar, level, na, num};
use super::overview::{Card, card};
use crate::config::Config;

pub fn draw(f: &mut Frame, app: &App, cfg: &Config, area: Rect) {
    let v = &app.snapshot.vllm;
    let card_h = (area.height / 3).clamp(3, 6);
    let [top, bottom] = Layout::vertical([Constraint::Length(card_h), Constraint::Min(0)]).areas(area);
    let kv_pct = v.kv_cache_usage.map(|k| k * 100.0);
    let cards = [
        Card {
            title: "Scheduler",
            value: format!("{} run · {} wait", na(v.running, |x| x.to_string()), na(v.waiting, |x| x.to_string())),
            sub: na(v.preemptions_total, |x| format!("{x} preemptions")),
            key: "running",
            level: level(v.waiting.map(|w| w as f64), 1.0, f64::INFINITY),
        },
        Card { title: "KV cache", value: na(kv_pct, |x| format!("{} %", num(x, 0))), sub: "of blocks in use".into(), key: "kv", level: level(kv_pct, 80.0, 95.0) },
        Card {
            title: "Prefix cache",
            value: na(v.prefix_cache_hit_rate, |x| format!("prefix hit {} %", num(x * 100.0, 0))),
            sub: "n/a when prefix caching is off".into(),
            key: "prefix_hit",
            level: Level::Unknown,
        },
    ];
    let cols = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(top);
    for (c, cell) in cols.iter().enumerate() {
        card(f, app, cfg, &cards[c], *cell);
    }
    let [left, right] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(bottom);
    histogram(f, cfg, left, "TTFT", &v.ttft_buckets, v.ttft_p50_ms, v.ttft_p99_ms);
    histogram(f, cfg, right, "E2E latency", &v.e2e_buckets, None, None);
}

/// Title line, then one bar per bucket from the first non-empty one; counts are since server start.
fn histogram(f: &mut Frame, cfg: &Config, area: Rect, title: &'static str, buckets: &[(f64, u64)], p50_ms: Option<f64>, p99_ms: Option<f64>) {
    if area.height == 0 || area.width < 4 {
        return;
    }
    let mut head = vec![title.bold()];
    if let Some(p) = p50_ms {
        head.push(format!("  p50 {} ms", num(p, 0)).dim());
    }
    if let Some(p) = p99_ms {
        head.push(format!("  p99 {} ms", num(p, 0)).dim());
    }
    head.push("  · since start".dim());
    let mut lines = vec![Line::from(head)];
    let rows = hist_rows(buckets);
    if rows.is_empty() {
        lines.push(Line::from("no samples yet".dim()));
    } else {
        let max = rows.iter().map(|r| r.1).max().unwrap_or(1).max(1) as f64;
        let count_w = rows.iter().map(|r| num(r.1 as f64, 0).len()).max().unwrap_or(1);
        let bar_w = (area.width as usize).saturating_sub(11 + 2 + 2 + count_w).max(1);
        for (label, count) in rows.iter().take(area.height as usize - 1) {
            lines.push(Line::from(vec![
                format!("{label:>11}  ").dim(),
                bar(*count as f64 / max, bar_w, cfg.ascii).into(),
                format!("  {:>count_w$}", num(*count as f64, 0)).into(),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// `(label, count)` from the first to the last non-empty bucket: "0.25–0.5 s", "1–2.5 s", ">10 s".
fn hist_rows(buckets: &[(f64, u64)]) -> Vec<(String, u64)> {
    let Some(first) = buckets.iter().position(|(_, c)| *c > 0) else { return Vec::new() };
    let last = buckets.iter().rposition(|(_, c)| *c > 0).unwrap_or(first);
    let mut lo = if first == 0 { 0.0 } else { buckets[first - 1].0 };
    buckets[first..=last]
        .iter()
        .map(|&(le, c)| {
            let label = if le.is_infinite() { format!(">{} s", secs(lo)) } else { format!("{}–{} s", secs(lo), secs(le)) };
            lo = le;
            (label, c)
        })
        .collect()
}

fn secs(v: f64) -> String {
    if v == v.trunc() { format!("{v:.0}") } else { format!("{v}") }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::app::App;
    use crate::config::Config;
    use crate::snapshot::{Snapshot, Vllm};

    fn render(w: u16, h: u16, snap: Snapshot) -> String {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut app = App { tab: 1, ..App::default() };
        app.ingest(snap);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::super::overview::draw(f, &app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    fn snap() -> Snapshot {
        Snapshot {
            vllm: Vllm {
                reachable: true,
                running: Some(3),
                waiting: Some(1),
                preemptions_total: Some(2),
                kv_cache_usage: Some(0.61),
                prefix_cache_hit_rate: Some(0.25),
                ttft_p50_ms: Some(420.0),
                ttft_p99_ms: Some(1980.0),
                ttft_buckets: vec![(0.25, 0), (0.5, 10), (1.0, 30), (f64::INFINITY, 2)],
                e2e_buckets: vec![(1.0, 5), (2.0, 0), (f64::INFINITY, 0)],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn engine_renders_scheduler_and_histograms() {
        let out = render(120, 40, snap());
        for needle in ["3 run · 1 wait", "2 preemptions", "61 %", "prefix hit 25 %", "TTFT", "E2E", "0.25–0.5 s", "0.5–1 s", "30", "p50 420 ms"] {
            assert!(out.contains(needle), "missing {needle:?}\n{out}");
        }
        assert!(!out.contains("0–0.25 s"), "leading empty buckets are hidden\n{out}");
        assert!(!out.contains("1–2 s"), "trailing empty buckets are hidden\n{out}");
    }

    #[test]
    fn engine_renders_without_samples() {
        let out = render(120, 40, Snapshot { vllm: Vllm { reachable: true, ..Default::default() }, ..Default::default() });
        assert_eq!(out.matches("no samples yet").count(), 2, "{out}");
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for (w, h) in [(20, 5), (40, 8), (60, 12)] {
            render(w, h, snap());
        }
    }
}
