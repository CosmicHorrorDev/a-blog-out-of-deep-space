use crate::extract::Encoding;

use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, header, response},
    response::Response,
};
use axum_extra::headers::{ETag, HeaderMapExt, IfNoneMatch};

pub struct ServedFile {
    pub e_tag: ETag,
    pub ty: ContentType,
    pub file: File,
}

impl ServedFile {
    pub fn to_response(&self, encoding: Encoding, if_none_match: Option<IfNoneMatch>) -> Response {
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

        match if_none_match {
            // Handle e-tag revalidation
            Some(client_tag) if client_tag.precondition_passes(&self.e_tag) => builder
                .status(StatusCode::NOT_MODIFIED)
                .body(Body::empty())
                .unwrap(),
            _ => {
                builder
                    .headers_mut()
                    .unwrap()
                    .typed_insert(self.e_tag.clone());

                match &self.file {
                    File::Data(data_file) => builder.body(data_file.to_owned().into()).unwrap(),
                    File::Text(text_file) => text_file.finish_response(builder, encoding),
                }
            }
        }
    }
}

// TODO: switch this to automaitcally try compressing and bail out if the size isn't better
pub enum File {
    Data(DataFile),
    Text(TextFile),
}

#[derive(Clone)]
pub struct DataFile(Bytes);

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
pub struct TextFile {
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

    pub fn is_compressible(self) -> bool {
        match self {
            Self::Html | Self::Js | Self::Svg | Self::Css | Self::Xml | Self::Txt => true,
            Self::Woff | Self::Woff2 | Self::Ico | Self::Png => false,
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
            "ico" => Self::Ico,
            "png" => Self::Png,
            _ => return None,
        };
        Some(ty)
    }
}
