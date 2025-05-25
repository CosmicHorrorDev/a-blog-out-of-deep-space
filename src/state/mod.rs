use axum::extract::State;

mod dir;
mod file;

pub use dir::ServedDir;

pub type AppState = State<ServedDir>;
