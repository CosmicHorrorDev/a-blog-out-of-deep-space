use std::{
    convert::Infallible,
    env, fs,
    net::Ipv4Addr,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    BoxError, Router,
    body::{Body, Bytes},
    error_handling::HandleErrorLayer,
    extract::{FromRequestParts, Path, State},
    http::{HeaderValue, StatusCode, header, request, response},
    response::Response,
    routing::get,
};
use dashmap::DashMap;
use tokio::net::TcpListener;
use tower::ServiceBuilder;

#[derive(Clone, Copy, Debug)]
enum ContentType {
    Html,
    Js,
    Svg,
    Css,
    Xml,
    Txt,
    Woff,
    Woff2,
    Ico,
    Png,
}

impl ContentType {
    const fn into_value(self) -> HeaderValue {
        match self {
            ContentType::Html => HeaderValue::from_static("text/html; charset=utf-8"),
            ContentType::Js => HeaderValue::from_static("application/javascript; charset=UTF-8"),
            ContentType::Svg => HeaderValue::from_static("image/svg+xml"),
            ContentType::Css => HeaderValue::from_static("text/css; charset=utf-8"),
            ContentType::Xml => HeaderValue::from_static("application/xml; charset=UTF-8"),
            ContentType::Txt => HeaderValue::from_static("text/plain"),
            ContentType::Woff => HeaderValue::from_static("font/woff"),
            ContentType::Woff2 => HeaderValue::from_static("font/woff2"),
            ContentType::Ico => HeaderValue::from_static("image/vnd.microsoft.icon"),
            ContentType::Png => HeaderValue::from_static("image/png"),
        }
    }

    fn is_compressible(self) -> bool {
        match self {
            Self::Html | Self::Js | Self::Svg | Self::Css | Self::Xml | Self::Txt => true,
            Self::Woff | Self::Woff2 | Self::Ico | Self::Png => false,
        }
    }

    fn from_file_ext(ext: &str) -> Option<Self> {
        let ty = match ext {
            "html" => Self::Html,
            "js" => Self::Js,
            "svg" => Self::Svg,
            "css" => Self::Css,
            "xml" => Self::Xml,
            "txt" => Self::Txt,
            "woff" => Self::Woff,
            "woff2" => Self::Woff2,
            "ico" => Self::Ico,
            "png" => Self::Png,
            _ => return None,
        };
        Some(ty)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Encoding {
    #[default]
    Identity,
    Gzip,
    Brotli,
}

impl Encoding {
    const ALL_ENCODINGS: HeaderValue = HeaderValue::from_static("gzip, br");

    const fn into_content_encoding_value(self) -> Option<HeaderValue> {
        match self {
            Self::Identity => None,
            Self::Gzip => Some(HeaderValue::from_static("gzip")),
            Self::Brotli => Some(HeaderValue::from_static("br")),
        }
    }
}

impl FromStr for Encoding {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let encoding = match s {
            "identity" => Self::Identity,
            "gzip" => Self::Gzip,
            "br" => Self::Brotli,
            _ => return Err(()),
        };
        Ok(encoding)
    }
}

// TODO: need to handle wildcard encoding and rank by quality
impl FromRequestParts<ServedDir> for Encoding {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut request::Parts,
        _: &ServedDir,
    ) -> Result<Self, Self::Rejection> {
        fn from_req_parts(parts: &request::Parts) -> Option<Encoding> {
            let accept_encoding = parts.headers.get(header::ACCEPT_ENCODING)?;
            let accept_encoding = accept_encoding.to_str().ok()?;
            accept_encoding
                .split(',')
                .map(|chunk| {
                    let trimmed = chunk.trim();
                    match trimmed.split_once(';') {
                        Some((encoding, quality)) => todo!(),
                        None => trimmed,
                    }
                })
                .filter_map(|encoding| encoding.parse().ok())
                .next()
        }

        let encoding = from_req_parts(&*parts).unwrap_or_default();
        Ok(encoding)
    }
}

#[derive(Default)]
struct IfNoneMatch(Option<String>);

impl FromRequestParts<ServedDir> for IfNoneMatch {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut request::Parts,
        _: &ServedDir,
    ) -> Result<Self, Self::Rejection> {
        let maybe_tag = parts
            .headers
            .get(header::IF_NONE_MATCH)
            .and_then(|tag| tag.to_str().ok())
            .map(ToOwned::to_owned);
        Ok(Self(maybe_tag))
    }
}

impl From<Option<String>> for IfNoneMatch {
    fn from(maybe_tag: Option<String>) -> Self {
        Self(maybe_tag)
    }
}

type AppState = State<ServedDir>;

/// An in-memory map automatically synced from a filesystem directory through a notification
/// watcher
///
/// This avoids the need to handle updating the in-memory structure while trying to fetch an entry
/// which is a very nice property to have because fetching is user-controlled behavior while we can
/// keep writing exclusively to server-controlled behavior
// TODO: maybe switch the key to a (path, content type)?
#[derive(Clone)]
struct ServedDir(Arc<DashMap<String, ServedFile>>);

impl ServedDir {
    fn load(dir_path: PathBuf) -> Self {
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

    fn get_file(
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

    fn get_status(&self, status: StatusCode, encoding: Encoding) -> Response {
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

struct ServedFile {
    e_tag: HeaderValue,
    ty: ContentType,
    file: File,
}

impl ServedFile {
    fn to_response(&self, encoding: Encoding, if_none_match: IfNoneMatch) -> Response {
        const SERVER: HeaderValue = HeaderValue::from_static(concat!(
            "a-blog-out-of-deep-space ",
            env!("CARGO_PKG_VERSION")
        ));
        let mut temp = SERVER.clone();
        temp.set_sensitive(true);
        let mut builder = Response::builder()
            .header(header::SERVER, SERVER)
            .header(header::CONTENT_TYPE, self.ty.into_value())
            // TODO: set this based on content type?
            .header(
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=300"),
            );

        match if_none_match.0 {
            // Handle e-tag revalidation
            Some(client_tag) if client_tag == self.e_tag => builder
                .status(StatusCode::NOT_MODIFIED)
                .body(Body::empty())
                .unwrap(),
            _ => {
                builder = builder.header(header::ETAG, self.e_tag.clone());

                match &self.file {
                    File::Data(data_file) => builder.body(data_file.to_owned().into()).unwrap(),
                    File::Text(text_file) => text_file.finish_response(builder, encoding),
                }
            }
        }
    }
}

// TODO: switch this to automaitcally try compressing and bail out if the size isn't better
enum File {
    Data(DataFile),
    Text(TextFile),
}

#[derive(Clone)]
struct DataFile(Bytes);

impl From<DataFile> for Body {
    fn from(file: DataFile) -> Self {
        file.0.into()
    }
}

impl From<Vec<u8>> for DataFile {
    fn from(content: Vec<u8>) -> Self {
        Self(content.into())
    }
}

// NOTE: UTF-8 is validated before construction
struct TextFile {
    gz_compressed: Bytes,
    br_compressed: Bytes,
    contents: Bytes,
}

impl TextFile {
    fn finish_response(&self, mut builder: response::Builder, encoding: Encoding) -> Response {
        // Include the encodings we support for this entity no matter what
        builder = builder.header(header::ACCEPT_ENCODING, Encoding::ALL_ENCODINGS);

        // Setup headers for our content encoding
        if let Some(content_encoding) = encoding.into_content_encoding_value() {
            builder = builder
                .header(header::VARY, header::ACCEPT_ENCODING)
                .header(header::CONTENT_ENCODING, content_encoding)
        }

        builder.body(self.select_body(encoding)).unwrap()
    }

    fn select_body(&self, encoding: Encoding) -> Body {
        match encoding {
            Encoding::Gzip => self.gz_compressed.clone(),
            Encoding::Brotli => self.br_compressed.clone(),
            Encoding::Identity => self.contents.clone(),
        }
        .into()
    }
}

impl From<String> for TextFile {
    fn from(contents: String) -> Self {
        fn check_compression_ratio(source: &[u8], compressed: &[u8]) {
            let ratio = compressed.len() as f32 / source.len() as f32;
            if ratio > 0.9 {
                println!("- Poor compression ratio: {ratio}");
            }
        }
        let gz_compressed: Bytes = gz_compress(contents.as_bytes()).into();
        let br_compressed: Bytes = br_compress(contents.as_bytes()).into();
        let contents: Bytes = contents.into();
        check_compression_ratio(&contents, &gz_compressed);
        check_compression_ratio(&contents, &br_compressed);
        Self {
            gz_compressed,
            br_compressed,
            contents,
        }
    }
}

fn gz_compress(bytes: &[u8]) -> Vec<u8> {
    use std::io::prelude::*;

    use flate2::{Compression, write::GzEncoder};

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn br_compress(bytes: &[u8]) -> Vec<u8> {
    use std::io::prelude::*;

    use brotli::CompressorWriter;

    const BUFFER_SIZE: usize = 4_096;
    const BEST_QUALITY: u32 = 11;
    const LGWIN: u32 = 22;

    let output = Vec::new();
    let mut encoder = CompressorWriter::new(output, BUFFER_SIZE, BEST_QUALITY, LGWIN);
    encoder.write_all(bytes).unwrap();
    encoder.flush().unwrap();
    encoder.into_inner()
}

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
    let served_dir = ServedDir::load(dir_to_serve.into());

    // `axum` (at the time of writing) doesn't support passing state into the function for
    // `HandleError`, so instead we capture it in a closure here
    let dir2 = served_dir.clone();
    let middleware_error_w_state = async |err: BoxError| handle_middleware_error(dir2, err).await;

    let app = Router::new()
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
        .with_state(served_dir);
    // TODO: use a normal port
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 8080))
        .await
        .unwrap();
    // TODO: display server addr
    println!("Launching server...");
    axum::serve(listener, app).await.unwrap();
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

async fn root(state: AppState, encoding: Encoding, if_none_match: IfNoneMatch) -> Response {
    let path = Path("index.html".to_owned());
    static_file(state, path, encoding, if_none_match).await
}

async fn static_file(
    State(dir): AppState,
    Path(path): Path<String>,
    encoding: Encoding,
    if_none_match: IfNoneMatch,
) -> Response {
    dir.get_file(path, encoding, if_none_match)
        .unwrap_or_else(|| dir.get_status(StatusCode::NOT_FOUND, encoding))
}
