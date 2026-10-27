//! Tab 3: every recorded request, a cursor, a filter line and the detail of the selected row.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::app::{App, status_text};
use super::format::{na, num};
use super::table::request_rows;
use crate::requests::Record;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let [top, list, detail] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(4)]).areas(area);
    let visible = app.visible_requests();
    let cursor = if app.filter_editing { "▏" } else { "" };
    let summary = format!("{} requests · filter: {}{cursor}", visible.len(), app.filter);
    f.render_widget(Paragraph::new(summary), top);
    if !app.proxy_on {
        f.render_widget(Paragraph::new("start with --proxy to record requests".dim()), list);
        return;
    }
    // keep the cursor on screen: scroll the window so `selected` is within the rows that fit
    let rows = list.height.saturating_sub(1) as usize;
    let first = app.selected.saturating_sub(rows.saturating_sub(1));
    let owned: Vec<Record> = visible.iter().skip(first).take(rows).map(|r| (*r).clone()).collect();
    let mut lines = request_rows(&owned, list.width, rows);
    if let Some(line) = lines.get_mut(app.selected - first + 1) {
        *line = std::mem::take(line).reversed();
    }
    f.render_widget(Paragraph::new(lines), list);
    if let Some(r) = visible.get(app.selected) {
        f.render_widget(Paragraph::new(detail_lines(r)), detail);
    }
}

fn detail_lines(r: &Record) -> Vec<Line<'static>> {
    let tokens = format!(
        "prompt {}  output {}{}",
        na(r.prompt_tokens, |t| t.to_string()),
        if r.approx { "~" } else { "" },
        na(r.output_tokens, |t| t.to_string())
    );
    vec![
        Line::from(format!("#{}  {}  model {}  {}", r.id, r.path, r.model.as_deref().unwrap_or("-"), if r.stream { "stream" } else { "non-stream" })),
        Line::from(format!("status {}", status_text(&r.status))),
        Line::from(format!(
            "{tokens}  ttft {} ms  duration {} s  tok/s {}",
            na(r.ttft_ms, |t| num(t, 0)),
            na(r.duration_ms, |d| num(d / 1000.0, 1)),
            na(r.tok_s(), |t| num(t, 1))
        )),
    ]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::app::App;
    use crate::config::Config;
    use crate::requests::{Record, Status};

    fn render(w: u16, h: u16, app: &App) -> String {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::super::overview::draw(f, app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn requests_tab_renders_detail_of_selected() {
        let done = Record {
            id: 1,
            started_unix_ms: 0,
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
        let failed = Record { id: 2, status: Status::Error("connection refused".into()), path: "/v1/models".into(), ..done.clone() };
        let mut app = App { tab: 2, proxy_on: true, requests: vec![failed, done], ..App::default() };
        let out = render(120, 30, &app);
        assert!(out.contains("2 requests"), "{out}");
        assert!(out.contains("connection refused") && out.contains("/v1/models"), "{out}");
        app.selected = 1;
        let out = render(120, 30, &app);
        assert!(out.contains("tok/s 31.5") && out.contains("/v1/chat/completions"), "{out}");
        assert!(!out.contains("connection refused"), "{out}");
    }
}
