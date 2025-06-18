use std::{hint::black_box, path::Path, time::Duration};

use a_blog_out_of_deep_space::router;
use axum::{
    body::Body,
    extract::Request,
    http::{header, request},
    response::Response,
};
use divan::{Bencher, Divan, bench};
use tokio::runtime::Runtime;
use tower::{Service, ServiceExt};

fn main() {
    // Run registered benchmarks
    Divan::default()
        .min_time(Duration::from_millis(500))
        .config_with_args()
        .main();
}

#[bench]
fn root(bencher: Bencher) {
    let req = Request::get("/");
    bench_req(bencher, req);
}

#[bench]
fn root_index_html(bencher: Bencher) {
    let req = Request::get("/index.html");
    bench_req(bencher, req);
}

#[bench]
fn root_compressed(bencher: Bencher) {
    let req = Request::get("/index.html").header(header::ACCEPT_ENCODING, "deflate, br");
    bench_req(bencher, req);
}

#[bench]
fn not_found(bencher: Bencher) {
    let req = Request::get("/not-found");
    bench_req(bencher, req);
}

#[bench]
fn revalidation(bencher: Bencher) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    // get the etag for this resp
    let initial = Request::get("/").body(Body::empty()).unwrap();
    let resp = rt.block_on(async { call_req(initial).await });
    let etag = resp.headers().get(header::ETAG).unwrap();

    // and benchmark revalidating with the etag
    let revalidate = req_parts(Request::get("/").header(header::IF_NONE_MATCH, etag));
    bench_req_with_rt(bencher, rt, revalidate);
}

fn req_parts(req: request::Builder) -> request::Parts {
    req.body(()).unwrap().into_parts().0
}

async fn call_req(req: Request) -> Response {
    let dir = Path::new("tests").join("assets").join("site");
    let mut app = router(dir);
    <_ as ServiceExt<Request>>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap()
}

fn bench_req(bencher: Bencher, req: request::Builder) {
    let parts = req_parts(req);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    bench_req_with_rt(bencher, rt, parts);
}

fn bench_req_with_rt(bencher: Bencher, rt: Runtime, parts: request::Parts) {
    let dir = Path::new("tests").join("assets").join("site");
    // TODO: add etag revalidation?
    // NOTE: internally uses `tokio::spawn`, so must be run from an async context
    let mut app = rt.block_on(async { router(dir) });
    bencher.counter(1u32).bench_local(|| {
        rt.block_on(async {
            let req = Request::from_parts(black_box(parts.clone()), Body::empty());
            let resp = <_ as ServiceExt<Request>>::ready(&mut app)
                .await
                .unwrap()
                .call(req)
                .await
                .unwrap();
            let resp = black_box(resp);
            black_box(
                axum::body::to_bytes(resp.into_body(), 1_024 * 1_024)
                    .await
                    .unwrap(),
            );
        });
    });
}
