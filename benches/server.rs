use std::{hint::black_box, path::Path, time::Duration};

use axum::{body::Body, extract::Request, http::header};
use blog_server::{ServedDir, router};
use divan::{Bencher, Divan, bench};
use tower::{Service, ServiceExt};

fn main() {
    // Run registered benchmarks
    Divan::default()
        .min_time(Duration::from_secs(3))
        .config_with_args()
        .main();
}

#[bench]
pub fn request_variety(bencher: Bencher) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    let dir = ServedDir::load(Path::new("tests").join("assets").join("site"));
    // TODO: add etag revalidation?
    // NOTE: internally uses `tokio::spawn`, so must be run from an async context
    let mut app = rt.block_on(async { router(dir) });
    let reqs = [
        Request::get("/"),
        Request::get("/").header(header::ACCEPT_ENCODING, "br"),
        Request::get("/not-found"),
    ]
    .map(|b| b.body(()).unwrap().into_parts().0);

    bencher.counter(reqs.len()).bench_local(|| {
        rt.block_on(async {
            for req in reqs.clone() {
                let req = Request::from_parts(black_box(req), Body::empty());
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
            }
        });
    });
}
