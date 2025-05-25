use std::{fs, path::PathBuf, sync::Arc, time::Instant};

use super::file::{ContentType, File, ServedFile};
use crate::extract::{Encoding, IfNoneMatch};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use dashmap::DashMap;

/// An in-memory map automatically synced from a filesystem directory through a notification
/// watcher
///
/// This avoids the need to handle updating the in-memory structure while trying to fetch an entry
/// which is a very nice property to have because fetching is user-controlled behavior while we can
/// keep writing exclusively to server-controlled behavior
// TODO: maybe switch the key to a (path, content type)?
#[derive(Clone)]
pub struct ServedDir(Arc<DashMap<String, ServedFile>>);

impl ServedDir {
    pub fn load(dir_path: PathBuf) -> Self {
        use twox_hash::XxHash64;
        use walkdir::WalkDir;

        let start = Instant::now();
        let inner: DashMap<_, _> = WalkDir::new(&dir_path)
            .into_iter()
            .filter_map(|res| {
                let start = Instant::now();

                let entry = res.ok()?;
                let path = entry.path();

                let rel_path = path
                    .strip_prefix(&dir_path)
                    .unwrap()
                    .components()
                    .map(|comp| comp.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/");

                // TODO: split logic out into ServedFile from path helper

                let ext = path.extension()?.to_str()?;
                let ty = ContentType::from_file_ext(ext)?;

                let contents = fs::read(path).ok()?;
                let e_tag = {
                    const ARBITRARY_SEED: u64 = 0xc0ffee;
                    let hash = XxHash64::oneshot(ARBITRARY_SEED, &contents);
                    // format as a strong e-tag as we're constructing it off the bytes themselves
                    let value = format!("\"{hash:x}\"");
                    value
                        .parse()
                        .expect("quotes and 0-9a-f are all valid header contents")
                };

                let file = if ty.is_compressible() {
                    let contents = String::from_utf8(contents).ok()?;
                    File::Text(contents.into())
                } else {
                    File::Data(contents.into())
                };

                let served_file = ServedFile { e_tag, ty, file };

                println!("- Loaded {} in {:0.2?}", rel_path, start.elapsed());
                Some((rel_path, served_file))
            })
            .collect();
        println!("Loaded {} files in {:0.2?}", inner.len(), start.elapsed());
        Self(Arc::new(inner))
    }

    pub fn get_file(
        &self,
        mut path: String,
        encoding: Encoding,
        if_none_match: IfNoneMatch,
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
            .get_file_directly(&format!("{}.html", status.as_u16()), encoding, None.into())
            .unwrap_or_else(|| Response::new(Body::from(status.to_string())));
        *resp.status_mut() = status;
        // it's an error page, so we don't know what it's really supposed to be
        resp.headers_mut().remove(header::ACCEPT_ENCODING);
        resp
    }

    fn get_file_directly(
        &self,
        path: &str,
        encoding: Encoding,
        if_none_match: IfNoneMatch,
    ) -> Option<Response> {
        self.0
            .get(path)
            .map(|file| file.to_response(encoding, if_none_match))
    }
}
