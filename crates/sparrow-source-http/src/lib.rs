//! Production HTTP adapter for Sparrow's private source-access seam.

mod playback;

use std::{
    error::Error as _,
    fmt::{self, Debug, Formatter},
    io,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use reqwest::{
    Client, ClientBuilder, RequestBuilder, StatusCode, Url,
    header::{
        ETAG, HeaderMap, HeaderName, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
        RETRY_AFTER,
    },
    redirect,
};
use sparrow_core::{
    PrivateSourceValidators, SourceAccess, SourceAccessError, SourceAccessFailure,
    SourceByteStream, SourceReadError, SourceRequest, SourceResponse,
};
use thiserror::Error;

pub use playback::{
    HttpPlaybackAccess, HttpPlaybackAccessBuildError, PlaybackAccessError, PlaybackByteStream,
    PlaybackReadError, PlaybackResponse,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

/// A shared, streaming HTTP adapter with bounded network waits.
///
/// Source locations, URL credentials, query strings, and validators are never
/// retained by this value and are intentionally omitted from its diagnostics.
#[derive(Clone)]
pub struct HttpSourceAccess {
    client: Client,
}

impl HttpSourceAccess {
    /// Builds the production client with fixed connection and overall request
    /// deadlines. The overall deadline includes streaming the response body.
    pub fn new() -> Result<Self, HttpSourceAccessBuildError> {
        let client = production_client_builder()
            .build()
            .map_err(|_| HttpSourceAccessBuildError)?;
        Ok(Self { client })
    }

    async fn fetch(
        &self,
        location: &str,
        validators: &PrivateSourceValidators,
    ) -> Result<HttpFetch, HttpFailure> {
        let location = Url::parse(location).map_err(|_| HttpFailure::invalid_response())?;
        if !matches!(location.scheme(), "http" | "https") || !location.has_host() {
            return Err(HttpFailure::invalid_response());
        }

        let request = apply_conditionals(self.client.get(location), validators)?;
        let response = request.send().await.map_err(map_request_error)?;
        let status = response.status();

        match status {
            StatusCode::OK => {
                let declared_decoded_length = response.content_length();
                let validators = response_validators(response.headers())?;
                let decoded_body: SourceByteStream = Box::pin(
                    response
                        .bytes_stream()
                        .map_err(|error| map_body_error(&error)),
                );
                Ok(HttpFetch::Modified {
                    declared_decoded_length,
                    decoded_body,
                    validators,
                })
            }
            StatusCode::NOT_MODIFIED if validators.is_empty() => {
                Err(HttpFailure::invalid_response())
            }
            StatusCode::NOT_MODIFIED => Ok(HttpFetch::NotModified {
                validators: response_validators(response.headers())?,
            }),
            _ => Err(status_failure(
                status,
                response.headers(),
                SystemTime::now(),
            )),
        }
    }
}

impl Debug for HttpSourceAccess {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpSourceAccess")
            .finish_non_exhaustive()
    }
}

/// A safe client-construction failure. The provider/library error is discarded
/// because it can include proxy configuration or other private environment data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("the HTTP source adapter could not be initialized")]
pub struct HttpSourceAccessBuildError;

#[async_trait]
impl SourceAccess for HttpSourceAccess {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        match self
            .fetch(request.expose_location_for_access(), request.validators())
            .await
            .map_err(HttpFailure::into_source_failure)?
        {
            HttpFetch::Modified {
                declared_decoded_length,
                decoded_body,
                validators,
            } => Ok(SourceResponse::modified(
                declared_decoded_length,
                decoded_body,
                validators,
            )),
            HttpFetch::NotModified { validators } => Ok(SourceResponse::not_modified(validators)),
        }
    }
}

fn production_client_builder() -> ClientBuilder {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        // Unlike the overall deadline, this resets after each received frame
        // and bounds a provider that stalls partway through a large source.
        .read_timeout(READ_TIMEOUT)
        .referer(false)
        .redirect(private_redirect_policy())
}

fn private_redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("redirect limit exceeded");
        }

        let Some(previous) = attempt.previous().last() else {
            return attempt.stop();
        };
        if same_origin(previous, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn apply_conditionals(
    mut request: RequestBuilder,
    validators: &PrivateSourceValidators,
) -> Result<RequestBuilder, HttpFailure> {
    if let Some(etag) = validators.expose_etag() {
        request = request.header(IF_NONE_MATCH, sensitive_header(etag)?);
    }
    if let Some(last_modified) = validators.expose_last_modified() {
        request = request.header(IF_MODIFIED_SINCE, sensitive_header(last_modified)?);
    }
    Ok(request)
}

fn sensitive_header(value: &str) -> Result<HeaderValue, HttpFailure> {
    let mut value = HeaderValue::from_str(value).map_err(|_| HttpFailure::invalid_response())?;
    value.set_sensitive(true);
    Ok(value)
}

fn response_validators(headers: &HeaderMap) -> Result<PrivateSourceValidators, HttpFailure> {
    PrivateSourceValidators::parse(
        response_header(headers, ETAG)?,
        response_header(headers, LAST_MODIFIED)?,
    )
    .map_err(|_| HttpFailure::invalid_response())
}

fn response_header(headers: &HeaderMap, name: HeaderName) -> Result<Option<String>, HttpFailure> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| HttpFailure::invalid_response())
        })
        .transpose()
}

fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    parse_retry_after(headers.get(RETRY_AFTER)?.to_str().ok()?, now)
}

fn status_failure(status: StatusCode, headers: &HeaderMap, now: SystemTime) -> HttpFailure {
    let reason = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => SourceAccessError::Rejected,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => SourceAccessError::TimedOut,
        StatusCode::TOO_MANY_REQUESTS => SourceAccessError::Unavailable,
        status if status.is_client_error() => SourceAccessError::Rejected,
        status if status.is_server_error() => SourceAccessError::Unavailable,
        _ => SourceAccessError::InvalidResponse,
    };
    let retry_after = matches!(
        reason,
        SourceAccessError::Unavailable | SourceAccessError::TimedOut
    )
    .then(|| retry_after(headers, now))
    .flatten();
    HttpFailure::new(reason, retry_after)
}

fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }

    let deadline = httpdate::parse_http_date(value).ok()?;
    Some(deadline.duration_since(now).unwrap_or(Duration::ZERO))
}

fn map_request_error(error: reqwest::Error) -> HttpFailure {
    if error.is_timeout() {
        HttpFailure::timed_out()
    } else if error.is_builder() || error.is_decode() || error.is_redirect() {
        HttpFailure::invalid_response()
    } else {
        HttpFailure::unavailable()
    }
}

fn map_body_error(error: &reqwest::Error) -> SourceReadError {
    // `Response::bytes_stream` categorizes every body error as a decode error,
    // including truncated sockets. Only an InvalidData source proves malformed
    // content-encoding; transport EOF/timeouts remain interruptions.
    let mut source = error.source();
    let mut has_transport_io = false;
    while let Some(cause) = source {
        if let Some(error) = cause.downcast_ref::<io::Error>() {
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
            ) || (error.kind() == io::ErrorKind::UnexpectedEof && error.get_ref().is_none())
            {
                return SourceReadError::InvalidBody;
            }
            has_transport_io = true;
        }
        source = cause.source();
    }
    if error.is_timeout() || has_transport_io {
        SourceReadError::Interrupted
    } else {
        SourceReadError::InvalidBody
    }
}

enum HttpFetch {
    Modified {
        declared_decoded_length: Option<u64>,
        decoded_body: SourceByteStream,
        validators: PrivateSourceValidators,
    },
    NotModified {
        validators: PrivateSourceValidators,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HttpFailure {
    reason: SourceAccessError,
    retry_after: Option<Duration>,
}

impl HttpFailure {
    const fn unavailable() -> Self {
        Self::new(SourceAccessError::Unavailable, None)
    }

    const fn timed_out() -> Self {
        Self::new(SourceAccessError::TimedOut, None)
    }

    const fn invalid_response() -> Self {
        Self::new(SourceAccessError::InvalidResponse, None)
    }

    const fn new(reason: SourceAccessError, retry_after: Option<Duration>) -> Self {
        Self {
            reason,
            retry_after,
        }
    }

    fn into_source_failure(self) -> SourceAccessFailure {
        match self.retry_after {
            Some(delay) => SourceAccessFailure::with_retry_after(self.reason, delay),
            None => SourceAccessFailure::new(self.reason),
        }
    }
}

#[cfg(test)]
mod tests;
