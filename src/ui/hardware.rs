//! Tab 4: one row per GPU, per-core CPU bars, memory/fans line, disk and network sparklines, top processes.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::app::App;
use super::format::{bar, bytes, na, num};
use super::overview::sparkline;
use crate::config::Config;

const CORE_CELL: u16 = 14; // "c12 ██████ 100" plus a gap
const MAX_CORE_ROWS: u16 = 4;
const MAX_PROCS: usize = 5;

pub fn draw(f: &mut Frame, app: &App, cfg: &Config, area: Rect) {
    let s = &app.snapshot;
    let mut rest = area;
    // sections are carved top-down with fixed heights; whatever does not fit is simply not drawn
    let gpu_h = s.gpu.len().max(1) as u16 + 1;
    f.render_widget(Paragraph::new(gpu_lines(app)), carve(&mut rest, gpu_h));

    let per_row = (area.width / CORE_CELL).max(1) as usize;
    let core_rows = (s.system.cpu_cores.len().div_ceil(per_row) as u16).min(MAX_CORE_ROWS);
    f.render_widget(Paragraph::new(cpu_lines(app, cfg, per_row, core_rows as usize)), carve(&mut rest, 1 + core_rows));

    f.render_widget(Paragraph::new(memory_line(app)), carve(&mut rest, 1));

    let io = carve(&mut rest, 3);
    let [disk, net] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(io);
    io_block(f, app, cfg, disk, "Disk", "R", s.system.disk_read_bps, "W", s.system.disk_write_bps, "disk_r");
    io_block(f, app, cfg, net, "Net", "↓", s.system.net_rx_bps, "↑", s.system.net_tx_bps, "net_rx");

    let procs = carve(&mut rest, 1 + s.top_processes.len().min(MAX_PROCS) as u16);
    f.render_widget(Paragraph::new(process_lines(app)), procs);
}

/// Takes `h` rows off the top of `rest` (fewer when it runs out).
fn carve(rest: &mut Rect, h: u16) -> Rect {
    let h = h.min(rest.height);
    let top = Rect { height: h, ..*rest };
    rest.y += h;
    rest.height -= h;
    top
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn gpu_lines(app: &App) -> Vec<Line<'static>> {
    let gpus = &app.snapshot.gpu;
    if gpus.is_empty() {
        return vec![Line::from("GPU".bold()), Line::from("no GPU".dim())];
    }
    let mut lines = vec![Line::from(format!("{:<24} {:>5}  {:<19} {:<15} {:>6}  {:>9}", "GPU", "UTIL", "MEM", "POWER", "TEMP", "CLOCK").dim())];
    for g in gpus {
        let mem = match (g.mem_used, g.mem_total) {
            (Some(u), Some(t)) => format!("{} / {}", bytes(u), bytes(t)),
            (Some(u), None) => bytes(u),
            _ => "n/a".into(),
        };
        let power = match (g.power_w, g.power_limit_w) {
            (Some(p), Some(l)) => format!("{} / {} W", num(p, 1), num(l, 0)),
            (Some(p), None) => format!("{} W", num(p, 1)),
            _ => "n/a".into(),
        };
        lines.push(Line::from(format!(
            "{:<24} {:>5}  {:<19} {:<15} {:>6}  {:>9}",
            truncate(&g.name, 24),
            na(g.utilization, |u| format!("{} %", num(u, 0))),
            mem,
            power,
            na(g.temp_c, |t| format!("{} °C", num(t, 0))),
            na(g.freq_mhz, |f| format!("{} MHz", num(f as f64, 0))),
        )));
    }
    lines
}

fn cpu_lines(app: &App, cfg: &Config, per_row: usize, rows: usize) -> Vec<Line<'static>> {
    let s = &app.snapshot.system;
    let mut head = vec!["CPU".bold(), format!("  {}", na(s.cpu_pct, |x| format!("{} %", num(x, 0)))).into()];
    if let Some(w) = s.cpu_power_w {
        head.push(format!("  ·  cpu {} W", num(w, 1)).dim());
    }
    if let Some(w) = s.ane_power_w {
        head.push(format!("  ·  ane {} W", num(w, 1)).dim());
    }
    let mut lines = vec![Line::from(head)];
    for chunk in s.cpu_cores.chunks(per_row).take(rows) {
        let row_start = lines.len() - 1;
        let cells: Vec<String> = chunk
            .iter()
            .enumerate()
            .map(|(i, pct)| format!("{:<3}{} {:>3}", format!("c{}", row_start * per_row + i), bar(pct / 100.0, 6, cfg.ascii), num(*pct, 0)))
            .collect();
        lines.push(Line::from(cells.join(" ")));
    }
    lines
}

fn memory_line(app: &App) -> Line<'static> {
    let s = &app.snapshot.system;
    let ram = match (s.mem_used, s.mem_total) {
        (Some(u), Some(t)) => format!("{} / {}", bytes(u), bytes(t)),
        _ => "n/a".into(),
    };
    let mut spans = vec!["RAM".bold(), format!("  {ram}").into(), format!("  ·  swap {}", na(s.swap_used, bytes)).dim()];
    if !s.fans.is_empty() {
        let fans: Vec<String> = s.fans.iter().map(|(n, rpm)| format!("{n} {} rpm", num(*rpm as f64, 0))).collect();
        spans.push(format!("  ·  fans {}", fans.join(", ")).dim());
    }
    Line::from(spans)
}

fn rate(v: Option<f64>) -> String {
    na(v, |x| format!("{}/s", bytes(x as u64)))
}

#[allow(clippy::too_many_arguments)]
fn io_block(f: &mut Frame, app: &App, cfg: &Config, area: Rect, title: &'static str, a: &'static str, av: Option<f64>, b: &'static str, bv: Option<f64>, key: &str) {
    if area.height == 0 {
        return;
    }
    let [text, graph] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(vec![title.bold(), format!("  {a} {}  {b} {}", rate(av), rate(bv)).into()])), text);
    if graph.height > 0 {
        f.render_widget(sparkline(app, cfg, key, graph.width), graph);
    }
}

fn process_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!("{:<7} {:<20} {:>6} {:>10} {:>10}", "PID", "NAME", "CPU %", "RSS", "GPU MEM").dim())];
    for p in app.snapshot.top_processes.iter().take(MAX_PROCS) {
        lines.push(Line::from(format!(
            "{:<7} {:<20} {:>6} {:>10} {:>10}",
            p.pid,
            truncate(&p.name, 20),
            num(p.cpu_pct, 0),
            bytes(p.rss),
            na(p.gpu_mem, bytes)
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::app::App;
    use crate::config::Config;
    use crate::snapshot::{Gpu, Process, Snapshot, System};

    fn render(w: u16, h: u16, snap: Snapshot) -> String {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut app = App { tab: 3, ..App::default() };
        app.ingest(snap);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::super::overview::draw(f, &app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    fn gpu(util: f64) -> Gpu {
        Gpu {
            name: "A100".into(),
            utilization: Some(util),
            mem_used: Some(10_400_000_000),
            mem_total: Some(40 << 30),
            power_w: Some(250.0),
            power_limit_w: Some(300.0),
            temp_c: Some(59.0),
            freq_mhz: Some(1980),
        }
    }

    fn proc(name: &str, cpu: f64) -> Process {
        Process { pid: 4242, name: name.into(), uptime_s: 10, cpu_pct: cpu, rss: 1 << 30, threads: None, gpu_mem: None }
    }

    fn snap() -> Snapshot {
        let mut cores = vec![10.0; 8];
        cores[3] = 62.0;
        Snapshot {
            gpu: vec![gpu(96.0), gpu(40.0)],
            system: System {
                cpu_pct: Some(23.0),
                cpu_cores: cores,
                cpu_power_w: Some(3.1),
                mem_used: Some(17_079_123_968),
                mem_total: Some(24 << 30),
                swap_used: Some(0),
                disk_read_bps: Some(12.3 * 1024.0 * 1024.0),
                net_rx_bps: Some(1.2 * 1024.0 * 1024.0),
                fans: vec![("Fan1".into(), 1200)],
                ..Default::default()
            },
            top_processes: vec![proc("python", 180.0), proc("vtop", 2.0)],
            ..Default::default()
        }
    }

    #[test]
    fn hardware_renders_gpu_rows_cores_and_processes() {
        let out = render(120, 40, snap());
        assert_eq!(out.matches("A100").count(), 2, "{out}");
        for needle in ["96 %", "250.0 / 300 W", "1,980 MHz", "59 °C", "c3", "62", "cpu 3.1 W", "Fan1 1,200 rpm", "12.3 MB/s", "1.2 MB/s", "python", "180", "n/a"] {
            assert!(out.contains(needle), "missing {needle:?}\n{out}");
        }
    }

    #[test]
    fn hardware_without_gpu() {
        let out = render(120, 40, Snapshot { gpu: Vec::new(), ..snap() });
        assert!(out.contains("no GPU"), "{out}");
    }

    #[test]
    fn hardware_fits_many_cores_in_small_area() {
        let mut s = snap();
        s.system.cpu_cores = vec![50.0; 96];
        let out = render(100, 12, s);
        assert_eq!(out.lines().count(), 12);
        assert!(out.contains("c0"), "{out}");
    }

    #[test]
    fn process_names_are_truncated() {
        let long = "a".repeat(40);
        let mut s = snap();
        s.top_processes = vec![proc(&long, 1.0)];
        let out = render(120, 40, s);
        assert!(out.contains(&"a".repeat(20)) && !out.contains(&long), "{out}");
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for (w, h) in [(20, 5), (40, 8), (60, 12)] {
            render(w, h, snap());
        }
    }
}
