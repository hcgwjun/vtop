//! The per-request table shared by the Overview footer and the Requests tab.

use ratatui::style::Stylize;
use ratatui::text::Line;

use super::format::{na, num};
use crate::requests::{Record, hhmmss_local};

/// Header line followed by at most `max_rows` rows, newest first. QUEUE and DECODE need `width >= 100`.
pub fn request_rows(records: &[Record], width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let wide = width >= 100;
    let mid = |q: String, d: String| if wide { format!(" {q:>8} {d:>8}") } else { String::new() };
    let header = format!(
        "{:<8} {:<16} {:>7} {:>7} {:>8}{} {:>6} {:>7}",
        "TIME", "ID", "PROMPT", "OUTPUT", "TTFT ms", mid("QUEUE ms".into(), "DECODE s".into()), "TOK/S", "E2E s"
    );
    let mut lines = vec![Line::from(header.dim())];
    for r in records.iter().take(max_rows) {
        let row = format!(
            "{:<8} {:<16} {:>7} {:>7} {:>8}{} {:>6} {:>7}",
            hhmmss_local(r.started_unix_ms),
            short_id(&r.request_id),
            na(r.prompt_tokens, |t| t.to_string()),
            na(r.output_tokens, |t| t.to_string()),
            na(r.ttft_ms, |t| num(t, 0)),
            mid(na(r.queue_ms, |t| num(t, 0)), na(r.decode_ms, |t| num(t / 1000.0, 1))),
            na(r.tok_s(), |t| num(t, 1)),
            na(r.e2e_ms, |t| num(t / 1000.0, 1)),
        );
        lines.push(Line::from(row));
    }
    lines
}

/// "chatcmpl-92e9abad140f8574" → "92e9abad140f8574": vLLM's prefix only says which endpoint was used.
fn short_id(id: &str) -> String {
    let id = id.strip_prefix("chatcmpl-").or_else(|| id.strip_prefix("cmpl-")).unwrap_or(id);
    id.chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::sample;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn columns_line_up_and_missing_values_are_na() {
        let mut r = sample("chatcmpl-92e9abad140f8574", 302);
        r.ttft_ms = Some(420.0);
        r.decode_ms = Some(1656.0);
        let mut missing = sample("cmpl-x", 1);
        missing.output_tokens = None;
        let rows = request_rows(&[r, missing], 120, 5);
        let (header, row, row2) = (text(&rows[0]), text(&rows[1]), text(&rows[2]));
        assert_eq!(header.find("ID"), row.find("92e9abad"), "id column\n{header}\n{row}");
        assert!(row.contains("182.4") && row.contains("420"), "{row}");
        assert!(row2.contains("n/a"), "{row2}");
        assert!(header.contains("QUEUE") && header.contains("DECODE"));
        assert!(!text(&request_rows(&[], 80, 5)[0]).contains("QUEUE"), "narrow drops queue/decode");
    }

    #[test]
    fn short_id_strips_only_known_prefixes() {
        assert_eq!(short_id("chatcmpl-92e9abad140f8574"), "92e9abad140f8574");
        assert_eq!(short_id("cmpl-abc"), "abc");
        assert_eq!(short_id("1946fac8-d423-4c99-889b-6f0e"), "1946fac8-d423-4c");
    }
}
