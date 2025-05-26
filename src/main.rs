use std::{env, net::Ipv4Addr};

use blog_server::{ServedDir, router};
use tokio::net::TcpListener;

// TODO: camino for utf8 paths?
// TODO: log if we get requests from user-agents we don't like
// TODO: allllll the middleware
// TODO: .env
// TODO: strip exif data off of images?
#[tokio::main]
async fn main() {
    let mut args = env::args();
    let _bin = args.next();
    let dir_to_serve = args.next().unwrap();
    println!("Loading {dir_to_serve}...");
    let dir = ServedDir::load(dir_to_serve.into());

    let app = router(dir);
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 8080))
        .await
        .unwrap();
    // TODO: display server addr
    println!("Launching server...");
    axum::serve(listener, app).await.unwrap();
}
