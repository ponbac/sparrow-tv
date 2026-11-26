//! Anonymous Channel Icon transport and bounded raster decoding for native clients.

pub mod cache;
pub mod disk;
mod raster;

use std::time::Duration;

use reqwest::{
    Client, Response, StatusCode,
    header::{ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, LOCATION},
    redirect::Policy,
};
use thiserror::Error;
use url::{ParseError, Url};

pub use raster::{DecodedChannelIcon, is_contained_icon_decode_panic};

const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Errors disclose no provider location, header, response or decoder detail.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ChannelIconError {
    #[error("invalid Channel Icon location")]
    InvalidLocation,
    #[error("Channel Icon redirect rejected")]
    RedirectRejected,
    #[error("Channel Icon request failed")]
    Network,
    #[error("Channel Icon response rejected")]
    Response,
    #[error("Channel Icon response exceeds the byte limit")]
    BodyTooLarge,
    #[error("Channel Icon format is unsupported")]
    UnsupportedFormat,
    #[error("Channel Icon could not be decoded")]
    InvalidImage,
    #[error("Channel Icon exceeds the image size limit")]
    ImageTooLarge,
}

/// A client with no Source credentials or state. Reuse it across icon requests.
#[derive(Clone)]
pub struct ChannelIconClient(Client);

impl ChannelIconClient {
    pub fn new() -> Result<Self, ChannelIconError> {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::aws_lc_rs::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .map_err(|_| ChannelIconError::Network)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(CONNECT_TIMEOUT)
            .referer(false)
            .redirect(Policy::none())
            .no_proxy()
            // Other workspace members can enable these features for reqwest.
            .no_brotli()
            .no_deflate()
            .no_gzip()
            .no_zstd()
            .build()
            .map(Self)
            .map_err(|_| ChannelIconError::Network)
    }

    /// Dropping the future cancels the request. The deadline covers the entire
    /// redirect chain and body, and all requests require a valid HTTP(S) URL.
    pub async fn fetch(&self, location: &str) -> Result<EncodedChannelIcon, ChannelIconError> {
        let url = absolute_icon_url(location).ok_or(ChannelIconError::InvalidLocation)?;
        tokio::time::timeout(LOAD_TIMEOUT, self.fetch_until_body(url))
            .await
            .map_err(|_| ChannelIconError::Network)?
    }

    async fn fetch_until_body(&self, mut url: Url) -> Result<EncodedChannelIcon, ChannelIconError> {
        let mut remaining_redirects = MAX_REDIRECTS;
        loop {
            let response = self
                .0
                .get(url.clone())
                .header(ACCEPT, "image/png,image/jpeg,image/gif,image/webp")
                .header(ACCEPT_ENCODING, "identity")
                .send()
                .await
                .map_err(|_| ChannelIconError::Network)?;
            if response.status().is_success() {
                return read_body(response).await;
            }
            if !matches!(
                response.status(),
                StatusCode::MOVED_PERMANENTLY
                    | StatusCode::FOUND
                    | StatusCode::SEE_OTHER
                    | StatusCode::TEMPORARY_REDIRECT
                    | StatusCode::PERMANENT_REDIRECT
            ) {
                return Err(ChannelIconError::Response);
            }
            remaining_redirects = remaining_redirects
                .checked_sub(1)
                .ok_or(ChannelIconError::RedirectRejected)?;
            url = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| redirect_url(&url, location))
                .ok_or(ChannelIconError::RedirectRejected)?;
        }
    }
}

/// Image bytes with the response size checked by `ChannelIconClient::fetch`.
pub struct EncodedChannelIcon(Vec<u8>);

impl EncodedChannelIcon {
    /// Decode the first frame on the caller's bounded blocking worker.
    pub fn decode(self) -> Result<DecodedChannelIcon, ChannelIconError> {
        raster::decode(&self.0)
    }
}

async fn read_body(mut response: Response) -> Result<EncodedChannelIcon, ChannelIconError> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY_BYTES as u64)
    {
        return Err(ChannelIconError::BodyTooLarge);
    }
    if response
        .headers()
        .get_all(CONTENT_ENCODING)
        .iter()
        .any(|encoding| !encoding.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(ChannelIconError::Response);
    }
    // Enforce the limit while streaming, including when the server omits a length.
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ChannelIconError::Network)?
    {
        if chunk.len() > MAX_BODY_BYTES - bytes.len() {
            return Err(ChannelIconError::BodyTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(EncodedChannelIcon(bytes))
}

fn absolute_icon_url(location: &str) -> Option<Url> {
    let (scheme, remainder) = location.split_once("://")?;
    let authority = remainder.split(['/', '\\', '?', '#']).next()?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || authority.is_empty()
        || authority.contains('@')
        || !valid_location_text(location)
    {
        return None;
    }
    validated_endpoint(Url::parse(location).ok()?)
}

fn redirect_url(base: &Url, location: &str) -> Option<Url> {
    if location.is_empty() || !valid_location_text(location) {
        return None;
    }
    if location.starts_with("//") {
        return absolute_icon_url(&format!("{}:{location}", base.scheme()));
    }
    // URL parsers can treat slash/backslash combinations as a new authority.
    if location.starts_with('\\') || location.starts_with("/\\") {
        return None;
    }
    match Url::parse(location) {
        Ok(_) => absolute_icon_url(location),
        Err(ParseError::RelativeUrlWithoutBase) => validated_endpoint(base.join(location).ok()?),
        Err(_) => None,
    }
}

fn valid_location_text(location: &str) -> bool {
    !location
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
}

fn validated_endpoint(mut url: Url) -> Option<Url> {
    if matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
    {
        url.set_fragment(None);
        Some(url)
    } else {
        None
    }
}
