//! Overview: header, eight sparkline cards, CPU/RAM gauges, request table slot, key hints.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::app::App;
use super::format::{Level, bytes, level, na, num, spark, uptime};
use crate::config::Config;

const TABS: [&str; 4] = ["Overview", "Engine", "Requests", "Hardware"];
const HINTS: &str = "1-4 tabs  +/- interval  [/] window  Space freeze  r reset  ? help  q quit";

pub fn draw(f: &mut Frame, app: &App, cfg: &Config) {
    let area = f.area();
    if area.width < 80 || area.height < 24 {
        f.render_widget(Paragraph::new("Terminal too small: vtop needs at least 80x24"), area);
        return;
    }
    let [header, body, hints] = Layout::vertical([Constraint::Length(2), Constraint::Min(1), Constraint::Length(1)]).areas(area);
    f.render_widget(Paragraph::new(header_lines(app, cfg)), header);
    if app.tab == 0 {
        overview(f, app, cfg, body);
    } else {
        f.render_widget(Paragraph::new(format!("{} tab is not implemented yet", TABS[app.tab as usize])).dim(), body);
    }
    f.render_widget(Paragraph::new(HINTS).dim(), hints);
    if app.help {
        help_overlay(f, area);
    }
}

fn header_lines(app: &App, cfg: &Config) -> Vec<Line<'static>> {
    let s = &app.snapshot;
    let interval = cfg.interval();
    let (state, color) = if !s.vllm.reachable {
        ("DISCONNECTED", Color::Red)
    } else if s.sampled_at.is_some_and(|t| t.elapsed() > interval * 3) {
        ("STALE", Color::Yellow)
    } else {
        ("LIVE", Color::Green)
    };
    let mut line1 = vec![
        "vtop".bold(),
        "  ".into(),
        s.vllm.model.clone().unwrap_or_else(|| "no model".into()).into(),
        "  ".into(),
        cfg.url.clone().dim(),
        "  ".into(),
        Span::styled(state, Style::default().fg(color).bold()),
        "  ".into(),
    ];
    for (i, name) in TABS.iter().enumerate() {
        let label = format!("{} {}", i + 1, name);
        line1.push(if i as u8 == app.tab { format!("[{label}]").bold() } else { format!(" {label} ").dim() });
    }
    line1.push(format!("  ⟳{:.1}s  {}m", interval.as_secs_f64(), app.window_min).dim());
    if app.frozen {
        line1.push("  FROZEN".yellow().bold());
    }
    let line2 = match &s.process {
        Some(p) => format!(
            "{}  PID {} · up {} · CPU {:.0}% · RSS {} · {} threads",
            p.name, p.pid, uptime(p.uptime_s), p.cpu_pct, bytes(p.rss), na(p.threads, |t| t.to_string())
        ),
        None => "process: not found".into(),
    };
    let mut line2 = vec![line2.dim()];
    if let Some(e) = s.errors.last() {
        line2.push("   ".into());
        line2.push(e.clone().red().dim());
    }
    vec![Line::from(line1), Line::from(line2)]
}

/// Subtitles are optional context, so a missing one is blank rather than n/a.
fn sub<T>(v: Option<T>, f: impl Fn(T) -> String) -> String {
    v.map(f).unwrap_or_default()
}

struct Card {
    title: &'static str,
    value: String,
    sub: String,
    key: &'static str,
    level: Level,
}

fn cards(app: &App) -> Vec<Card> {
    let s = &app.snapshot;
    let v = &s.vllm;
    let gpus = &s.gpu;
    let avg = |get: fn(&crate::snapshot::Gpu) -> Option<f64>| {
        let vals: Vec<f64> = gpus.iter().filter_map(get).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let sum_u = |get: fn(&crate::snapshot::Gpu) -> Option<u64>| {
        let vals: Vec<u64> = gpus.iter().filter_map(get).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<u64>())
    };
    let gpu_name = match gpus.len() {
        0 => "no GPU".to_string(),
        1 => gpus[0].name.clone(),
        n => format!("{n} × {}", gpus[0].name),
    };
    let kv_pct = v.kv_cache_usage.map(|k| k * 100.0);
    let power_limit = avg(|g| g.power_limit_w).map(|l| format!("of {} W", num(l * gpus.len() as f64, 0)));
    vec![
        Card { title: "Gen tok/s", value: na(v.generation_tok_s, |x| num(x, 1)), sub: "now".into(), key: "gen_tok_s", level: Level::Unknown },
        Card { title: "Prompt tok/s", value: na(v.prompt_tok_s, |x| num(x, 0)), sub: sub(v.ttft_p50_ms, |x| format!("TTFT p50 {} ms", num(x, 0))), key: "prompt_tok_s", level: Level::Unknown },
        Card {
            title: "Requests",
            value: format!("{} run · {} wait", na(v.running, |x| x.to_string()), na(v.waiting, |x| x.to_string())),
            sub: sub(v.preemptions_total, |x| format!("{x} preemptions")),
            key: "running",
            level: level(v.waiting.map(|w| w as f64), 1.0, f64::INFINITY),
        },
        Card {
            title: "KV cache",
            value: na(kv_pct, |x| format!("{} %", num(x, 0))),
            sub: sub(v.prefix_cache_hit_rate, |x| format!("prefix hit {} %", num(x * 100.0, 0))),
            key: "kv",
            level: level(kv_pct, 80.0, 95.0),
        },
        Card { title: "GPU", value: na(avg(|g| g.utilization), |x| format!("{} %", num(x, 0))), sub: gpu_name, key: "gpu_util", level: level(avg(|g| g.utilization), 80.0, 95.0) },
        Card {
            title: "GPU mem",
            value: match (sum_u(|g| g.mem_used), sum_u(|g| g.mem_total)) {
                (Some(u), Some(t)) => format!("{} / {}", bytes(u), bytes(t)),
                (Some(u), None) => bytes(u),
                _ => "n/a".into(),
            },
            sub: sub(avg(|g| g.freq_mhz.map(|f| f as f64)), |x| format!("{} MHz", num(x, 0))),
            key: "gpu_mem",
            level: match (sum_u(|g| g.mem_used), sum_u(|g| g.mem_total)) {
                (Some(u), Some(t)) if t > 0 => level(Some(u as f64 / t as f64 * 100.0), 80.0, 95.0),
                _ => Level::Unknown,
            },
        },
        Card {
            title: "Power",
            value: na(avg(|g| g.power_w).map(|p| p * gpus.len() as f64), |x| format!("{} W", num(x, 0))),
            sub: power_limit.unwrap_or_else(|| sub(s.system.cpu_power_w, |x| format!("cpu {} W", num(x, 1)))),
            key: "power",
            level: Level::Unknown,
        },
        Card { title: "GPU temp", value: na(avg(|g| g.temp_c), |x| format!("{} °C", num(x, 0))), sub: String::new(), key: "gpu_temp", level: level(avg(|g| g.temp_c), 80.0, 90.0) },
    ]
}

fn overview(f: &mut Frame, app: &App, cfg: &Config, area: Rect) {
    let cols = if area.width >= 100 { 4 } else { 2 };
    let rows = 8 / cols;
    let [grid, gauges, requests] =
        Layout::vertical([Constraint::Length(4 * rows as u16), Constraint::Length(1), Constraint::Min(2)]).areas(area);
    let row_areas = Layout::vertical(vec![Constraint::Length(4); rows]).split(grid);
    let cards = cards(app);
    for (r, row) in row_areas.iter().enumerate() {
        let col_areas = Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols]).split(*row);
        for (c, cell) in col_areas.iter().enumerate() {
            card(f, app, cfg, &cards[r * cols + c], *cell);
        }
    }
    let sys = &app.snapshot.system;
    let cpu = na(sys.cpu_pct, |x| format!("{} {} %", bar(x / 100.0, 12, cfg.ascii), num(x, 0)));
    let ram = match (sys.mem_used, sys.mem_total) {
        (Some(u), Some(t)) if t > 0 => format!("{} {} / {}", bar(u as f64 / t as f64, 12, cfg.ascii), bytes(u), bytes(t)),
        _ => "n/a".into(),
    };
    f.render_widget(Paragraph::new(Line::from(vec!["CPU  ".dim(), cpu.into(), "      RAM  ".dim(), ram.into()])), gauges);
    f.render_widget(
        Paragraph::new(vec![Line::from("Recent requests".bold()), Line::from("start with --proxy to see per-request rows".dim())]),
        requests,
    );
}

fn card(f: &mut Frame, app: &App, cfg: &Config, c: &Card, area: Rect) {
    let border = match c.level {
        Level::Ok => Style::default().fg(Color::Green),
        Level::Warn => Style::default().fg(Color::Yellow),
        Level::Crit => Style::default().fg(Color::Red),
        Level::Unknown => Style::default().dim(),
    };
    let block = Block::bordered().title(format!(" {} ", c.title)).border_style(border);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let points = app.history.window(c.key, width.min(app.window_min as usize * 60 * 2));
    let lines = vec![
        Line::from(vec![c.value.clone().bold(), "  ".into(), c.sub.clone().dim()]),
        Line::from(spark(&points, cfg.ascii)),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn bar(ratio: f64, width: usize, ascii: bool) -> String {
    let filled = (ratio.clamp(0.0, 1.0) * width as f64).round() as usize;
    let (on, off) = if ascii { ('#', '.') } else { ('█', '░') };
    std::iter::repeat_n(on, filled).chain(std::iter::repeat_n(off, width - filled)).collect()
}

fn help_overlay(f: &mut Frame, area: Rect) {
    let text = [
        "1-4      switch tab",
        "+ / -    sampling interval 0.5 / 1 / 2 / 5 / 10 s",
        "[ / ]    sparkline window 1 / 5 / 30 min",
        "Space    freeze screen (collection continues)",
        "r        reset sparklines and tok/s baseline",
        "?        this help",
        "q / Esc  quit / close",
    ];
    let w = 56u16.min(area.width);
    let h = text.len() as u16 + 2;
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(text.join("\n")).block(Block::bordered().title(" Keys ")), popup);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::Config;
    use crate::snapshot::{Gpu, Snapshot, Vllm};
    use crate::ui::app::App;

    fn render(w: u16, h: u16, snap: Snapshot) -> String {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut app = App::default();
        app.ingest(snap);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, &app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn overview_renders_cards_with_values() {
        let snap = Snapshot {
            vllm: Vllm {
                reachable: true,
                generation_tok_s: Some(45.1),
                running: Some(3),
                waiting: Some(0),
                kv_cache_usage: Some(0.61),
                ..Default::default()
            },
            gpu: vec![Gpu {
                name: "M4 Pro".into(),
                utilization: Some(96.0),
                mem_used: Some(10_400_000_000),
                mem_total: Some(24 << 30),
                temp_c: Some(59.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = render(120, 40, snap);
        eprintln!("{out}");
        for needle in ["45.1", "3 run · 0 wait", "61 %", "96 %", "59 °C", "n/a", "process: not found"] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
    }

    #[test]
    fn too_small_terminal_shows_hint() {
        assert!(render(60, 20, Snapshot::default()).contains("80x24"));
    }

    #[test]
    fn keys_change_interval_and_window() {
        let cfg = Config::new(String::new(), false);
        let mut app = App::default();
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.on_key(key('+'), &cfg);
        assert_eq!(cfg.interval().as_millis(), 2000);
        for _ in 0..3 {
            app.on_key(key('-'), &cfg);
        }
        assert_eq!(cfg.interval().as_millis(), 500);
        app.on_key(key(']'), &cfg);
        app.on_key(key(']'), &cfg);
        assert_eq!(app.window_min, 30);
        app.on_key(key(']'), &cfg);
        assert_eq!(app.window_min, 30);
        assert!(!app.on_key(key(' '), &cfg) && app.frozen);
        assert!(app.on_key(key('q'), &cfg));
    }
}
