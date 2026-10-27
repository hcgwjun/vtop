//! Reads SSE `data:` lines as they stream past to count content chunks and pick up `usage`; bytes are never altered.

use serde_json::Value;

#[derive(Default)]
pub struct SsePeek {
    buf: Vec<u8>,
    /// Chunks whose `choices[0].delta.content` is a non-empty string; stands in for output tokens when `usage` is absent.
    pub chunks: u64,
    /// `(prompt_tokens, completion_tokens)` from the last chunk that carried `usage`.
    pub usage: Option<(u64, u64)>,
}

impl SsePeek {
    /// Consumes complete lines; a partial trailing line waits in the buffer for the next call.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            self.line(&line);
        }
    }

    pub fn finish(&mut self) {
        let rest = std::mem::take(&mut self.buf);
        self.line(&rest);
    }

    fn line(&mut self, line: &[u8]) {
        let Some(data) = line.strip_prefix(b"data:") else { return };
        let data = data.trim_ascii();
        if data == b"[DONE]" {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(data) else { return };
        if let Some(u) = usage_of(&v) {
            self.usage = Some(u);
        }
        // one chunk ≈ one token whichever field carries it: chat content/reasoning/tool calls or completions text
        let has_text = |x: &Value| x.as_str().is_some_and(|s| !s.is_empty()) || x.as_array().is_some_and(|a| !a.is_empty());
        let choices = v["choices"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if choices.iter().any(|c| {
            has_text(&c["text"]) || ["content", "reasoning_content", "tool_calls"].iter().any(|k| has_text(&c["delta"][k]))
        }) {
            self.chunks += 1;
        }
    }
}

/// `usage` of a non-streaming completion body.
pub fn parse_usage(body: &[u8]) -> Option<(u64, u64)> {
    usage_of(&serde_json::from_slice(body).ok()?)
}

fn usage_of(v: &Value) -> Option<(u64, u64)> {
    let u = v.get("usage")?.as_object()?;
    Some((u.get("prompt_tokens")?.as_u64()?, u.get("completion_tokens")?.as_u64()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(s: &str) -> String {
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{s}\"}}}}]}}\n\n")
    }

    #[test]
    fn sse_lines_split_across_chunks() {
        let mut p = SsePeek::default();
        p.feed(b"da");
        p.feed(b"ta: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\nda");
        p.feed(b"ta: [DONE]\n");
        p.finish();
        assert_eq!(p.chunks, 1);
    }

    #[test]
    fn stream_without_usage_is_approximate() {
        let mut p = SsePeek::default();
        for s in ["a", "b", "c"] {
            p.feed(content(s).as_bytes());
        }
        p.finish();
        assert_eq!(p.chunks, 3);
        assert_eq!(p.usage, None);
    }

    #[test]
    fn usage_chunk_wins() {
        let mut p = SsePeek::default();
        p.feed(content("a").as_bytes());
        p.feed(content("b").as_bytes());
        p.feed(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":7}}\n\n");
        p.finish();
        assert_eq!(p.chunks, 2);
        assert_eq!(p.usage, Some((20, 7)));
    }

    #[test]
    fn counts_completions_text_reasoning_and_tool_calls() {
        let mut p = SsePeek::default();
        p.feed(b"data: {\"choices\":[{\"text\":\"a\"}]}\n");
        p.feed(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n");
        p.feed(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0}]}}]}\n");
        p.feed(b"data: {\"choices\":[{\"index\":0,\"delta\":{}},{\"index\":1,\"delta\":{\"content\":\"b\"}}]}\n");
        p.feed(b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n");
        p.finish();
        assert_eq!(p.chunks, 4);
    }

    #[test]
    fn parse_usage_non_stream() {
        let body = br#"{"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}}"#;
        assert_eq!(parse_usage(body), Some((20, 2)));
        assert_eq!(parse_usage(b"{}"), None);
    }
}
