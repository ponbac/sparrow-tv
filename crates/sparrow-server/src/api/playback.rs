use std::{
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    extract::{Path, RawQuery, State, rejection::PathRejection},
    http::{HeaderValue, Response, StatusCode, header},
};
use futures_util::Stream;
use sparrow_core::PlaybackActivityLease;
use sparrow_source_http::{PlaybackByteStream, PlaybackReadError};

use super::{ApiError, AppState, query::channel_id};

pub(crate) async fn play(
    State(state): State<AppState>,
    path: Result<Path<String>, PathRejection>,
    RawQuery(raw_query): RawQuery,
) -> Result<Response<Body>, ApiError> {
    if raw_query.is_some() {
        return Err(ApiError::invalid("query", "invalid-format"));
    }

    let id = channel_id(path)?;
    // Admission must precede resolution so automatic refresh and Playback
    // Session start have a total order. The guarded body retains this lease
    // until downstream cancellation or completion drops the provider stream.
    let activity = state.core().begin_playback_activity();
    let source = state.core().resolve_playback(&id).map_err(ApiError::from)?;
    let upstream = state
        .playback()
        .open(source)
        .await
        .map_err(ApiError::from)?;
    let body = Body::from_stream(PlaybackBodyStream::new(upstream.into_body(), activity));

    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

/// Couples downstream response ownership to both upstream I/O and the core's
/// playback-activity fact. No producer task can outlive this stream.
struct PlaybackBodyStream {
    // Fields are deliberately ordered so provider I/O drops before the core is
    // told that playback became inactive.
    upstream: PlaybackByteStream,
    _activity: PlaybackActivityLease,
}

impl PlaybackBodyStream {
    fn new(upstream: PlaybackByteStream, activity: PlaybackActivityLease) -> Self {
        Self {
            upstream,
            _activity: activity,
        }
    }
}

impl Stream for PlaybackBodyStream {
    type Item = Result<bytes::Bytes, PlaybackReadError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().upstream.as_mut().poll_next(context)
    }
}
