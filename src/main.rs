use std::{env, net::Ipv4Addr};

use blog_server::{ServedDir, router};
use tokio::net::TcpListener;

// TODO: camino for utf8 paths?
// TODO: strip exif data off of images?
#[tokio::main]
async fn main() {
    use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};

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
    let dir = ServedDir::load(dir_to_serve.into());

    let app = router(dir);
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 8080))
        .await
        .unwrap();
    // TODO: display server addr
    tracing::info!("Launching server...");
    axum::serve(listener, app).await.unwrap();
}
