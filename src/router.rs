use std::time::Duration;

use crate::{
    extract::{Encoding, IfNoneMatch},
    state::{AppState, ServedDir},
};

use axum::{
    BoxError, Router,
    error_handling::HandleErrorLayer,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::get,
};
use tower::ServiceBuilder;

pub fn router(dir: ServedDir) -> Router {
    // `axum` (at the time of writing) doesn't support passing state into the function for
    // `HandleError`, so instead we capture it in a closure here
    let dir2 = dir.clone();
    let middleware_error_w_state = async |err| handle_middleware_error(dir2, err).await;

    Router::new()
        .route("/", get(root))
        .route("/{*path}", get(static_file))
        .layer(
            // NOTE: when you add a fallible middleware here make sure that you handle the error in
            // `handle_middleware_error`
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(middleware_error_w_state))
                // TODO: allow customizing this value
                .timeout(Duration::from_secs(60))
                .load_shed(),
        )
        .with_state(dir)
}

async fn handle_middleware_error(dir: ServedDir, err: BoxError) -> Response {
    let status = if err.is::<tower::load_shed::error::Overloaded>() {
        StatusCode::SERVICE_UNAVAILABLE
    } else if err.is::<tower::timeout::error::Elapsed>() {
        StatusCode::REQUEST_TIMEOUT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    dir.get_status(status, Encoding::default())
}

async fn root(state: AppState, encoding: Encoding, if_none_match: Option<IfNoneMatch>) -> Response {
    let path = Path("index.html".to_owned());
    static_file(state, path, encoding, if_none_match).await
}

async fn static_file(
    State(dir): AppState,
    Path(path): Path<String>,
    encoding: Encoding,
    if_none_match: Option<IfNoneMatch>,
) -> Response {
    dir.get_file(path, encoding, if_none_match)
        .unwrap_or_else(|| dir.get_status(StatusCode::NOT_FOUND, encoding))
}
