//! `POST /v1/traces` receiver for vLLM's `--otlp-traces-endpoint` (OTLP/HTTP, protobuf).

use std::thread::JoinHandle;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::post;

use super::proto::parse_spans;
use crate::requests::{Record, SharedLog};

pub fn router(log: SharedLog) -> Router {
    Router::new().route("/v1/traces", post(traces)).layer(DefaultBodyLimit::max(16 << 20)).with_state(log)
}

/// 400 on a body that is not protobuf (OTLP says so, and vLLM then logs the failure instead of silently
/// exporting into the void, e.g. with http/json or gzip set); otherwise 200 with an empty response message.
async fn traces(State(log): State<SharedLog>, body: Bytes) -> Response {
    let Ok(spans) = parse_spans(&body) else {
        return (StatusCode::BAD_REQUEST, "vtop: body is not an OTLP protobuf ExportTraceServiceRequest").into_response();
    };
    let records: Vec<Record> = spans.iter().filter_map(Record::from_span).collect();
    let mut log = log.lock().unwrap();
    for r in records {
        log.push(r);
    }
    ([(CONTENT_TYPE, "application/x-protobuf")], Bytes::new()).into_response()
}

/// Binds now (so a taken port fails before the TUI starts) and serves on its own runtime thread.
pub fn spawn(bind: &str, log: SharedLog) -> anyhow::Result<JoinHandle<()>> {
    let listener =
        std::net::TcpListener::bind(bind).map_err(|e| anyhow::anyhow!("cannot bind otlp receiver on {bind}: {e}"))?;
    listener.set_nonblocking(true)?;
    let handle = std::thread::Builder::new().name("otlp".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("tokio runtime");
        rt.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            if let Err(e) = axum::serve(listener, router(log)).await {
                eprintln!("vtop: otlp receiver stopped: {e}");
            }
        });
    })?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/otlp_llm_request.bin");

    async fn post(log: &SharedLog, body: Vec<u8>) -> axum::response::Response {
        let req = Request::post("/v1/traces").header("content-type", "application/x-protobuf").body(Body::from(body)).unwrap();
        router(log.clone()).oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn fixture_post_records_one_row() {
        let log = SharedLog::default();
        let resp = post(&log, FIXTURE.to_vec()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "application/x-protobuf");
        let rows = log.lock().unwrap().latest(10);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].request_id.starts_with("chatcmpl-"));
    }

    #[tokio::test]
    async fn batch_of_two_spans_gives_two_rows() {
        let log = SharedLog::default();
        post(&log, [FIXTURE, FIXTURE].concat()).await;
        assert_eq!(log.lock().unwrap().latest(10).len(), 2);
    }

    #[tokio::test]
    async fn garbage_body_gets_400_without_rows() {
        let log = SharedLog::default();
        assert_eq!(post(&log, b"nope".to_vec()).await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(log.lock().unwrap().latest(10).len(), 0);
    }

    #[tokio::test]
    async fn empty_valid_body_is_200() {
        assert_eq!(post(&SharedLog::default(), Vec::new()).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn other_paths_are_404() {
        let req = Request::get("/v1/models").body(Body::empty()).unwrap();
        let resp = router(SharedLog::default()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
