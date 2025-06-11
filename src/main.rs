use std::{env, net::Ipv4Addr};

use blog_server::router;
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};

// TODO: camino for utf8 paths?
// TODO: strip exif data off of images?
#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder()
                .with_env_var("LOG")
                .with_default_directive(LevelFilter::DEBUG.into())
                .from_env()
                .unwrap(),
        )
        .init();

    let mut args = env::args();
    let _bin = args.next();
    let dir_to_serve = args.next().unwrap();
    tracing::info!("Loading {dir_to_serve}...");

    let app = router(dir_to_serve.into());
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 8080))
        .await
        .unwrap();
    // TODO: display server addr
    tracing::info!("Launching server...");
    axum::serve(listener, app).await.unwrap();
}
