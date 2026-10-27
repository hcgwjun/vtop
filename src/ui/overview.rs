//! Overview: header, eight sparkline cards, CPU/RAM gauges, request table slot, key hints.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::symbols::bar;
use ratatui::widgets::{Block, Clear, Paragraph, Sparkline};

use super::app::App;
use super::format::{Level, bytes, downsample, level, na, normalize, num, uptime};
use crate::config::Config;

const TABS: [&str; 4] = ["Overview", "Engine", "Requests", "Hardware"];
const HINTS: &str = "1-4 tabs  +/- interval  [/] window  Space freeze  r reset  ? help  q quit";

pub fn draw(f: &mut Frame, app: &App, cfg: &Config) {
    let area = f.area();
    if area.height < 3 || area.width < 10 {
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
    // status and toggles sit before the tabs so they survive an 80-column terminal
    let mut line1 = vec!["vtop".bold()];
    for (i, name) in TABS.iter().enumerate() {
        let label = format!("{} {}", i + 1, name);
        line1.push(" ".into());
        line1.push(if i as u8 == app.tab { format!("[{label}]").bold() } else { label.dim() });
    }
    line1.push(" ".into());
    line1.push(Span::styled(state, Style::default().fg(color).bold()));
    line1.push(format!(" ⟳{:.1}s {}m", interval.as_secs_f64(), app.window_min).dim());
    if app.frozen {
        line1.push(" FROZEN".yellow().bold());
    }
    let line2 = match &s.process {
        Some(p) => format!(
            "{}  PID {} · up {} · CPU {:.0}% · RSS {}{}",
            p.name, p.pid, uptime(p.uptime_s), p.cpu_pct, bytes(p.rss), sub(p.threads, |t| format!(" · {t} threads"))
        ),
        None => "process: not found".into(),
    };
    let mut line2 = vec![
        s.vllm.model.clone().unwrap_or_else(|| "no model".into()).into(),
        "  ".into(),
        cfg.url.clone().dim(),
        "   ".into(),
        line2.dim(),
    ];
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
            sub: format!(
                "{}{}",
                if cfg!(target_os = "macos") { "unified · " } else { "" },
                sub(avg(|g| g.freq_mhz.map(|f| f as f64)), |x| format!("{} MHz", num(x, 0)))
            ),
            key: "gpu_mem",
            level: match (sum_u(|g| g.mem_used), sum_u(|g| g.mem_total)) {
                (Some(u), Some(t)) if t > 0 => level(Some(u as f64 / t as f64 * 100.0), 80.0, 95.0),
                _ => Level::Unknown,
            },
        },
        Card {
            title: "Power",
            value: na(avg(|g| g.power_w).map(|p| p * gpus.len() as f64), |x| format!("{} W", num(x, 1))),
            sub: power_limit.unwrap_or_else(|| sub(s.system.cpu_power_w, |x| format!("cpu {} W", num(x, 1)))),
            key: "power",
            level: Level::Unknown,
        },
        Card { title: "GPU temp", value: na(avg(|g| g.temp_c), |x| format!("{} °C", num(x, 0))), sub: String::new(), key: "gpu_temp", level: level(avg(|g| g.temp_c), 80.0, 90.0) },
    ]
}

fn overview(f: &mut Frame, app: &App, cfg: &Config, area: Rect) {
    // small terminals degrade instead of refusing: 4 → 2 → 1 columns, cards 6 → 3 rows, then rows are cut
    let cols = if area.width >= 100 { 4 } else if area.width >= 50 { 2 } else { 1 };
    let rows = 8 / cols;
    let card_h = ((area.height.saturating_sub(4)) / rows as u16).clamp(3, 6);
    let grid_h = (card_h * rows as u16).min(area.height);
    let [grid, gauges, requests] =
        Layout::vertical([Constraint::Length(grid_h), Constraint::Max(1), Constraint::Min(0)]).areas(area);
    // only split the rows that fit: the solver would otherwise shrink every row evenly below a usable height
    let fit = ((grid_h / card_h) as usize).min(rows);
    let row_areas = Layout::vertical(vec![Constraint::Length(card_h); fit]).split(grid);
    let cards = cards(app);
    for (r, row) in row_areas.iter().enumerate() {
        let col_areas = Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols]).split(*row);
        for (c, cell) in col_areas.iter().enumerate() {
            card(f, app, cfg, &cards[r * cols + c], *cell);
        }
    }
    if gauges.height == 0 {
        return;
    }
    let sys = &app.snapshot.system;
    let cpu = na(sys.cpu_pct, |x| format!("{} {} %", bar(x / 100.0, 12, cfg.ascii), num(x, 0)));
    let ram = match (sys.mem_used, sys.mem_total) {
        (Some(u), Some(t)) if t > 0 => format!("{} {} / {}", bar(u as f64 / t as f64, 12, cfg.ascii), bytes(u), bytes(t)),
        _ => "n/a".into(),
    };
    f.render_widget(Paragraph::new(Line::from(vec!["CPU  ".dim(), cpu.into(), "      RAM  ".dim(), ram.into()])), gauges);
    let mut lines = vec![Line::from("Recent requests".bold())];
    if app.proxy_on {
        lines.extend(super::table::request_rows(&app.requests, requests.width, requests.height.saturating_sub(2) as usize));
    } else {
        lines.push(Line::from("start with --proxy to see per-request rows".dim()));
    }
    f.render_widget(Paragraph::new(lines), requests);
}

/// History slots covering the selected window: the UI ingests once per `min(interval, 1 s)`.
pub fn samples_for(app: &App, cfg: &Config) -> usize {
    let tick_ms = cfg.interval().as_millis().min(1000).max(1) as usize;
    app.window_min as usize * 60 * 1000 / tick_ms
}

const ASCII_BARS: bar::Set = bar::Set {
    full: "#",
    seven_eighths: "#",
    three_quarters: "*",
    five_eighths: "*",
    half: "+",
    three_eighths: "=",
    one_quarter: "-",
    one_eighth: ".",
    empty: " ",
};

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
    let tall = inner.height >= 4;
    let text_rows = if tall { 2 } else { 1 };
    let [text, graph] = Layout::vertical([Constraint::Length(text_rows), Constraint::Min(0)]).areas(inner);
    let lines = if tall {
        vec![Line::from(c.value.clone().bold()), Line::from(c.sub.clone().dim())]
    } else {
        vec![Line::from(vec![c.value.clone().bold(), "  ".into(), c.sub.clone().dim()])]
    };
    f.render_widget(Paragraph::new(lines), text);
    if graph.height == 0 {
        return;
    }
    let points = normalize(&downsample(&app.history.window(c.key, samples_for(app, cfg)), graph.width as usize));
    let set = if cfg.ascii { ASCII_BARS } else { bar::NINE_LEVELS };
    f.render_widget(Sparkline::default().data(points).max(1000).bar_set(set).absent_value_symbol(" "), graph);
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
        let mut app = App::default();
        app.ingest(snap);
        render_app(w, h, &app)
    }

    fn render_app(w: u16, h: u16, app: &App) -> String {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn overview_shows_recent_requests_when_proxy_on() {
        use crate::requests::{Record, Status};
        let done = Record {
            id: 1,
            started_unix_ms: 55_431_000,
            path: "/v1/chat/completions".into(),
            model: Some("m".into()),
            stream: true,
            status: Status::Done(200),
            prompt_tokens: Some(1200),
            output_tokens: Some(261),
            approx: false,
            ttft_ms: Some(420.0),
            duration_ms: Some(8700.0),
        };
        let in_flight = Record { id: 2, status: Status::InFlight, output_tokens: None, ..done.clone() };
        let mut app = App::default();
        app.proxy_on = true;
        app.requests = vec![in_flight, done];
        let out = render_app(120, 40, &app);
        assert!(out.contains("261") && out.contains("31.5") && out.contains("in flight"), "{out}");
        assert!(!out.contains("start with --proxy"), "{out}");
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
    fn frozen_marker_visible_at_80_columns() {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut app = App::default();
        app.frozen = true;
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| draw(f, &app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        let line1: String = (0..80).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(line1.contains("FROZEN") && line1.contains("1.0s"), "{line1}");
    }

    #[test]
    fn window_key_changes_sample_count() {
        let cfg = Config::new(String::new(), false);
        let mut app = App::default();
        assert_eq!(samples_for(&app, &cfg), 60);
        app.on_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE), &cfg);
        assert_eq!(samples_for(&app, &cfg), 300);
        app.on_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE), &cfg); // 0.5 s interval → 0.5 s ticks
        assert_eq!(samples_for(&app, &cfg), 600);
    }

    #[test]
    fn ctrl_c_quits() {
        let cfg = Config::new(String::new(), false);
        let mut app = App::default();
        assert!(app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), &cfg));
    }

    #[test]
    fn tall_cards_put_subtitle_on_its_own_line() {
        let snap = Snapshot { vllm: Vllm { generation_tok_s: Some(45.1), ..Default::default() }, ..Default::default() };
        let out = render(120, 36, snap);
        let lines: Vec<&str> = out.lines().collect();
        let i = lines.iter().position(|l| l.contains("45.1")).unwrap();
        assert!(lines[i + 1].starts_with("│now"), "{:?}", lines[i + 1]);
        let out = render(80, 24, Snapshot { vllm: Vllm { generation_tok_s: Some(45.1), ..Default::default() }, ..Default::default() });
        assert!(out.lines().any(|l| l.contains("45.1  now")), "compact cards keep value and subtitle on one line");
    }

    #[test]
    fn small_terminals_still_render_cards() {
        let snap = || Snapshot { vllm: Vllm { generation_tok_s: Some(45.1), ..Default::default() }, ..Default::default() };
        assert!(render(60, 20, snap()).contains("45.1"), "two columns at 60 wide");
        assert!(render(40, 12, snap()).contains("45.1"), "one column at 40 wide");
        for (w, h) in [(20, 5), (10, 3), (5, 2), (120, 1)] {
            render(w, h, snap()); // must not panic
        }
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
