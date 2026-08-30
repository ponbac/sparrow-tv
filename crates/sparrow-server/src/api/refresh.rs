use std::time::Duration;

use axum::{
    Json,
    body::to_bytes,
    extract::{RawQuery, Request, State},
    http::{HeaderMap, HeaderValue, header},
    response::{Response, Sse, sse::Event, sse::KeepAlive},
};
use futures_util::stream;
use sparrow_core::RefreshTrigger;

use super::{
    ApiError, AppState,
    dto::{CoreEventDto, RefreshReportDto},
    no_store,
};

const REQUEST_MARKER: &str = "x-sparrow-request";
const REQUEST_MARKER_VALUE: &[u8] = b"refresh";
const EMPTY_BODY_INSPECTION_LIMIT: usize = 1;
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

pub(crate) async fn manual(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    request: Request,
) -> Result<Response, ApiError> {
    ensure_no_query(query)?;
    ensure_request_marker(request.headers())?;
    ensure_empty_body(request).await?;

    let report = state.core().refresh(RefreshTrigger::Manual).await;
    Ok(no_store(Json(RefreshReportDto::from(report))))
}

pub(crate) async fn events(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    ensure_no_query(query)?;
    let core_events = state.core().subscribe();
    let events = stream::unfold(core_events, |mut core_events| async move {
        core_events.recv().await.map(|event| {
            (
                Event::default().json_data(CoreEventDto::from(event)),
                core_events,
            )
        })
    });
    let sse = Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(KEEP_ALIVE_INTERVAL)
            .text("keep-alive"),
    );
    let mut response = no_store(sse);
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn ensure_no_query(query: Option<String>) -> Result<(), ApiError> {
    if query.is_some() {
        return Err(ApiError::invalid("query", "invalid-format"));
    }
    Ok(())
}

fn ensure_request_marker(headers: &HeaderMap) -> Result<(), ApiError> {
    let mut values = headers.get_all(REQUEST_MARKER).iter();
    match (values.next(), values.next()) {
        (Some(value), None) if value.as_bytes() == REQUEST_MARKER_VALUE => Ok(()),
        _ => Err(ApiError::invalid("header", "invalid-format")),
    }
}

async fn ensure_empty_body(request: Request) -> Result<(), ApiError> {
    match to_bytes(request.into_body(), EMPTY_BODY_INSPECTION_LIMIT).await {
        Ok(body) if body.is_empty() => Ok(()),
        Ok(_) => Err(ApiError::invalid("body", "invalid-format")),
        Err(_) => Err(ApiError::invalid("body", "too-long")),
    }
}
