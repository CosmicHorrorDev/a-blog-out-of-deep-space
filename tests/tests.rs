use std::{array, collections::BTreeMap, fmt, path::Path, sync::LazyLock};

use a_blog_out_of_deep_space::router;
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use tokio::task::JoinSet;
use tower::{Service, ServiceExt};

async fn call_test_server(req: Request) -> Response {
    // cache to avoid costly reinitialization
    static ROUTER: LazyLock<Router> =
        LazyLock::new(|| router(Path::new("tests").join("assets").join("site")));
    let mut router = ROUTER.clone();
    <_ as ServiceExt<Request>>::ready(&mut router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap()
}

fn get_req(path: &str) -> Request {
    Request::get(path).body(Body::empty()).unwrap()
}

#[track_caller]
fn assert_resp_success(resp: &Response) {
    assert!(
        resp.status().is_success(),
        "Error response: {}",
        resp.status()
    );
}

async fn body_vec(body: Body) -> Option<Vec<u8>> {
    const LIMIT: usize = 10 * 1_024 * 1_024;
    let bytes = axum::body::to_bytes(body, LIMIT).await.ok()?;
    Some(bytes.to_vec())
}

async fn body_string(body: Body) -> Option<String> {
    let bytes = body_vec(body).await?;
    String::from_utf8(bytes.to_vec()).ok()
}

struct SnapTextResp {
    status: StatusCode,
    headers: BTreeMap<String, String>,
    body: String,
}

impl fmt::Display for SnapTextResp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            status,
            headers,
            body,
        } = self;
        writeln!(
            f,
            "{} - {}",
            status.as_u16(),
            status.canonical_reason().unwrap()
        )?;
        for (name, value) in headers {
            writeln!(f, "{name:>16}: {value}")?;
        }
        if !body.is_empty() {
            writeln!(f, "---\n{body}")?;
        }
        Ok(())
    }
}

impl SnapTextResp {
    async fn new(resp: Response) -> Self {
        let status = resp.status();
        let headers = resp
            .headers()
            .iter()
            .map(|(n, v)| (n.as_str().to_owned(), v.to_str().unwrap().to_owned()))
            .collect();
        let body = body_string(resp.into_body()).await.unwrap();
        Self {
            status,
            headers,
            body,
        }
    }
}

#[tokio::test]
async fn sanity_root() {
    let req = get_req("/");
    let resp = call_test_server(req).await;
    assert_resp_success(&resp);
    let snap_resp = SnapTextResp::new(resp).await;
    insta::assert_snapshot!(
        snap_resp,
        @r#"
        200 - OK
         accept-encoding: gzip, br
           cache-control: max-age=300
          content-length: 654
            content-type: text/html; charset=utf-8
                    etag: "e2e7b1b46a3923e"
                  server: a-blog-out-of-deep-space 0.1.0
        ---
        <!doctype html>
        <html lang="en">
        <head>
        <link rel="icon" type="image/png" href="/img/favicon.png" />
        <meta name="author" content="Cosmic Horror" />
        </head>

        <body>

        <h1>The base</h1>

        <p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
        tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam,
        quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo
        consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum
        dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident,
        sunt in culpa qui officia deserunt mollit anim id est laborum.</p>

        </body>
        </html>
        "#,
    );
}

#[tokio::test]
async fn index_html_normalized() {
    let equiv_paths = &["/posts", "/posts/", "/posts/index.html"];
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
async fn status_code_page_not_found() {
    let req = get_req("/408.html");
    let resp = call_test_server(req).await;
    let snap_resp = SnapTextResp::new(resp).await;
    insta::assert_snapshot!(
        snap_resp,
        @r#"
        404 - Not Found
          content-length: 519
            content-type: text/html; charset=utf-8
                    etag: "7e03829c89f8eb3f"
                  server: a-blog-out-of-deep-space 0.1.0
        ---
        <!doctype html>
        <html lang="en">
        <h1>404 NOT FOUND</h1>

        <p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
        tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam,
        quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo
        consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum
        dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident,
        sunt in culpa qui officia deserunt mollit anim id est laborum.</p>

        </html>
        "#,
    );
}

#[tokio::test]
async fn status_code_page_can_be_compressed() {
    let mut req = get_req("/not-found");
    req.headers_mut().insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("deflate, br"),
    );
    let resp = call_test_server(req).await;
    let resp_headers = resp.headers();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp_headers.get(header::CONTENT_ENCODING).unwrap(), "br");
    assert!(!resp_headers.contains_key(header::ACCEPT_ENCODING));
}

/// server supports etag based revalidation to support client http caches
#[tokio::test]
async fn revalidation() {
    let path = "/img/favicon.png";

    // initial call to get the resource's etag
    let req = get_req(path);
    let resp = call_test_server(req).await;
    assert_resp_success(&resp);
    let mut etag_iter = resp.headers().get_all(header::ETAG).iter().cloned();
    let [Some(etag), None] = array::from_fn(|_| etag_iter.next()) else {
        panic!("there should be only one etag");
    };

    // and now try revalidating with said tag
    let mut req = get_req(path);
    req.headers_mut().insert(header::IF_NONE_MATCH, etag);
    let resp = call_test_server(req).await;
    let snap_resp = SnapTextResp::new(resp).await;
    insta::assert_snapshot!(
        snap_resp,
        @r"
        304 - Not Modified
           cache-control: max-age=300
          content-length: 0
            content-type: image/png
                  server: a-blog-out-of-deep-space 0.1.0
        ",
    );
}

/// server supports serving compressed content through proactive-content negotiation
#[tokio::test]
async fn proactive_content_negotiation() {
    #[track_caller]
    fn uncompress_text(compressed: &[u8]) -> String {
        use std::io::prelude::*;

        use flate2::read::GzDecoder;

        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut text = String::new();
        decoder.read_to_string(&mut text).unwrap();
        text
    }

    let path = "/sitemap.xml";

    // get response with a compressed body
    let mut req = get_req(path);
    req.headers_mut().insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("gzip, deflate, br, zstd"),
    );
    let resp = call_test_server(req).await;
    assert_resp_success(&resp);
    let resp_headers = resp.headers();
    let resp_accept_encoding = resp_headers.get(header::ACCEPT_ENCODING).unwrap();
    insta::assert_snapshot!(resp_accept_encoding.to_str().unwrap(), @"gzip, br");
    let resp_vary = resp_headers.get(header::VARY).unwrap();
    assert_eq!(resp_vary, HeaderValue::from(header::ACCEPT_ENCODING));
    let resp_content_encoding = resp_headers.get(header::CONTENT_ENCODING).unwrap();
    assert_eq!(resp_content_encoding, "gzip");
    let compressed_body = body_vec(resp.into_body()).await.unwrap();

    // and now the uncompressed body
    let req2 = get_req(path);
    let resp2 = call_test_server(req2).await;
    assert_resp_success(&resp2);
    let full_body = body_string(resp2.into_body()).await.unwrap();

    // which should be equal to the decompressed body
    assert_eq!(uncompress_text(&compressed_body), full_body);
}
