use std::{convert::Infallible, str::FromStr};

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::{HeaderValue, header, request},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Identity,
    Gzip,
    Brotli,
}

impl Encoding {
    pub const ALL_ENCODINGS: HeaderValue = HeaderValue::from_static("gzip, br");

    pub const fn into_content_encoding_value(self) -> Option<HeaderValue> {
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
            // TODO: handle wildcard encoding
            _ => return Err(()),
        };
        Ok(encoding)
    }
}

impl<S> FromRequestParts<S> for Encoding
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        fn from_req_parts(parts: &request::Parts) -> Option<Encoding> {
            let accept_encoding = parts.headers.get(header::ACCEPT_ENCODING)?;
            let accept_encoding = accept_encoding.to_str().ok()?;
            accept_encoding
                .split(',')
                .filter_map(|chunk| {
                    let trimmed = chunk.trim();
                    match trimmed.split_once(';') {
                        // TODO: properly handle non-default encoding qualities
                        Some((_encoding, _quality)) => None,
                        None => Some(trimmed),
                    }
                })
                .filter_map(|encoding| encoding.parse().ok())
                .next()
        }

        let encoding = from_req_parts(&*parts).unwrap_or_default();
        Ok(encoding)
    }
}

// TODO: link github issue about why we use this instead of the default typed header
// `headers::IfNoneMatch` impl
pub struct IfNoneMatch(pub String);

impl<S> OptionalFromRequestParts<S> for IfNoneMatch
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut request::Parts,
        _: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        let maybe_tag = parts
            .headers
            .get(header::IF_NONE_MATCH)
            .and_then(|tag| tag.to_str().ok())
            .map(|tag| Self(tag.to_owned()));
        Ok(maybe_tag)
    }
}
