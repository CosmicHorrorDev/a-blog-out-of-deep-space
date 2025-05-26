use std::path::Path;

use axum::{Router, body::Body, extract::Request, response::Response};
use blog_server::{ServedDir, router};
use tower::{Service, ServiceExt};

#[track_caller]
fn assert_resp_success(resp: &Response) {
    assert!(
        resp.status().is_success(),
        "Error response: {}",
        resp.status()
    );
}

#[tokio::test]
async fn sanity() {
    let dir = ServedDir::load(Path::new("tests").join("assets").join("site"));
    let mut app = router(dir);
    let resp = <Router as ServiceExt<Request>>::ready(&mut app)
        .await
        .unwrap()
        .call(Request::new(Body::empty()))
        .await
        .unwrap();
    assert_resp_success(&resp);
}
