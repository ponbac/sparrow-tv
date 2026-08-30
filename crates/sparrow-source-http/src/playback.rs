use std::{
    fmt::{self, Debug, Formatter},
    pin::Pin,
    time::Duration,
};

use futures_util::{Stream, TryStreamExt as _};
use reqwest::{
    Client, ClientBuilder, StatusCode, Url,
    header::{ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING},
    redirect,
};
use sparrow_core::ResolvedPlaybackSource;
use thiserror::Error;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PLAYBACK_REDIRECTS: usize = 5;
const ACCEPTED_CONTENT_TYPES: &str = "video/mp2t, application/octet-stream;q=0.9, */*;q=0.1";
const USER_AGENT: &str = concat!("sparrow-tv/", env!("CARGO_PKG_VERSION"));

/// A byte stream whose errors have already been reduced to a privacy-safe value.
pub type PlaybackByteStream =
    Pin<Box<dyn Stream<Item = Result<bytes::Bytes, PlaybackReadError>> + Send + 'static>>;

/// Production HTTP access for a core-resolved Playback Source.
///
/// The adapter accepts no client-provided destination or headers. It opens one
/// bounded HTTP(S) redirect chain with fixed policy and returns only provider
/// bytes across its interface. Credentials are removed on origin changes, and
/// locations, response headers, and library errors remain private.
#[derive(Clone)]
pub struct HttpPlaybackAccess {
    client: Client,
}

impl HttpPlaybackAccess {
    pub fn new() -> Result<Self, HttpPlaybackAccessBuildError> {
        let client = playback_client_builder()
            .build()
            .map_err(|_| HttpPlaybackAccessBuildError)?;
        Ok(Self { client })
    }

    /// Opens a provider stream for a core-resolved Playback Source.
    ///
    /// Only a successful, unencoded response becomes a stream. Error response
    /// bodies and all provider headers are discarded rather than projected to
    /// the hosted client.
    pub async fn open(
        &self,
        source: &ResolvedPlaybackSource,
    ) -> Result<PlaybackResponse, PlaybackAccessError> {
        self.fetch(source.location_for_adapter()).await
    }

    async fn fetch(
        &self,
        location: &reqwest::Url,
    ) -> Result<PlaybackResponse, PlaybackAccessError> {
        let response = self
            .client
            .get(location.clone())
            .header(ACCEPT, ACCEPTED_CONTENT_TYPES)
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(map_request_error)?;

        ensure_success(response.status())?;
        ensure_identity_encoding(response.headers())?;

        let body: PlaybackByteStream = Box::pin(
            response
                .bytes_stream()
                .map_err(|_| PlaybackReadError::Interrupted),
        );
        Ok(PlaybackResponse { body })
    }
}

impl Debug for HttpPlaybackAccess {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpPlaybackAccess")
            .finish_non_exhaustive()
    }
}

/// A successful provider response reduced to its byte stream.
///
/// This value intentionally implements neither `Debug` nor `Display` because
/// it owns the live provider response body.
pub struct PlaybackResponse {
    body: PlaybackByteStream,
}

impl PlaybackResponse {
    pub fn into_body(self) -> PlaybackByteStream {
        self.body
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("the HTTP playback adapter could not be initialized")]
pub struct HttpPlaybackAccessBuildError;

/// Header-stage failures that can still be returned as a typed HTTP response.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PlaybackAccessError {
    #[error("playback access was rejected")]
    Rejected,
    #[error("playback access timed out")]
    TimedOut,
    #[error("playback is unavailable")]
    Unavailable,
    #[error("playback returned an invalid response")]
    InvalidResponse,
}

impl PlaybackAccessError {
    pub const fn retryable(self) -> bool {
        matches!(self, Self::TimedOut | Self::Unavailable)
    }
}

/// Body-stage errors after the successful HTTP response has begun.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PlaybackReadError {
    #[error("the playback body was interrupted")]
    Interrupted,
}

fn playback_client_builder() -> ClientBuilder {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        // Live playback has no overall deadline. The resetting read deadline
        // still releases a provider that stops producing bytes.
        .read_timeout(READ_TIMEOUT)
        .referer(false)
        .redirect(playback_redirect_policy())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .user_agent(USER_AGENT)
}

fn playback_redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        if playback_redirect_is_safe(attempt.previous(), attempt.url()) {
            // Before following an origin change, reqwest removes Authorization,
            // Cookie, Cookie2, Proxy-Authorization, and WWW-Authenticate. Tests
            // pin that behavior because leaking source credentials to a CDN is
            // a security boundary of this adapter.
            attempt.follow()
        } else {
            attempt.error("playback redirect rejected")
        }
    })
}

fn playback_redirect_is_safe(previous: &[Url], next: &Url) -> bool {
    let Some(current) = previous.last() else {
        return false;
    };

    previous.len() <= MAX_PLAYBACK_REDIRECTS
        && matches!(next.scheme(), "http" | "https")
        && next.has_host()
        && !(current.scheme() == "https" && next.scheme() == "http")
        && next.username().is_empty()
        && next.password().is_none()
        && !previous.contains(next)
}

fn ensure_success(status: StatusCode) -> Result<(), PlaybackAccessError> {
    match status {
        StatusCode::OK => Ok(()),
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            Err(PlaybackAccessError::TimedOut)
        }
        StatusCode::TOO_MANY_REQUESTS => Err(PlaybackAccessError::Unavailable),
        status if status.is_client_error() => Err(PlaybackAccessError::Rejected),
        status if status.is_server_error() => Err(PlaybackAccessError::Unavailable),
        _ => Err(PlaybackAccessError::InvalidResponse),
    }
}

fn ensure_identity_encoding(
    headers: &reqwest::header::HeaderMap,
) -> Result<(), PlaybackAccessError> {
    let mut encodings = headers.get_all(CONTENT_ENCODING).iter();
    match (encodings.next(), encodings.next()) {
        (None, None) => Ok(()),
        (Some(encoding), None)
            if encoding
                .to_str()
                .ok()
                .is_some_and(|encoding| encoding.eq_ignore_ascii_case("identity")) =>
        {
            Ok(())
        }
        _ => Err(PlaybackAccessError::InvalidResponse),
    }
}

fn map_request_error(error: reqwest::Error) -> PlaybackAccessError {
    if error.is_timeout() {
        PlaybackAccessError::TimedOut
    } else if error.is_builder() || error.is_decode() || error.is_redirect() {
        PlaybackAccessError::InvalidResponse
    } else {
        PlaybackAccessError::Unavailable
    }
}

#[cfg(test)]
mod tests;
