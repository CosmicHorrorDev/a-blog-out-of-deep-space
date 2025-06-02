use std::{
    collections::{BTreeMap, HashMap, hash_map},
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime},
};

use axum::{
    extract::Request,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use flume::{Sender, r#async::RecvStream};
use futures_util::stream::StreamExt;
use pin_project_lite::pin_project;
use serde::Serialize;
use tower::{Layer, Service};

#[derive(Clone, Debug)]
struct ReqMetadata {
    uri: Uri,
    method: Method,
    headers: HeaderMap,
}

impl From<&Request> for ReqMetadata {
    fn from(req: &Request) -> Self {
        let uri = req.uri().to_owned();
        let method = req.method().to_owned();
        let headers = req.headers().to_owned();
        Self {
            uri,
            method,
            headers,
        }
    }
}

#[derive(Clone, Debug)]
struct RespMetadata {
    status: StatusCode,
    headers: HeaderMap,
}

impl From<&Response> for RespMetadata {
    fn from(resp: &Response) -> Self {
        let status = resp.status();
        let headers = resp.headers().to_owned();
        Self { status, headers }
    }
}

type RecorderEntry = (SystemTime, Duration, ReqMetadata, RespMetadata);

// NOTE: we could use `axum::middleware::from_fn`, but that would record storing the sender in
// global state. instead we implement it as a custom middleware to handle its own state
#[derive(Clone)]
pub struct RecorderLayer(Sender<RecorderEntry>);

impl RecorderLayer {
    pub fn spawn() -> Self {
        let (send, recv) = flume::bounded(32);
        let recv_stream: RecvStream<'static, RecorderEntry> = recv.into_stream();
        tokio::spawn(async move {
            recorder_worker(recv_stream).await;
        });
        Self(send)
    }
}

#[derive(Debug, Serialize)]
struct NormEntry {
    time: SystemTime,
    duration: Duration,
    uri: String,
    method: String,
    status: u16,
    req_headers: u64,
    resp_headers: u64,
}

async fn recorder_worker(mut recv_stream: RecvStream<'static, RecorderEntry>) {
    // Normalize headers to a lossy representation where:
    // - Things are ordered to ensure hashing is stable
    // - Any entries that are invalid UTF-8 are filtered
    // - Any header values beyond the first one are ignored
    fn norm_headers(headers: HeaderMap) -> (u64, BTreeMap<String, String>) {
        use std::hash::{Hash, Hasher};

        let norm: BTreeMap<_, _> = headers
            .into_iter()
            .filter_map(|(maybe_name, val)| {
                // `maybe_name` will only be `None` when this value is after the first associated with
                // this name
                let name = maybe_name?;
                let val = val.to_str().ok()?;
                Some((name.as_str().to_owned(), val.to_owned()))
            })
            .collect();
        let mut hasher = twox_hash::XxHash64::with_seed(0xc0ffee);
        norm.hash(&mut hasher);
        let hash = hasher.finish();
        (hash, norm)
    }

    let mut counter = 0;
    let mut norm_entries = Vec::new();
    let mut headers = HashMap::new();

    while let Some((time, duration, req, resp)) = recv_stream.next().await {
        let ReqMetadata {
            uri,
            method,
            headers: req_headers,
        } = req;
        let RespMetadata {
            status,
            headers: resp_headers,
        } = resp;
        let (req_hash, req_headers) = norm_headers(req_headers);
        let (resp_hash, resp_headers) = norm_headers(resp_headers);
        let norm_entry = NormEntry {
            time,
            duration,
            uri: uri.to_string(),
            method: method.as_str().to_owned(),
            status: status.as_u16(),
            req_headers: req_hash,
            resp_headers: resp_hash,
        };
        norm_entries.push(norm_entry);
        if let hash_map::Entry::Vacant(vacant) = headers.entry(req_hash) {
            vacant.insert(req_headers);
        }
        if let hash_map::Entry::Vacant(vacant) = headers.entry(resp_hash) {
            vacant.insert(resp_headers);
        }

        // if we have a decent amount of entries then dump them to a file, and reset our recorder
        // session
        counter += 1;
        if counter > 200 {
            let encoded = bincode::serde::encode_to_vec(
                (&norm_entries, &headers),
                bincode::config::standard(),
            )
            .unwrap();
            let timestamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let output_path = format!("/tmp/record-{timestamp}.bin");
            tokio::fs::write(&output_path, &encoded).await.unwrap();
            println!("Dumped record file {output_path}");
            counter = 0;
            norm_entries.clear();
            headers.clear();
            // eepy time. work may be lost, but that's the whole point
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

impl<S> Layer<S> for RecorderLayer {
    type Service = Recorder<S>;

    fn layer(&self, inner: S) -> Self::Service {
        let sender = self.0.clone();
        Recorder { inner, sender }
    }
}

#[derive(Clone)]
pub struct Recorder<S> {
    inner: S,
    sender: Sender<RecorderEntry>,
}

impl<S> Service<Request> for Recorder<S>
where
    // define a bunch of concrete types based on what `axum` expects
    S: Service<Request, Response = Response, Error = Infallible>,
{
    type Response = Response;
    type Error = Infallible;
    // a custom future, so that we can customize behavior after we get the response
    type Future = RecorderFut<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let start = Instant::now();
        let req_meta = (&req).into();
        let response_fut = self.inner.call(req);
        let sender = self.sender.clone();
        RecorderFut {
            response_fut,
            start,
            req_meta,
            sender,
        }
    }
}

pin_project! {
    pub struct RecorderFut<F> {
        #[pin]
        response_fut: F,
        start: Instant,
        req_meta: ReqMetadata,
        sender: Sender<RecorderEntry>,
    }
}

impl<F> Future for RecorderFut<F>
where
    F: Future<Output = Result<Response, Infallible>>,
{
    type Output = Result<Response, Infallible>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.response_fut.poll(cx) {
            Poll::Ready(Ok(response)) => {
                let duration = this.start.elapsed();
                let resp_meta = (&response).into();
                let _ = this.sender.try_send((
                    SystemTime::now(),
                    duration,
                    this.req_meta.clone(),
                    resp_meta,
                ));
                Poll::Ready(Ok(response))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
