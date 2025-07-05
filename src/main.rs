use std::{array, env, net::Ipv4Addr, process};

use a_blog_out_of_deep_space::router;
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};

// TODO: camino for utf8 paths?
// TODO: strip exif data off of images?
#[tokio::main]
async fn main() {
    // setup logging
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

    // parse cli args
    let mut args = env::args();
    let args = array::from_fn(|_| args.next());
    let dir_to_serve = match args {
        [Some(_), Some(dir), None] if !["-h", "--help", "help"].contains(&&*dir) => dir,
        [None, ..] => panic!("Somehow arg0 is unset...?"),
        [Some(bin), ..] => {
            eprintln!(
                "Usage: {bin} <DIR_TO_SERVE>\n\
                \n\
                Arguments:\n  \
                <DIR_TO_SERVE>  Directory containing a config file and files to serve"
            );
            process::exit(1);
        }
    };
    tracing::info!("Loading {dir_to_serve}...");

    // launch server
    let app = router(dir_to_serve.into());
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 8080))
        .await
        .unwrap();
    // TODO: display server addr
    tracing::info!("Launching server...");
    axum::serve(listener, app).await.unwrap();
}
