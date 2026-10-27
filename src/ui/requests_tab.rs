//! Tab 3: every recorded request, a cursor, a filter line and the detail of the selected row.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::app::App;
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
    let ms = |v: Option<f64>| na(v, |t| format!("{} ms", num(t, 0)));
    vec![
        Line::from(format!("#{}  request {}  max_tokens {}", r.id, r.request_id, na(r.max_tokens, |t| t.to_string()))),
        Line::from(format!(
            "prompt {}  output {}  tok/s {}",
            na(r.prompt_tokens, |t| t.to_string()),
            na(r.output_tokens, |t| t.to_string()),
            na(r.tok_s(), |t| num(t, 1))
        )),
        Line::from(format!(
            "ttft {}  queue {}  prefill {}  decode {}  e2e {}",
            ms(r.ttft_ms), ms(r.queue_ms), ms(r.prefill_ms), ms(r.decode_ms), ms(r.e2e_ms)
        )),
    ]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::app::App;
    use crate::config::Config;
    use crate::requests::sample;

    fn render(w: u16, h: u16, app: &App) -> String {
        let cfg = Config::new("http://127.0.0.1:8000".into(), false);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::super::overview::draw(f, app, &cfg)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn requests_tab_renders_detail_of_selected() {
        let mut first = sample("chatcmpl-first0000", 261);
        first.queue_ms = Some(12.0);
        first.prefill_ms = Some(400.0);
        first.decode_ms = Some(2000.0);
        let second = sample("chatcmpl-second000", 5);
        let mut app = App { tab: 2, requests: vec![first, second], ..App::default() };
        let out = render(120, 30, &app);
        assert!(out.contains("2 requests"), "{out}");
        assert!(out.contains("request chatcmpl-first0000") && out.contains("queue 12 ms") && out.contains("tok/s 130.5"), "{out}");
        app.selected = 1;
        let out = render(120, 30, &app);
        assert!(out.contains("request chatcmpl-second000") && !out.contains("queue 12 ms"), "{out}");
    }
}
