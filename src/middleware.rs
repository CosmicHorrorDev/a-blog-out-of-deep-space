use std::{
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
use tower::{Layer, Service};

use crate::util::disp;

#[derive(Clone, Debug)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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

async fn recorder_worker(mut recv_stream: RecvStream<'static, RecorderEntry>) {
    while let Some((time, duration, req, resp)) = recv_stream.next().await {
        tracing::trace!(time = %disp::Time(time), duration = %disp::Duration(duration), ?req, ?resp);
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
