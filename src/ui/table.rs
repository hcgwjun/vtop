//! The per-request table shared by the Overview footer and the Requests tab.

use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};

use super::format::{na, num};
use crate::requests::{Record, Status};

const MODEL_W: usize = 30;

/// Header line followed by at most `max_rows` rows, newest first. The MODEL column needs `width >= 100`.
pub fn request_rows(records: &[Record], width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let model_col = width >= 100;
    let model = |m: &str| if model_col { format!(" {:<MODEL_W$}", m.chars().take(MODEL_W).collect::<String>()) } else { String::new() };
    let header = format!(
        "{:<8} {:<9}{} {:>7} {:>7} {:>8} {:>6} {:>7}",
        "TIME(UTC)", "STATUS", model("MODEL"), "PROMPT", "OUTPUT", "TTFT ms", "TOK/S", "TIME"
    );
    let mut lines = vec![Line::from(header.dim())];
    for r in records.iter().take(max_rows) {
        let (status, color) = match &r.status {
            Status::InFlight => ("in flight".to_string(), Color::Yellow),
            Status::Done(c) if (200..300).contains(c) => (c.to_string(), Color::Green),
            Status::Done(c) => (c.to_string(), Color::Red),
            Status::Error(_) => ("error".to_string(), Color::Red),
        };
        let output = na(r.output_tokens, |t| format!("{}{}", if r.approx { "~" } else { "" }, t));
        let rest = format!(
            "{} {:>7} {:>7} {:>8} {:>6} {:>7}",
            model(r.model.as_deref().unwrap_or("-")),
            na(r.prompt_tokens, |t| t.to_string()),
            output,
            na(r.ttft_ms, |t| num(t, 0)),
            na(r.tok_s(), |t| num(t, 1)),
            na(r.duration_ms, |d| format!("{} s", num(d / 1000.0, 1))),
        );
        lines.push(Line::from(vec![
            Span::raw(format!("{:<8} ", crate::requests::hhmmss_utc(r.started_unix_ms))),
            Span::styled(format!("{status:<9}"), color),
            Span::raw(rest),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::{Record, Status};

    #[test]
    fn approx_marks_output_with_tilde() {
        let r = Record {
            id: 1,
            started_unix_ms: 0,
            path: "/v1/chat/completions".into(),
            model: None,
            stream: true,
            status: Status::Done(200),
            prompt_tokens: None,
            output_tokens: Some(42),
            approx: true,
            ttft_ms: Some(100.0),
            duration_ms: Some(1100.0),
        };
        let rows = request_rows(&[r], 120, 5);
        let text: String = rows[1].spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("~42"), "{text}");
        assert!(text.contains("n/a"), "{text}");
    }
}
