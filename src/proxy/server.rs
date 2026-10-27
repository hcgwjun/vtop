//! axum pass-through to the vLLM server: same method, path, headers and body both ways; one record per request.

use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{CONNECTION, CONTENT_LENGTH, HOST, TRANSFER_ENCODING};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use futures_util::Stream;

use super::sse::{SsePeek, parse_usage};
use crate::requests::{Record, SharedLog, Status};

/// Large enough for image requests (paddlex OCR sends base64 pages).
const MAX_BODY: usize = 64 << 20;

#[derive(Clone)]
struct Ctx {
    upstream: String,
    log: SharedLog,
}

pub fn router(upstream: String, log: SharedLog) -> Router {
    let upstream = upstream.trim_end_matches('/').to_string();
    Router::new().fallback(any(handler)).with_state(Ctx { upstream, log })
}

/// Binds now (so a bad address fails before the TUI starts) and serves on its own runtime thread.
pub fn spawn(bind: &str, upstream: String, log: SharedLog) -> anyhow::Result<JoinHandle<()>> {
    let listener = std::net::TcpListener::bind(bind).map_err(|e| anyhow::anyhow!("cannot bind proxy on {bind}: {e}"))?;
    listener.set_nonblocking(true)?;
    let handle = std::thread::Builder::new().name("proxy".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
        rt.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            if let Err(e) = axum::serve(listener, router(upstream, log)).await {
                eprintln!("vtop: proxy stopped: {e}");
            }
        });
    })?;
    Ok(handle)
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(60)) // per read, so long streams survive
            .build()
            .expect("reqwest client")
    })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// reqwest's Display hides the cause ("error sending request"); join the chain so the row says "connection refused".
fn describe(e: &reqwest::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur: &dyn std::error::Error = e;
    while let Some(src) = cur.source() {
        parts.push(src.to_string());
        cur = src;
    }
    parts.join(": ")
}

fn strip(h: &mut HeaderMap) {
    for name in [HOST, CONTENT_LENGTH, TRANSFER_ENCODING, CONNECTION] {
        h.remove(name);
    }
}

async fn handler(State(ctx): State<Ctx>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::PAYLOAD_TOO_LARGE, format!("vtop: request body: {e}")).into_response(),
    };
    let json: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
    let model = json.as_ref().and_then(|v| v["model"].as_str()).map(str::to_string);
    let stream = json.as_ref().and_then(|v| v["stream"].as_bool()).unwrap_or(false);
    let started = Instant::now();
    let id = ctx.log.lock().unwrap().push(Record {
        id: 0,
        started_unix_ms: now_ms(),
        path: parts.uri.path().to_string(),
        model,
        stream,
        status: Status::InFlight,
        prompt_tokens: None,
        output_tokens: None,
        approx: false,
        ttft_ms: None,
        duration_ms: None,
    });

    let url = format!("{}{}", ctx.upstream, parts.uri.path_and_query().map_or("/", |p| p.as_str()));
    let mut headers = parts.headers;
    strip(&mut headers);
    let resp = match client().request(parts.method, &url).headers(headers).body(body).send().await {
        Ok(r) => r,
        Err(e) => {
            let msg = describe(&e);
            ctx.log.lock().unwrap().update(id, |r| {
                r.status = Status::Error(msg.clone());
                r.duration_ms = Some(ms_since(started));
            });
            return (StatusCode::BAD_GATEWAY, format!("vtop: upstream unreachable: {msg}")).into_response();
        }
    };
    let status = resp.status();
    let mut headers = resp.headers().clone();
    headers.remove(TRANSFER_ENCODING);
    headers.remove(CONNECTION);

    if stream {
        let tap = Tap { inner: resp.bytes_stream(), peek: SsePeek::default(), started, ttft_ms: None, id, status: status.as_u16(), log: ctx.log, outcome: None };
        return (status, headers, Body::from_stream(tap)).into_response();
    }
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            let msg = describe(&e);
            ctx.log.lock().unwrap().update(id, |r| {
                r.status = Status::Error(msg.clone());
                r.duration_ms = Some(ms_since(started));
            });
            return (StatusCode::BAD_GATEWAY, format!("vtop: upstream unreachable: {msg}")).into_response();
        }
    };
    let ms = ms_since(started);
    let usage = parse_usage(&bytes);
    ctx.log.lock().unwrap().update(id, |r| {
        r.status = Status::Done(status.as_u16());
        r.prompt_tokens = usage.map(|u| u.0);
        r.output_tokens = usage.map(|u| u.1);
        r.ttft_ms = Some(ms);
        r.duration_ms = Some(ms);
    });
    (status, headers, bytes).into_response()
}

/// Passes upstream chunks through untouched while peeking at them; the record is finalized on drop,
/// which also covers a client that disconnects mid-stream.
struct Tap<S> {
    inner: S,
    peek: SsePeek,
    started: Instant,
    ttft_ms: Option<f64>,
    id: u64,
    status: u16,
    log: SharedLog,
    outcome: Option<Status>,
}

impl<S: Stream<Item = reqwest::Result<Bytes>> + Unpin> Stream for Tap<S> {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let r = Pin::new(&mut self.inner).poll_next(cx);
        match &r {
            Poll::Ready(Some(Ok(b))) => {
                if self.ttft_ms.is_none() {
                    self.ttft_ms = Some(ms_since(self.started));
                }
                self.peek.feed(b);
            }
            Poll::Ready(Some(Err(e))) => self.outcome = Some(Status::Error(describe(e))),
            Poll::Ready(None) => self.outcome = Some(Status::Done(self.status)),
            Poll::Pending => {}
        }
        r
    }
}

impl<S> Drop for Tap<S> {
    fn drop(&mut self) {
        self.peek.finish();
        let status = self.outcome.take().unwrap_or_else(|| Status::Error("client disconnected".into()));
        let usage = self.peek.usage;
        let chunks = self.peek.chunks;
        let (ttft, duration) = (self.ttft_ms, ms_since(self.started));
        self.log.lock().unwrap().update(self.id, |r| {
            r.status = status;
            r.prompt_tokens = usage.map(|u| u.0);
            r.output_tokens = Some(usage.map_or(chunks, |u| u.1));
            r.approx = usage.is_none();
            r.ttft_ms = ttft;
            r.duration_ms = Some(duration);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use futures_util::stream;
    use std::time::Duration;

    const SSE: &[&str] = &[
        "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"c\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":7}}\n\n",
        "data: [DONE]\n\n",
    ];
    const JSON: &str = r#"{"choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}}"#;

    async fn serve(router: Router) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
        format!("http://{addr}")
    }

    async fn upstream() -> String {
        let completions = post(|body: Bytes| async move {
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            if v["stream"] == true {
                let chunks = SSE.iter().map(|s| Ok::<_, std::io::Error>(Bytes::from_static(s.as_bytes())));
                ([("content-type", "text/event-stream")], Body::from_stream(stream::iter(chunks))).into_response()
            } else {
                ([("content-type", "application/json")], JSON).into_response()
            }
        });
        let len = post(|body: Bytes| async move { body.len().to_string() });
        serve(Router::new().route("/v1/chat/completions", completions).route("/len", len)).await
    }

    async fn settled(log: &SharedLog) -> Record {
        for _ in 0..200 {
            let r = log.lock().unwrap().latest(1).remove(0);
            if !matches!(r.status, Status::InFlight) {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("record still in flight");
    }

    #[tokio::test]
    async fn stream_passes_through_and_records_usage() {
        let log = SharedLog::default();
        let proxy = serve(router(upstream().await, log.clone())).await;
        let resp = reqwest::Client::new()
            .post(format!("{proxy}/v1/chat/completions"))
            .json(&serde_json::json!({"model": "m", "stream": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        assert_eq!(resp.bytes().await.unwrap(), SSE.concat().as_bytes());
        let r = settled(&log).await;
        assert!(matches!(r.status, Status::Done(200)), "{:?}", r.status);
        assert_eq!(r.model.as_deref(), Some("m"));
        assert!(r.stream);
        assert_eq!((r.prompt_tokens, r.output_tokens, r.approx), (Some(20), Some(7), false));
        assert!(r.ttft_ms.is_some() && r.duration_ms.is_some());
    }

    #[tokio::test]
    async fn non_stream_records_usage_and_body_unchanged() {
        let log = SharedLog::default();
        let proxy = serve(router(upstream().await, log.clone())).await;
        let resp = reqwest::Client::new()
            .post(format!("{proxy}/v1/chat/completions"))
            .json(&serde_json::json!({"model": "m"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), JSON);
        let r = settled(&log).await;
        assert!(matches!(r.status, Status::Done(200)));
        assert_eq!((r.prompt_tokens, r.output_tokens, r.approx), (Some(20), Some(2), false));
        assert_eq!(r.ttft_ms, r.duration_ms);
    }

    #[tokio::test]
    async fn upstream_down_gives_502_and_error_row() {
        let log = SharedLog::default();
        let proxy = serve(router("http://127.0.0.1:1".into(), log.clone())).await;
        let resp = reqwest::Client::new().get(format!("{proxy}/v1/models")).send().await.unwrap();
        assert_eq!(resp.status(), 502);
        assert!(resp.text().await.unwrap().contains("upstream unreachable"));
        let r = settled(&log).await;
        assert!(matches!(r.status, Status::Error(_)), "{:?}", r.status);
        assert_eq!(r.path, "/v1/models");
    }

    #[tokio::test]
    async fn large_body_passes_through_unchanged() {
        let log = SharedLog::default();
        let proxy = serve(router(upstream().await, log.clone())).await;
        let body = vec![b'x'; 300 * 1024];
        let resp = reqwest::Client::new().post(format!("{proxy}/len")).body(body).send().await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "307200");
    }
}
