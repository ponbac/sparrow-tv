use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Utc};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use sparrow_core::{
    ChannelQuery, Clock, CoreAdapters, PageRequest, RefreshTrigger, SourceAccessError,
    SourceAccessFailure, SourceKind, SparrowCore,
};
use tokio::time::timeout;
use tower::ServiceExt as _;

use super::*;

const EVENT_TIME: &str = "2026-08-30T12:00:00Z";
const STALE_TIME: &str = "2026-08-30T19:00:00Z";
const REQUEST_MARKER: &str = "x-sparrow-request";

#[tokio::test]
async fn manual_refresh_is_authenticated_csrf_gated_and_accepts_no_inputs() {
    let source = FixtureSource::available(BROWSE_M3U);
    let core = configured_core(source.clone(), Arc::new(MemorySnapshotStore::default())).await;
    settle_tasks().await;
    let app = TestApp::with_core(core);
    let opens_before = source.open_count();

    let unauthorized = send(
        &app.router,
        refresh_request("/api/v1/refresh", None, Some("refresh"), Body::empty()),
    )
    .await;
    assert_authentication_required(&unauthorized);

    for password in [None, Some("wrong-events-password-canary")] {
        let unauthorized_events = send(
            &app.router,
            request(Method::GET, "/api/v1/events", password),
        )
        .await;
        assert_authentication_required(&unauthorized_events);
        assert!(
            !unauthorized_events
                .text
                .contains("wrong-events-password-canary")
        );
        assert_no_cors(&unauthorized_events.headers);
    }
    let queried_events = send(
        &app.router,
        request(
            Method::GET,
            "/api/v1/events?cursor=private-event-query-canary",
            Some(PASSWORD),
        ),
    )
    .await;
    assert_invalid_input(&queried_events, "query", "invalid-format");
    assert!(!queried_events.text.contains("private-event-query-canary"));
    assert_no_cors(&queried_events.headers);

    let mut duplicate = refresh_request(
        "/api/v1/refresh",
        Some(PASSWORD),
        Some("refresh"),
        Body::empty(),
    );
    duplicate
        .headers_mut()
        .append(REQUEST_MARKER, "refresh".parse().unwrap());
    for request in [
        refresh_request("/api/v1/refresh", Some(PASSWORD), None, Body::empty()),
        refresh_request(
            "/api/v1/refresh",
            Some(PASSWORD),
            Some("wrong-marker"),
            Body::empty(),
        ),
        duplicate,
    ] {
        let response = send(&app.router, request).await;
        assert_invalid_input(&response, "header", "invalid-format");
    }

    let query = send(
        &app.router,
        refresh_request(
            "/api/v1/refresh?source=https://private-query-canary.invalid",
            Some(PASSWORD),
            Some("refresh"),
            Body::empty(),
        ),
    )
    .await;
    assert_invalid_input(&query, "query", "invalid-format");
    assert!(!query.text.contains("private-query-canary"));

    let nonempty = send(
        &app.router,
        refresh_request(
            "/api/v1/refresh",
            Some(PASSWORD),
            Some("refresh"),
            Body::from("x"),
        ),
    )
    .await;
    assert_invalid_input(&nonempty, "body", "invalid-format");

    let oversized = send(
        &app.router,
        refresh_request(
            "/api/v1/refresh",
            Some(PASSWORD),
            Some("refresh"),
            Body::from("oversized-private-body-canary"),
        ),
    )
    .await;
    assert_invalid_input(&oversized, "body", "too-long");
    assert!(!oversized.text.contains("private-body-canary"));

    let preflight = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/v1/refresh")
        .header(header::AUTHORIZATION, basic(PASSWORD, "Basic"))
        .header(header::ORIGIN, "https://attacker.fixture.invalid")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, REQUEST_MARKER)
        .body(Body::empty())
        .expect("fixture preflight is valid");
    let preflight = send(&app.router, preflight).await;
    assert_eq!(preflight.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_no_cors(&preflight.headers);
    assert_eq!(source.open_count(), opens_before);

    let accepted = send(
        &app.router,
        refresh_request(
            "/api/v1/refresh",
            Some(PASSWORD),
            Some("refresh"),
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.text);
    assert_eq!(accepted.json["trigger"], "manual");
    assert_eq!(accepted.json["m3u"]["_tag"], "updated");
    assert_eq!(accepted.json["epg"], Value::Null);
    assert_eq!(
        accepted.headers.get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert_eq!(source.open_count(), opens_before + 1);
}

#[tokio::test]
async fn concurrent_manual_requests_share_one_refresh_and_return_typed_completion() {
    let source = FixtureSource::available(BROWSE_M3U);
    let core = configured_core(source.clone(), Arc::new(MemorySnapshotStore::default())).await;
    settle_tasks().await;
    source.set_available(REPLACEMENT_M3U);
    let gate = source.gate_next_open();
    let app = TestApp::with_core(core);
    let opens_before = source.open_count();

    let first = tokio::spawn(send_owned(app.router.clone(), valid_refresh_request()));
    wait_for(|| source.open_count() == opens_before + 1).await;
    let second = tokio::spawn(send_owned(app.router.clone(), valid_refresh_request()));
    settle_tasks().await;
    assert_eq!(source.open_count(), opens_before + 1);

    gate.add_permits(1);
    let first = first.await.expect("first refresh response task completes");
    let second = second
        .await
        .expect("second refresh response task completes");
    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    assert_eq!(first.json, second.json);
    assert_eq!(first.json["trigger"], "manual");
    assert_eq!(first.json["m3u"]["_tag"], "updated");
    assert_eq!(source.open_count(), opens_before + 1);
}

#[tokio::test]
async fn events_emit_immediate_timed_status_publication_completion_and_lag_resync() {
    let clock = FixedClock::at(EVENT_TIME);
    let source = FixtureSource::available(BROWSE_M3U);
    let core = Arc::new(
        SparrowCore::bootstrap(
            Some(source_configuration()),
            CoreAdapters::new(
                Arc::new(source.clone()),
                Arc::new(MemorySnapshotStore::default()),
                Arc::new(clock.clone()),
            ),
        )
        .await
        .expect("fixture core bootstraps"),
    );
    settle_tasks().await;
    let app = TestApp::with_core(Arc::clone(&core));
    let response = app
        .router
        .clone()
        .oneshot(request(Method::GET, "/api/v1/events", Some(PASSWORD)))
        .await
        .expect("the infallible router returns SSE");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert_no_cors(response.headers());
    let mut body = response.into_body();
    let mut buffer = Vec::new();

    let initial = next_sse_json(&mut body, &mut buffer).await;
    assert_eq!(initial["_tag"], "catalog-status-changed");
    assert_eq!(initial["occurredAt"], EVENT_TIME);
    assert_eq!(
        initial["status"]["generation"],
        core.status().generation().unwrap().get()
    );

    source.set_available(REPLACEMENT_M3U);
    let refreshed = send(&app.router, valid_refresh_request()).await;
    assert_eq!(refreshed.status, StatusCode::OK, "{}", refreshed.text);
    let generation = refreshed.json["status"]["generation"]
        .as_u64()
        .expect("refresh publishes a generation");

    let mut observed = Vec::new();
    while !observed
        .iter()
        .any(|event: &Value| event["_tag"] == "refresh-completed")
    {
        observed.push(next_sse_json(&mut body, &mut buffer).await);
    }
    for event in &observed {
        assert_eq!(event["occurredAt"], EVENT_TIME);
        assert_private_markers_absent(&event.to_string());
    }
    let published = observed
        .iter()
        .position(|event| event["_tag"] == "catalog-published")
        .expect("content refresh emits catalog publication");
    let completed = observed
        .iter()
        .position(|event| event["_tag"] == "refresh-completed")
        .expect("refresh emits completion");
    assert!(published < completed);
    assert_eq!(observed[published]["generation"], generation);
    assert_eq!(observed[completed]["source"], "m3u");
    assert_eq!(observed[completed]["outcome"]["_tag"], "updated");

    source.set_unavailable();
    for _ in 0..20 {
        let _ = core.refresh(RefreshTrigger::Manual).await;
    }
    let resync = next_sse_json(&mut body, &mut buffer).await;
    assert_eq!(resync["_tag"], "catalog-status-changed");
    assert_eq!(resync["occurredAt"], EVENT_TIME);
    assert_eq!(resync["status"]["generation"], generation);
    assert_eq!(resync["status"]["m3u"]["_tag"], "failed");
    assert!(
        timeout(
            Duration::from_millis(50),
            next_sse_json_unbounded(&mut body, &mut buffer)
        )
        .await
        .is_err(),
        "lag resynchronization discards obsolete queued events"
    );

    drop(body);
}

#[tokio::test]
async fn events_body_owns_no_detached_producer_or_core_lifetime() {
    let dropped = Arc::new(AtomicBool::new(false));
    let clock = DropProbeClock::new(EVENT_TIME, Arc::clone(&dropped));
    let core = Arc::new(
        SparrowCore::bootstrap(
            Some(source_configuration()),
            CoreAdapters::new(
                Arc::new(FixtureSource::available(BROWSE_M3U)),
                Arc::new(MemorySnapshotStore::default()),
                Arc::new(clock),
            ),
        )
        .await
        .expect("fixture core bootstraps"),
    );
    settle_tasks().await;
    let app = TestApp::with_core(Arc::clone(&core));
    let response = app
        .router
        .clone()
        .oneshot(request(Method::GET, "/api/v1/events", Some(PASSWORD)))
        .await
        .expect("the infallible router returns SSE");
    let mut body = response.into_body();
    let mut buffer = Vec::new();
    let _ = next_sse_json(&mut body, &mut buffer).await;

    drop(core);
    drop(app);
    wait_for(|| dropped.load(Ordering::Acquire)).await;
    assert!(
        timeout(Duration::from_millis(500), body.frame())
            .await
            .expect("closed core promptly closes SSE")
            .is_none(),
        "SSE directly owns only the bounded receiver"
    );
    drop(body);
}

#[tokio::test]
async fn independent_epg_failure_retains_browse_guide_and_playback_resolution() {
    let source = FixtureSource::available_with_epg(PROGRAMME_M3U, PROGRAMME_EPG);
    let core = configured_core_with_configuration(
        source.clone(),
        Arc::new(MemorySnapshotStore::default()),
        source_configuration_with_epg(),
    )
    .await;
    settle_tasks().await;
    source.set_source_available(SourceKind::M3u, PROGRAMME_M3U);
    source.set_source_unavailable(SourceKind::Epg);
    let app = TestApp::with_core(Arc::clone(&core));

    let refresh = send(&app.router, valid_refresh_request()).await;

    assert_eq!(refresh.status, StatusCode::OK, "{}", refresh.text);
    assert_eq!(refresh.json["m3u"]["_tag"], "updated");
    assert_eq!(refresh.json["epg"]["_tag"], "failed");
    assert_eq!(
        refresh.json["epg"]["failure"],
        json!({
            "_tag": "source-access",
            "source": "epg",
            "reason": "unavailable",
            "retryAfterSeconds": null,
        })
    );
    assert_eq!(refresh.json["status"]["m3u"]["_tag"], "fresh");
    assert_eq!(refresh.json["status"]["epg"]["_tag"], "failed");
    assert!(refresh.json["status"]["epg"]["validatedAt"].is_string());
    assert_private_markers_absent(&refresh.text);

    let channels = get_json(&app.router, "/api/v1/channels?limit=10").await;
    assert!(!channels["items"].as_array().unwrap().is_empty());
    let first_id = channel_id_named(&channels, "Misleading Name");
    let schedule = get_json(
        &app.router,
        &format!("/api/v1/channels/{first_id}/schedule?limit=10"),
    )
    .await;
    assert!(
        !schedule["items"].as_array().unwrap().is_empty(),
        "retained EPG remains queryable"
    );
    let page = core
        .list_channels(ChannelQuery::all(PageRequest::first(page_limit(10))))
        .expect("retained catalog remains browsable");
    let _activity = core.begin_playback_activity();
    let _source = core
        .resolve_playback(page.items()[0].id())
        .expect("EPG failure does not block playback resolution");
}

#[tokio::test]
async fn stale_snapshot_and_failed_refresh_keep_catalog_available_without_private_diagnostics() {
    let snapshots = Arc::new(MemorySnapshotStore::default());
    let online = FixtureSource::available(BROWSE_M3U);
    let seeded = SparrowCore::bootstrap(
        Some(source_configuration()),
        CoreAdapters::new(
            Arc::new(online),
            Arc::clone(&snapshots) as Arc<_>,
            Arc::new(FixedClock::at(EVENT_TIME)),
        ),
    )
    .await
    .expect("online fixture seeds a snapshot");
    let generation = seeded.status().generation().unwrap().get();
    drop(seeded);
    settle_tasks().await;

    let offline = FixtureSource::unavailable();
    let recovered = Arc::new(
        SparrowCore::bootstrap(
            Some(source_configuration()),
            CoreAdapters::new(
                Arc::new(offline),
                snapshots,
                Arc::new(FixedClock::at(STALE_TIME)),
            ),
        )
        .await
        .expect("stale snapshot remains usable offline"),
    );
    let status = serde_json::to_value(crate::api::CatalogStatusDto::from(recovered.status()))
        .expect("stale status serializes");
    assert_eq!(status["generation"], generation);
    assert_eq!(status["m3u"]["_tag"], "stale");
    assert_eq!(status["m3u"]["validatedAt"], EVENT_TIME);
    assert_private_markers_absent(&status.to_string());

    settle_tasks().await;
    let app = TestApp::with_core(Arc::clone(&recovered));

    let failed = send(&app.router, valid_refresh_request()).await;
    assert_eq!(failed.status, StatusCode::OK, "{}", failed.text);
    assert_eq!(failed.json["m3u"]["_tag"], "failed");
    assert_eq!(failed.json["status"]["generation"], generation);
    assert_eq!(failed.json["status"]["m3u"]["_tag"], "failed");
    assert_eq!(failed.json["status"]["m3u"]["validatedAt"], EVENT_TIME);
    assert_eq!(
        failed.json["m3u"]["failure"],
        json!({
            "_tag": "source-access",
            "source": "m3u",
            "reason": "unavailable",
            "retryAfterSeconds": null,
        })
    );
    assert_private_markers_absent(&failed.text);

    let page = recovered
        .list_channels(ChannelQuery::all(PageRequest::first(page_limit(100))))
        .expect("failed refresh retains the stale catalog");
    assert_eq!(page.items().len(), 8);
    let _activity = recovered.begin_playback_activity();
    let _source = recovered
        .resolve_playback(page.items()[0].id())
        .expect("failed refresh retains playback resolution");
}

#[tokio::test]
async fn maximum_retry_after_stays_in_the_four_digit_datetime_contract_everywhere() {
    const BROWSER_MAX_INSTANT: &str = "9999-12-31T23:59:59Z";

    let clock = FixedClock::at(EVENT_TIME);
    let source = FixtureSource::available(BROWSE_M3U);
    let core = Arc::new(
        SparrowCore::bootstrap(
            Some(source_configuration()),
            CoreAdapters::new(
                Arc::new(source.clone()),
                Arc::new(MemorySnapshotStore::default()),
                Arc::new(clock),
            ),
        )
        .await
        .expect("fixture core bootstraps"),
    );
    settle_tasks().await;
    source.set_source_failure(
        SourceKind::M3u,
        SourceAccessFailure::with_retry_after(SourceAccessError::Unavailable, Duration::MAX),
    );
    let app = TestApp::with_core(Arc::clone(&core));
    let response = app
        .router
        .clone()
        .oneshot(request(Method::GET, "/api/v1/events", Some(PASSWORD)))
        .await
        .expect("the infallible router returns SSE");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let mut buffer = Vec::new();
    let initial = next_sse_json(&mut body, &mut buffer).await;
    assert_eq!(initial["_tag"], "catalog-status-changed");

    let refresh = send(&app.router, valid_refresh_request()).await;
    assert_eq!(refresh.status, StatusCode::OK, "{}", refresh.text);
    assert_eq!(refresh.json["m3u"]["_tag"], "failed");
    assert_eq!(refresh.json["m3u"]["nextAttemptAt"], BROWSER_MAX_INSTANT);
    assert_eq!(
        refresh.json["status"]["m3u"]["nextAttemptAt"],
        BROWSER_MAX_INSTANT
    );
    assert_eq!(
        refresh.json["m3u"]["failure"]["retryAfterSeconds"],
        9_007_199_254_740_991_u64
    );

    let status = get_json(&app.router, "/api/v1/status").await;
    assert_eq!(status["m3u"]["nextAttemptAt"], BROWSER_MAX_INSTANT);

    let mut observed = Vec::new();
    while !observed
        .iter()
        .any(|event: &Value| event["_tag"] == "refresh-completed" && event["source"] == "m3u")
    {
        observed.push(next_sse_json(&mut body, &mut buffer).await);
    }
    let failed_status = observed
        .iter()
        .find(|event| {
            event["_tag"] == "catalog-status-changed" && event["status"]["m3u"]["_tag"] == "failed"
        })
        .expect("SSE includes the failed status transition");
    assert_eq!(
        failed_status["status"]["m3u"]["nextAttemptAt"],
        BROWSER_MAX_INSTANT
    );
    let completed = observed
        .iter()
        .find(|event| event["_tag"] == "refresh-completed" && event["source"] == "m3u")
        .expect("SSE includes the typed refresh completion");
    assert_eq!(completed["outcome"]["_tag"], "failed");
    assert_eq!(completed["outcome"]["nextAttemptAt"], BROWSER_MAX_INSTANT);
    for event in observed {
        assert_eq!(event["occurredAt"], EVENT_TIME);
    }

    drop(body);
}

#[derive(Clone)]
struct FixedClock {
    now: DateTime<Utc>,
}

impl FixedClock {
    fn at(value: &str) -> Self {
        Self {
            now: DateTime::parse_from_rfc3339(value)
                .expect("fixture instant is valid")
                .with_timezone(&Utc),
        }
    }
}

#[async_trait]
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.now
    }

    async fn wait_until(&self, _deadline: DateTime<Utc>) {
        pending::<()>().await;
    }
}

#[derive(Clone)]
struct DropProbeClock {
    inner: Arc<DropProbeClockInner>,
}

struct DropProbeClockInner {
    now: DateTime<Utc>,
    dropped: Arc<AtomicBool>,
}

impl DropProbeClock {
    fn new(value: &str, dropped: Arc<AtomicBool>) -> Self {
        Self {
            inner: Arc::new(DropProbeClockInner {
                now: DateTime::parse_from_rfc3339(value)
                    .expect("fixture instant is valid")
                    .with_timezone(&Utc),
                dropped,
            }),
        }
    }
}

impl Drop for DropProbeClockInner {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[async_trait]
impl Clock for DropProbeClock {
    fn now(&self) -> DateTime<Utc> {
        self.inner.now
    }

    async fn wait_until(&self, _deadline: DateTime<Utc>) {
        pending::<()>().await;
    }
}

fn valid_refresh_request() -> Request<Body> {
    refresh_request(
        "/api/v1/refresh",
        Some(PASSWORD),
        Some("refresh"),
        Body::empty(),
    )
}

fn refresh_request(
    uri: &str,
    password: Option<&str>,
    marker: Option<&str>,
    body: Body,
) -> Request<Body> {
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    if let Some(password) = password {
        builder = builder.header(header::AUTHORIZATION, basic(password, "Basic"));
    }
    if let Some(marker) = marker {
        builder = builder.header(REQUEST_MARKER, marker);
    }
    builder
        .body(body)
        .expect("fixture refresh request is valid")
}

async fn send_owned(app: axum::Router, request: Request<Body>) -> ObservedResponse {
    send(&app, request).await
}

async fn next_sse_json(body: &mut Body, buffer: &mut Vec<u8>) -> Value {
    timeout(
        Duration::from_secs(2),
        next_sse_json_unbounded(body, buffer),
    )
    .await
    .expect("SSE event arrives")
}

async fn next_sse_json_unbounded(body: &mut Body, buffer: &mut Vec<u8>) -> Value {
    loop {
        if let Some(end) = buffer.windows(2).position(|window| window == b"\n\n") {
            let frame: Vec<_> = buffer.drain(..end + 2).collect();
            let text = String::from_utf8(frame).expect("SSE frame is UTF-8");
            if let Some(data) = text.lines().find_map(|line| line.strip_prefix("data: ")) {
                return serde_json::from_str(data).expect("SSE data is JSON");
            }
        }
        let frame = body
            .frame()
            .await
            .expect("SSE stream remains open")
            .expect("SSE body frame is readable");
        if let Ok(data) = frame.into_data() {
            buffer.extend_from_slice(&data);
        }
    }
}

async fn wait_for(predicate: impl Fn() -> bool) {
    timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fixture condition becomes true");
}

async fn settle_tasks() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

fn assert_private_markers_absent(output: &str) {
    for marker in [
        PASSWORD,
        CONFIGURATION_CANARY,
        PROVIDER_CANARY,
        PLAYBACK_CANARY,
        EPG_CONFIGURATION_CANARY,
        EPG_PROVIDER_CANARY,
        "source-canary",
        "guide-canary",
        "https://",
    ] {
        assert!(!output.contains(marker), "private marker leaked: {marker}");
    }
}
