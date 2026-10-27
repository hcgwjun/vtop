//! Just enough of the OTLP trace protobuf to read span names, times and attributes; unknown fields are skipped by prost.
//! Tags follow opentelemetry/proto/{collector/trace/v1/trace_service, trace/v1/trace, common/v1/common}.proto.

use prost::Message;

#[derive(Clone, PartialEq, Message)]
struct ExportTraceServiceRequest {
    #[prost(message, repeated, tag = "1")]
    resource_spans: Vec<ResourceSpans>,
}

#[derive(Clone, PartialEq, Message)]
struct ResourceSpans {
    #[prost(message, repeated, tag = "2")]
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Clone, PartialEq, Message)]
struct ScopeSpans {
    #[prost(message, repeated, tag = "2")]
    spans: Vec<Span>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Span {
    #[prost(string, tag = "5")]
    pub name: String,
    #[prost(fixed64, tag = "7")]
    pub start_time_unix_nano: u64,
    #[prost(fixed64, tag = "8")]
    pub end_time_unix_nano: u64,
    #[prost(message, repeated, tag = "9")]
    pub attributes: Vec<KeyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyValue {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(message, optional, tag = "2")]
    pub value: Option<AnyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AnyValue {
    #[prost(string, optional, tag = "1")]
    pub string_value: Option<String>,
    #[prost(bool, optional, tag = "2")]
    pub bool_value: Option<bool>,
    #[prost(int64, optional, tag = "3")]
    pub int_value: Option<i64>,
    #[prost(double, optional, tag = "4")]
    pub double_value: Option<f64>,
}

impl Span {
    fn attr(&self, key: &str) -> Option<&AnyValue> {
        self.attributes.iter().find(|kv| kv.key == key)?.value.as_ref()
    }

    /// Doubles and ints both count: vLLM sends token counts as ints and latencies as doubles.
    pub fn f64(&self, key: &str) -> Option<f64> {
        let v = self.attr(key)?;
        v.double_value.or_else(|| v.int_value.map(|i| i as f64))
    }

    pub fn u64(&self, key: &str) -> Option<u64> {
        let v = self.attr(key)?;
        v.int_value.and_then(|i| u64::try_from(i).ok()).or_else(|| v.double_value.map(|d| d as u64))
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.attr(key)?.string_value.as_deref()
    }
}

/// Every span in an OTLP export request.
pub fn parse_spans(body: &[u8]) -> Result<Vec<Span>, prost::DecodeError> {
    ExportTraceServiceRequest::decode(body)
        .map(|r| r.resource_spans.into_iter().flat_map(|rs| rs.scope_spans).flat_map(|ss| ss.spans).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/otlp_llm_request.bin");

    #[test]
    fn fixture_has_one_llm_request_span() {
        let spans = parse_spans(FIXTURE).unwrap();
        assert_eq!(spans.len(), 1);
        let s = &spans[0];
        assert_eq!(s.name, "llm_request");
        assert_eq!(s.u64("gen_ai.usage.prompt_tokens"), Some(419));
        assert_eq!(s.u64("gen_ai.usage.completion_tokens"), Some(266));
        let ttft = s.f64("gen_ai.latency.time_to_first_token").unwrap();
        assert!((0.3..3.0).contains(&ttft), "{ttft}");
        assert!(s.str("gen_ai.request.id").unwrap().starts_with("chatcmpl-"));
        assert!(s.start_time_unix_nano < s.end_time_unix_nano);
    }

    #[test]
    fn unknown_fields_and_spans_are_skipped() {
        assert!(parse_spans(b"\x00\x01garbage").is_err());
        assert_eq!(parse_spans(FIXTURE).unwrap()[0].f64("nope"), None);
    }
}
