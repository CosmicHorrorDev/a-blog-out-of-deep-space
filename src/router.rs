use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    extract::{Encoding, IfNoneMatch},
    file::ServedFile,
    middleware::RecorderLayer,
    util::disp,
};

use axum::{
    BoxError, Router,
    body::Body,
    error_handling::HandleErrorLayer,
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use tower::ServiceBuilder;
use walkdir::WalkDir;

// TODO: return an error in here instead of filtering out any bad entries?
// TODO: refactor all of this to be less gross
pub fn router(dir: PathBuf) -> Router {
    let mut not_found_page: Option<Arc<_>> = None;
    let mut status_pages = BTreeMap::new();
    let mut router = Router::new();

    for path in WalkDir::new(&dir).into_iter().filter_map(|res| {
        let entry = res.ok()?;
        let path = entry.into_path();
        path.is_file().then_some(path)
    }) {
        let start = Instant::now();

        let Some(served_file) = ServedFile::load(&path) else {
            // TODO: log
            continue;
        };

        let rel_path = path
            .strip_prefix(&dir)
            .unwrap()
            .components()
            .map(|comp| comp.as_os_str().to_str().unwrap())
            .collect::<Vec<_>>()
            .join("/");

        if let Some(status_code) = rel_path
            .strip_suffix(".html")
            .and_then(|name| name.parse::<StatusCode>().ok())
        {
            if status_code == StatusCode::NOT_FOUND {
                not_found_page = Some(served_file.clone().into());
            }
            status_pages.insert(status_code, served_file);
        } else {
            let rel_path = format!("/{rel_path}");
            let served_file = Arc::new(served_file);
            // add equivalent routes on `/index.html` pages
            if rel_path == "index.html" {
                let served_file = Arc::clone(&served_file);
                router = router.route(
                    "/",
                    get(async |encoding, if_none_match| {
                        serve_file(encoding, if_none_match, served_file).await
                    }),
                );
            } else if rel_path.ends_with("/index.html") {
                if rel_path != "/index.html" {
                    let served_file = Arc::clone(&served_file);
                    router = router.route(
                        rel_path.strip_suffix("/index.html").unwrap(),
                        get(async |encoding, if_none_match| {
                            serve_file(encoding, if_none_match, served_file).await
                        }),
                    );
                }
                let served_file = Arc::clone(&served_file);
                router = router.route(
                    rel_path.strip_suffix("index.html").unwrap(),
                    get(async |encoding, if_none_match| {
                        serve_file(encoding, if_none_match, served_file).await
                    }),
                );
            }
            router = router.route(
                &rel_path,
                get(async |encoding, if_none_match| {
                    serve_file(encoding, if_none_match, served_file).await
                }),
            );
        }

        tracing::debug!(%rel_path, elapsed = %disp::Duration(start.elapsed()), "Loaded file");
    }

    // `axum` (at the time of writing) doesn't support passing state into the function for
    // `HandleError`, so instead we capture it in a closure here
    let middleware_error_w_state =
        async |encoding, err| handle_middleware_error(status_pages.into(), encoding, err).await;

    router
        .fallback(async move |encoding| {
            status_code_page(not_found_page.as_deref(), StatusCode::NOT_FOUND, encoding)
        })
        .layer(
            // NOTE: when you add a fallible middleware here make sure that you handle the error in
            // `handle_middleware_error`
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(middleware_error_w_state))
                // TODO: allow customizing this value
                .timeout(Duration::from_secs(60))
                .load_shed()
                .layer(RecorderLayer::spawn()),
        )
}

async fn handle_middleware_error(
    status_pages: Arc<BTreeMap<StatusCode, ServedFile>>,
    encoding: Encoding,
    err: BoxError,
) -> Response {
    let status = if err.is::<tower::load_shed::error::Overloaded>() {
        StatusCode::SERVICE_UNAVAILABLE
    } else if err.is::<tower::timeout::error::Elapsed>() {
        StatusCode::REQUEST_TIMEOUT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    status_code_page(status_pages.get(&status), status, encoding)
}

fn status_code_page(page: Option<&ServedFile>, status: StatusCode, encoding: Encoding) -> Response {
    let mut resp = match page {
        Some(file) => file.to_response(encoding, None),
        None => Response::new(Body::from(status.to_string())),
    };

    *resp.status_mut() = status;
    // it's a status code page, so we don't know what content we would return
    resp.headers_mut().remove(header::ACCEPT_ENCODING);
    resp.headers_mut().remove(header::CACHE_CONTROL);
    resp
}

async fn serve_file(
    encoding: Encoding,
    if_none_match: Option<IfNoneMatch>,
    file: Arc<ServedFile>,
) -> Response {
    file.to_response(encoding, if_none_match)
}
