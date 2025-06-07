use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

use super::file::{ContentType, ServedFile};
use crate::extract::{Encoding, IfNoneMatch};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};

/// An in-memory map automatically synced from a filesystem directory through a notification
/// watcher
///
/// This avoids the need to handle updating the in-memory structure while trying to fetch an entry
/// which is a very nice property to have because fetching is user-controlled behavior while we can
/// keep writing exclusively to server-controlled behavior
// TODO: maybe switch the key to a (path, content type)?
#[derive(Clone)]
pub struct ServedDir(Arc<HashMap<String, ServedFile>>);

impl ServedDir {
    pub fn load(dir_path: PathBuf) -> Self {
        use walkdir::WalkDir;

        let start = Instant::now();
        let inner: HashMap<_, _> = WalkDir::new(&dir_path)
            .into_iter()
            .filter_map(|res| {
                let start = Instant::now();

                let entry = res.ok()?;
                let path = entry.path();

                let served_file = ServedFile::load(&path)?;
                let rel_path = path
                    .strip_prefix(&dir_path)
                    .unwrap()
                    .components()
                    .map(|comp| comp.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/");

                tracing::debug!(%rel_path, elapsed = ?start.elapsed(), "Loaded file");
                Some((rel_path, served_file))
            })
            .collect();
        tracing::info!(
            num = %inner.len(),
            elapsed = ?start.elapsed(),
            "Loaded all files"
        );
        Self(Arc::new(inner))
    }

    pub fn get_file(
        &self,
        mut path: String,
        encoding: Encoding,
        if_none_match: Option<IfNoneMatch>,
    ) -> Option<Response> {
        // guard against getting status code pages. use `.get_status(...)` for that
        if path
            .strip_suffix(".html")
            .is_some_and(|prefix| prefix.parse::<StatusCode>().is_ok())
        {
            return None;
        }

        // Implicitly tack on `/index.html` when missing
        let is_known_file = path
            .rsplit_once('.')
            .and_then(|(_, ext)| ContentType::from_file_ext(ext))
            .is_some();
        if !is_known_file {
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str("index.html");
        }

        self.get_file_directly(&path, encoding, if_none_match)
    }

    pub fn get_status(&self, status: StatusCode, encoding: Encoding) -> Response {
        let mut resp = self
            .get_file_directly(&format!("{}.html", status.as_u16()), encoding, None)
            .unwrap_or_else(|| Response::new(Body::from(status.to_string())));
        *resp.status_mut() = status;
        // it's an error page, so we don't know what it's really supposed to be
        resp.headers_mut().remove(header::ACCEPT_ENCODING);
        resp.headers_mut().remove(header::CACHE_CONTROL);
        resp
    }

    fn get_file_directly(
        &self,
        path: &str,
        encoding: Encoding,
        if_none_match: Option<IfNoneMatch>,
    ) -> Option<Response> {
        self.0
            .get(path)
            .map(|file| file.to_response(encoding, if_none_match))
    }
}
