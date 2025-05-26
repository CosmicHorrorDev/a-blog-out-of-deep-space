use std::{path::Path, sync::LazyLock};

use axum::{body::Body, extract::Request, http::StatusCode, response::Response};
use blog_server::{ServedDir, router};
use tokio::task::JoinSet;
use tower::{Service, ServiceExt};

async fn call_test_server(req: Request) -> Response {
    // cache to avoid costly reinitialization
    static DIR: LazyLock<ServedDir> =
        LazyLock::new(|| ServedDir::load(Path::new("tests").join("assets").join("site")));
    let mut app = router(DIR.clone());
    <_ as ServiceExt<Request>>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap()
}

fn get_req(path: &str) -> Request {
    if path.is_empty() {
        Request::new(Body::empty())
    } else {
        Request::get(path).body(Body::empty()).unwrap()
    }
}

#[track_caller]
fn assert_resp_success(resp: &Response) {
    assert!(
        resp.status().is_success(),
        "Error response: {}",
        resp.status()
    );
}

async fn body_string(body: Body) -> Option<String> {
    const LIMIT: usize = 10 * 1_024 * 1_024;
    let bytes = axum::body::to_bytes(body, LIMIT).await.ok()?;
    String::from_utf8(bytes.to_vec()).ok()
}

#[tokio::test]
async fn sanity_root() {
    let req = get_req("");
    let resp = call_test_server(req).await;
    assert_resp_success(&resp);
    let body = body_string(resp.into_body()).await.unwrap();
    insta::assert_snapshot!(body, @r#"
    <!DOCTYPE html>
    <html lang="en">

    <head>

    <h1>The base</h1>

    </head>
    "#);
}

#[tokio::test]
async fn index_html_normalized() {
    let equiv_paths = &["", "/", "/index.html"];
    let mut req_set = JoinSet::new();
    for path in equiv_paths {
        req_set.spawn(async move {
            let req = get_req(path);
            let resp = call_test_server(req).await;
            assert_resp_success(&resp);
            body_string(resp.into_body()).await.unwrap()
        });
    }
    let mut bodies = req_set.join_all().await;
    println!("{bodies:#?}");
    let first = bodies.pop().unwrap();
    for body in &bodies {
        assert_eq!(&first, body);
    }
}

/// status code pages in the root of the site aren't reachable
///
/// 408.html is not found depsite existing in the root of the repo
#[tokio::test]
async fn status_pages_not_found() {
    let req = get_req("/408.html");
    let resp = call_test_server(req).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp.into_body()).await.unwrap();
    insta::assert_snapshot!(body, @r#"
    <!DOCTYPE html>
    <html lang="en">

    <head>

    <h1>404 NOT FOUND</h1>

    </head>
    "#);
}
