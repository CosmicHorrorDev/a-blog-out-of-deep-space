use std::{fs, mem, path::Path};

use crate::{
    extract::{Encoding, IfNoneMatch},
    util::TotalSize,
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use twox_hash::XxHash64;

#[derive(Clone)]
pub struct ServedFile {
    e_tag: HeaderValue,
    ty: ContentType,
    file: File,
}

impl TotalSize for ServedFile {
    fn total_size(&self) -> usize {
        let ServedFile { e_tag, ty, file } = self;
        e_tag.total_size() + ty.total_size() + file.total_size()
    }
}

impl ServedFile {
    pub fn load(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        let ty = ContentType::from_file_ext(ext)?;

        let contents = fs::read(path).ok()?;
        let e_tag = {
            const ARBITRARY_SEED: u64 = 0xc0ffee;
            let hash = XxHash64::oneshot(ARBITRARY_SEED, &contents);
            // format as a strong e-tag as we're constructing it off the bytes themselves
            let value = format!("\"{hash:x}\"");
            value.parse().expect("the format is a valid e-tag")
        };

        let file = if ty.is_compressible() {
            let contents = String::from_utf8(contents).ok()?;
            File::Text(contents.into())
        } else {
            File::Data(contents.into())
        };

        Some(Self { e_tag, ty, file })
    }

    pub fn to_response(&self, encoding: Encoding, if_none_match: Option<IfNoneMatch>) -> Response {
        const SERVER: HeaderValue = HeaderValue::from_static(concat!(
            "a-blog-out-of-deep-space/",
            env!("CARGO_PKG_VERSION")
        ));
        let mut builder = Response::builder()
            .header(header::SERVER, SERVER)
            .header(header::CONTENT_TYPE, self.ty.into_header_value())
            // TODO: set this based on content type?
            .header(header::CACHE_CONTROL, "max-age=300");

        // handle etag content revalidation
        if if_none_match.is_some_and(|client_tag| client_tag.0 == self.e_tag) {
            builder
                .status(StatusCode::NOT_MODIFIED)
                .body(Body::empty())
                .unwrap()
        } else {
            let bytes = match &self.file {
                File::Data(data_file) => data_file.0.clone(),
                File::Text(text_file) => {
                    text_file.setup_headers(builder.headers_mut().unwrap(), encoding);
                    text_file.select_body_bytes(encoding)
                }
            };

            builder = builder
                .header(header::ETAG, self.e_tag.clone())
                // `axum` automatically sets the content length for us, but we explicitly set it
                // here, so that our custom middleware can see it
                .header(header::CONTENT_LENGTH, bytes.len());

            builder.body(bytes.into()).unwrap()
        }
    }
}

// TODO: switch this to automaitcally try compressing and bail out if the size isn't better
#[derive(Clone)]
enum File {
    Data(DataFile),
    Text(TextFile),
}

impl TotalSize for File {
    fn total_size(&self) -> usize {
        let shallow_size = mem::size_of::<Self>()
            - match self {
                Self::Data(_) => mem::size_of::<DataFile>(),
                Self::Text(_) => mem::size_of::<TextFile>(),
            };
        shallow_size
            + match self {
                Self::Data(d) => d.total_size(),
                Self::Text(t) => t.total_size(),
            }
    }
}

#[derive(Clone)]
struct DataFile(Bytes);

impl TotalSize for DataFile {
    fn total_size(&self) -> usize {
        self.0.total_size()
    }
}

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
#[derive(Clone)]
struct TextFile {
    gz_compressed: Bytes,
    br_compressed: Bytes,
    contents: Bytes,
}

impl TotalSize for TextFile {
    fn total_size(&self) -> usize {
        let Self {
            gz_compressed,
            br_compressed,
            contents,
        } = self;
        gz_compressed.total_size() + br_compressed.total_size() + contents.total_size()
    }
}

impl TextFile {
    fn setup_headers(&self, headers: &mut HeaderMap, encoding: Encoding) {
        // include the encodings we support for this entity no matter what
        headers.insert(header::ACCEPT_ENCODING, Encoding::ALL_ENCODINGS);

        // setup headers for our content encoding
        if let Some(content_encoding) = encoding.into_content_encoding_value() {
            headers.insert(header::VARY, header::ACCEPT_ENCODING.into());
            headers.insert(header::CONTENT_ENCODING, content_encoding);
        }
    }

    fn select_body_bytes(&self, encoding: Encoding) -> Bytes {
        match encoding {
            Encoding::Gzip => self.gz_compressed.clone(),
            Encoding::Brotli => self.br_compressed.clone(),
            Encoding::Identity => self.contents.clone(),
        }
    }
}

impl From<String> for TextFile {
    fn from(contents: String) -> Self {
        fn check_compression_ratio(source: &[u8], compressed: &[u8]) {
            let ratio = compressed.len() as f32 / source.len() as f32;
            if ratio > 0.9 {
                tracing::warn!(ratio, "Poor compression");
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

#[derive(Clone, Copy, Debug)]
pub enum ContentType {
    Html,
    Js,
    Svg,
    Css,
    Xml,
    Txt,
    Woff,
    Woff2,
    Png,
}

impl TotalSize for ContentType {
    fn total_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

impl ContentType {
    const fn into_header_value(self) -> HeaderValue {
        match self {
            ContentType::Html => HeaderValue::from_static("text/html; charset=utf-8"),
            ContentType::Js => HeaderValue::from_static("application/javascript; charset=utf-8"),
            ContentType::Svg => HeaderValue::from_static("image/svg+xml"),
            ContentType::Css => HeaderValue::from_static("text/css; charset=utf-8"),
            ContentType::Xml => HeaderValue::from_static("application/xml"),
            ContentType::Txt => HeaderValue::from_static("text/plain"),
            ContentType::Woff => HeaderValue::from_static("font/woff"),
            ContentType::Woff2 => HeaderValue::from_static("font/woff2"),
            ContentType::Png => HeaderValue::from_static("image/png"),
        }
    }

    fn is_compressible(self) -> bool {
        match self {
            Self::Html | Self::Js | Self::Svg | Self::Css | Self::Xml | Self::Txt => true,
            Self::Woff | Self::Woff2 | Self::Png => false,
        }
    }

    pub fn from_file_ext(ext: &str) -> Option<Self> {
        let ty = match ext {
            "html" => Self::Html,
            "js" => Self::Js,
            "svg" => Self::Svg,
            "css" => Self::Css,
            "xml" => Self::Xml,
            "txt" => Self::Txt,
            "woff" => Self::Woff,
            "woff2" => Self::Woff2,
            "png" => Self::Png,
            _ => return None,
        };
        Some(ty)
    }
}
