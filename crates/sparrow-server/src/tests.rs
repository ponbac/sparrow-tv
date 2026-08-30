mod playback;
mod programmes;
mod search_lanes;

use std::{
    collections::HashMap,
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures_util::stream;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use sparrow_core::{
    ChannelGroupFilter, ChannelQuery, CoreAdapters, PageLimit, PageRequest, RefreshTrigger,
    SourceAccess, SourceAccessError, SourceAccessFailure, SourceByteStream, SourceConfiguration,
    SourceConfigurationInput, SourceKind, SourceRequest, SourceResponse, SparrowCore, SystemClock,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

use crate::{api::ApiError, memory_snapshot_store::MemorySnapshotStore, router};

const PASSWORD: &str = "deployment-password-canary";
const CONFIGURATION_CANARY: &str = "configuration-user:configuration-secret";
const PROVIDER_CANARY: &str = "private-provider.fixture.invalid";
const PLAYBACK_CANARY: &str = "private-media.fixture.invalid";
const SOURCE_LOCATION: &str = "https://configuration-user:configuration-secret@private-provider.fixture.invalid/browse.m3u?token=source-canary";
const EPG_CONFIGURATION_CANARY: &str = "guide-user:guide-secret";
const EPG_PROVIDER_CANARY: &str = "private-guide.fixture.invalid";
const EPG_SOURCE_LOCATION: &str = "https://guide-user:guide-secret@private-guide.fixture.invalid/schedules.xml?token=guide-canary";
const BROWSE_M3U: &[u8] = include_bytes!("../../sparrow-core/tests/fixtures/browse_channels.m3u");
const REORDERED_BROWSE_M3U: &[u8] =
    include_bytes!("../../sparrow-core/tests/fixtures/browse_channels_reordered.m3u");
const PROGRAMME_M3U: &[u8] =
    include_bytes!("../../sparrow-core/tests/fixtures/programme_channels.m3u");
const PROGRAMME_EPG: &[u8] =
    include_bytes!("../../sparrow-core/tests/fixtures/programme_schedules.xml");
const REPLACEMENT_M3U: &[u8] = b"#EXTM3U\n#EXTINF:-1 group-title=\"Replacement\",Replacement Channel\nhttps://private-media.fixture.invalid/replacement.ts\n";

#[tokio::test]
async fn unavailable_api_work_has_a_finite_privacy_safe_error() {
    let router = Router::new().fallback(|| async { ApiError::service_unavailable() });
    let response = send(&router, request(Method::GET, "/", None)).await;

    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json,
        json!({ "error": { "_tag": "service-unavailable" } })
    );
}

#[derive(Clone)]
struct FixtureSource {
    state: Arc<Mutex<HashMap<SourceKind, Result<Bytes, SourceAccessFailure>>>>,
    opens: Arc<AtomicUsize>,
}

impl FixtureSource {
    fn available(m3u: &[u8]) -> Self {
        Self::available_sources(m3u, None)
    }

    fn available_with_epg(m3u: &[u8], epg: &[u8]) -> Self {
        Self::available_sources(m3u, Some(epg))
    }

    fn available_sources(m3u: &[u8], epg: Option<&[u8]>) -> Self {
        let mut state = HashMap::from([(SourceKind::M3u, Ok(Bytes::copy_from_slice(m3u)))]);
        if let Some(epg) = epg {
            state.insert(SourceKind::Epg, Ok(Bytes::copy_from_slice(epg)));
        }
        Self {
            state: Arc::new(Mutex::new(state)),
            opens: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn unavailable() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::from([(
                SourceKind::M3u,
                Err(SourceAccessFailure::new(SourceAccessError::Unavailable)),
            )]))),
            opens: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn open_count(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    fn set_available(&self, m3u: &[u8]) {
        self.state
            .lock()
            .expect("fixture source state is not poisoned")
            .insert(SourceKind::M3u, Ok(Bytes::copy_from_slice(m3u)));
    }

    fn set_unavailable(&self) {
        self.set_source_unavailable(SourceKind::M3u);
    }

    fn set_source_unavailable(&self, kind: SourceKind) {
        self.state
            .lock()
            .expect("fixture source state is not poisoned")
            .insert(
                kind,
                Err(SourceAccessFailure::new(SourceAccessError::Unavailable)),
            );
    }
}

#[async_trait]
impl SourceAccess for FixtureSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let result = self
            .state
            .lock()
            .expect("fixture source state is not poisoned")
            .get(&request.kind())
            .cloned()
            .unwrap_or_else(|| Err(SourceAccessFailure::new(SourceAccessError::Unavailable)));
        let bytes = result?;
        let declared = u64::try_from(bytes.len()).expect("fixture size fits u64");
        let body: SourceByteStream = Box::pin(stream::once(async move { Ok(bytes) }));
        Ok(SourceResponse::new(Some(declared), body))
    }
}

struct TestApp {
    router: Router,
    core: Arc<SparrowCore>,
    _app_root: TempDir,
}

impl TestApp {
    async fn fixture(m3u: &[u8]) -> Self {
        let core = configured_core(
            FixtureSource::available(m3u),
            Arc::new(MemorySnapshotStore::default()),
        )
        .await;
        Self::with_core(core)
    }

    async fn fixture_with_guide(m3u: &[u8], epg: &[u8]) -> Self {
        let core = configured_core_with_configuration(
            FixtureSource::available_with_epg(m3u, epg),
            Arc::new(MemorySnapshotStore::default()),
            source_configuration_with_epg(),
        )
        .await;
        Self::with_core(core)
    }

    fn with_core(core: Arc<SparrowCore>) -> Self {
        let app_root = tempfile::tempdir().expect("temporary app root is available");
        fs::write(
            app_root.path().join("index.html"),
            "<!doctype html><title>Sparrow fixture app</title>",
        )
        .expect("fixture index can be written");
        let router = router(Arc::clone(&core), PASSWORD, app_root.path())
            .expect("fixture deployment password is valid");
        Self {
            router,
            core,
            _app_root: app_root,
        }
    }
}

#[tokio::test]
async fn health_root_and_static_app_have_the_required_authentication_seam() {
    let app = TestApp::fixture(BROWSE_M3U).await;

    let health = send(&app.router, request(Method::GET, "/health", None)).await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(health.json, json!({ "status": "ok" }));

    let root = send(&app.router, request(Method::GET, "/", None)).await;
    assert_eq!(root.status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(root.headers.get(header::LOCATION).unwrap(), "/app/");

    for uri in [
        "/app",
        "/app/",
        "/app/unknown",
        "/api/v1",
        "/api/v1/status",
        "/api/v1/search?term=news&channelLimit=10&programmeLimit=10",
        "/api/v1/channels/not-an-id/schedule?limit=10",
        "/api/v1/unknown",
    ] {
        let unauthorized = send(&app.router, request(Method::GET, uri, None)).await;
        assert_authentication_required(&unauthorized);
    }

    let wrong = send(
        &app.router,
        request(Method::GET, "/app/", Some("wrong-password-canary")),
    )
    .await;
    assert_authentication_required(&wrong);
    assert!(!wrong.text.contains(PASSWORD));
    assert!(!wrong.text.contains("wrong-password-canary"));

    let index = send(
        &app.router,
        request_with_scheme(Method::GET, "/app/", PASSWORD, "basic"),
    )
    .await;
    assert_eq!(index.status, StatusCode::OK);
    assert!(index.text.contains("Sparrow fixture app"));

    let fallback = send(
        &app.router,
        request(Method::GET, "/app/catalog/deep-link", Some(PASSWORD)),
    )
    .await;
    assert_eq!(fallback.status, StatusCode::OK);
    assert!(fallback.text.contains("Sparrow fixture app"));
}

#[tokio::test]
async fn malformed_and_oversized_credentials_fail_closed_without_diagnostics() {
    let app = TestApp::fixture(BROWSE_M3U).await;
    let credentials = [
        "Bearer deployment-password-canary".to_owned(),
        "Basic not-base64!".to_owned(),
        format!("Basic {}", STANDARD.encode(format!("other:{PASSWORD}"))),
        format!("Basic {}", "A".repeat(2049)),
    ];
    for authorization in credentials {
        let request = Request::builder()
            .uri("/api/v1/status")
            .header(header::AUTHORIZATION, &authorization)
            .body(Body::empty())
            .expect("fixture authorization is representable");
        let response = send(&app.router, request).await;
        assert_authentication_required(&response);
        assert!(!response.text.contains(PASSWORD));
        assert!(!response.text.contains(&authorization));
    }

    assert_eq!(
        router(Arc::clone(&app.core), b"", app._app_root.path())
            .expect_err("an empty deployment password is rejected"),
        crate::RouterBuildError::MissingPassword
    );
    assert_eq!(
        router(
            Arc::clone(&app.core),
            vec![b'x'; 1025],
            app._app_root.path(),
        )
        .expect_err("an oversized deployment password is rejected"),
        crate::RouterBuildError::PasswordTooLong
    );
    let debug = format!(
        "{:?}",
        crate::auth::DeploymentCredential::new(PASSWORD.as_bytes())
            .expect("fixture password is valid")
    );
    assert!(!debug.contains(PASSWORD));
}

#[tokio::test]
async fn capabilities_status_and_browse_match_the_real_core_fixture() {
    let app = TestApp::fixture(BROWSE_M3U).await;

    let capabilities = get_json(&app.router, "/api/v1/capabilities").await;
    assert_eq!(
        capabilities,
        json!({
            "sourceConfiguration": "deployment-readonly",
            "playbackTransport": "same-origin-http",
            "audioTrackSelection": false,
            "mpvFailover": false,
        })
    );

    let status = get_json(&app.router, "/api/v1/status").await;
    assert_eq!(
        status["configuration"],
        json!({
            "configured": true,
            "epgConfigured": false,
        })
    );
    assert_eq!(status["epg"], Value::Null);
    assert!(status["generation"].as_u64().is_some_and(|value| value > 0));
    assert_eq!(status["m3u"]["_tag"], "fresh");

    let direct_groups = app
        .core
        .list_groups(PageRequest::first(page_limit(2)))
        .expect("fixture groups are available");
    let groups = get_json(&app.router, "/api/v1/groups?limit=2").await;
    assert_eq!(groups["generation"], direct_groups.generation().get());
    assert_eq!(
        groups["items"],
        Value::Array(
            direct_groups
                .items()
                .iter()
                .map(|group| json!({
                    "name": group.name(),
                    "channelCount": group.channel_count(),
                }))
                .collect()
        )
    );
    assert_eq!(
        groups["next"],
        direct_groups
            .next()
            .map(|cursor| json!(cursor.as_str()))
            .unwrap_or(Value::Null)
    );

    let direct_channels = app
        .core
        .list_channels(ChannelQuery::in_group(
            ChannelGroupFilter::parse("News").expect("fixture group filter is valid"),
            PageRequest::first(page_limit(10)),
        ))
        .expect("fixture Channels are available");
    let channels = get_json(&app.router, "/api/v1/channels?limit=10&group=News").await;
    assert_eq!(channels["generation"], direct_channels.generation().get());
    assert_eq!(
        channels["items"],
        Value::Array(
            direct_channels
                .items()
                .iter()
                .map(|channel| json!({
                    "id": channel.id().as_str(),
                    "name": channel.name(),
                    "group": channel.group(),
                }))
                .collect()
        )
    );

    let selected = &direct_channels.items()[0];
    let details = get_json(
        &app.router,
        &format!("/api/v1/channels/{}", selected.id().as_str()),
    )
    .await;
    let direct_details = app
        .core
        .channel(selected.id())
        .expect("fixture Channel exists");
    assert_eq!(
        details,
        json!({
            "id": direct_details.id().as_str(),
            "name": direct_details.name(),
            "group": direct_details.group(),
        })
    );
}

#[tokio::test]
async fn query_and_identifier_failures_are_bounded_typed_responses() {
    let app = TestApp::fixture(BROWSE_M3U).await;
    for (uri, field, reason) in [
        ("/api/v1/groups?limit=0", "page-limit", "out-of-range"),
        ("/api/v1/groups?limit=101", "page-limit", "out-of-range"),
        ("/api/v1/groups?limit=nope", "page-limit", "invalid-format"),
        (
            "/api/v1/groups?cursor=not-a-cursor",
            "page-cursor",
            "invalid-format",
        ),
        ("/api/v1/groups?unknown=1", "query", "invalid-format"),
        ("/api/v1/groups?limit=1&limit=2", "query", "invalid-format"),
        (
            "/api/v1/channels?group=News%0A",
            "channel-group",
            "contains-control-character",
        ),
        ("/api/v1/channels/not-an-id", "channel-id", "invalid-format"),
    ] {
        let response = send(&app.router, request(Method::GET, uri, Some(PASSWORD))).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(
            response.json,
            json!({
                "error": {
                    "_tag": "invalid-input",
                    "field": field,
                    "reason": reason,
                }
            }),
            "{uri}"
        );
    }

    let ungrouped = get_json(&app.router, "/api/v1/channels?group=&limit=10").await;
    assert_eq!(ungrouped["items"].as_array().unwrap().len(), 1);
    assert_eq!(ungrouped["items"][0]["group"], "");

    let valid_missing_id = format!("ch1_{}", "0".repeat(64));
    for uri in [
        format!("/api/v1/channels/{valid_missing_id}"),
        format!("/api/v1/channels/{valid_missing_id}/schedule?limit=10"),
    ] {
        let missing = send(&app.router, request(Method::GET, &uri, Some(PASSWORD))).await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        assert_eq!(
            missing.json,
            json!({
                "error": { "_tag": "not-found", "resource": "channel" }
            })
        );
        assert!(!missing.text.contains(&valid_missing_id));
    }
}

#[tokio::test]
async fn stale_not_configured_and_unavailable_are_ordinary_client_errors() {
    let first = TestApp::fixture(BROWSE_M3U).await;
    let first_page = get_json(&first.router, "/api/v1/groups?limit=1").await;
    let cursor = first_page["next"]
        .as_str()
        .expect("fixture first page has a cursor");

    let changed = TestApp::fixture(REORDERED_BROWSE_M3U).await;
    let stale = send(
        &changed.router,
        request(
            Method::GET,
            &format!("/api/v1/groups?limit=1&cursor={cursor}"),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.json["error"]["_tag"], "stale-cursor");
    assert_eq!(
        stale.json["error"]["current"],
        changed.core.status().generation().unwrap().get()
    );

    let unconfigured = Arc::new(
        SparrowCore::bootstrap(
            None,
            adapters(
                FixtureSource::available(BROWSE_M3U),
                Arc::new(MemorySnapshotStore::default()),
            ),
        )
        .await
        .expect("an unconfigured core is usable"),
    );
    let unconfigured = TestApp::with_core(unconfigured);
    let status = send(
        &unconfigured.router,
        request(Method::GET, "/api/v1/status", Some(PASSWORD)),
    )
    .await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.json["configuration"]["configured"], false);
    for uri in [
        "/api/v1/groups",
        "/api/v1/search?term=news&channelLimit=10&programmeLimit=10",
    ] {
        let response = send(
            &unconfigured.router,
            request(Method::GET, uri, Some(PASSWORD)),
        )
        .await;
        assert_eq!(response.status, StatusCode::CONFLICT);
        assert_eq!(
            response.json,
            json!({ "error": { "_tag": "not-configured" } })
        );
    }

    let unavailable = TestApp::with_core(
        configured_core(
            FixtureSource::unavailable(),
            Arc::new(MemorySnapshotStore::default()),
        )
        .await,
    );
    let status = send(
        &unavailable.router,
        request(Method::GET, "/api/v1/status", Some(PASSWORD)),
    )
    .await;
    assert_eq!(status.status, StatusCode::OK);
    for uri in [
        "/api/v1/channels",
        "/api/v1/search?term=news&channelLimit=10&programmeLimit=10",
    ] {
        let response = send(
            &unavailable.router,
            request(Method::GET, uri, Some(PASSWORD)),
        )
        .await;
        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.json["error"]["_tag"], "catalog-unavailable");
        assert_eq!(response.json["error"]["status"]["m3u"]["_tag"], "failed");
        assert_eq!(
            response.json["error"]["status"]["m3u"]["failure"],
            json!({ "_tag": "source-access" })
        );
    }
}

#[tokio::test]
async fn same_origin_responses_never_enable_cors_or_expose_private_values() {
    let app = TestApp::fixture(BROWSE_M3U).await;
    let mut cors_request = request(Method::GET, "/api/v1/channels?limit=100", Some(PASSWORD));
    cors_request.headers_mut().insert(
        header::ORIGIN,
        "https://attacker.fixture.invalid".parse().unwrap(),
    );
    let channels = send(&app.router, cors_request).await;
    assert_eq!(channels.status, StatusCode::OK);
    assert_no_cors(&channels.headers);

    let mut preflight = request(Method::OPTIONS, "/api/v1/channels", Some(PASSWORD));
    preflight.headers_mut().insert(
        header::ORIGIN,
        "https://attacker.fixture.invalid".parse().unwrap(),
    );
    preflight.headers_mut().insert(
        header::ACCESS_CONTROL_REQUEST_METHOD,
        "GET".parse().unwrap(),
    );
    let preflight = send(&app.router, preflight).await;
    assert_eq!(preflight.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_no_cors(&preflight.headers);

    for uri in [
        "/api/v1/status",
        "/api/v1/groups?limit=100",
        "/api/v1/channels?limit=100",
        "/api/v1/channels/not-an-id",
    ] {
        let response = send(&app.router, request(Method::GET, uri, Some(PASSWORD))).await;
        for canary in [
            PASSWORD,
            CONFIGURATION_CANARY,
            PROVIDER_CANARY,
            PLAYBACK_CANARY,
            "source-canary",
            "browse-canary",
            "https://",
        ] {
            assert!(!response.text.contains(canary), "{uri} leaked {canary}");
        }
    }
}

#[tokio::test]
async fn process_memory_store_recovers_the_published_catalog_without_provider_access() {
    let snapshots = Arc::new(MemorySnapshotStore::default());
    let online = FixtureSource::available(BROWSE_M3U);
    let first = SparrowCore::bootstrap(
        Some(source_configuration()),
        adapters(online, Arc::clone(&snapshots)),
    )
    .await
    .expect("the online catalog bootstraps");
    let first_generation = first.status().generation().unwrap();

    let offline = FixtureSource::unavailable();
    let restarted = SparrowCore::bootstrap(
        Some(source_configuration()),
        adapters(offline.clone(), snapshots),
    )
    .await
    .expect("the process snapshot is recoverable");
    assert_eq!(restarted.status().generation(), Some(first_generation));
    assert_eq!(
        restarted
            .list_channels(ChannelQuery::all(PageRequest::first(page_limit(100))))
            .expect("recovered Channels are available")
            .items()
            .len(),
        8
    );
    assert_eq!(offline.open_count(), 0);
}

#[tokio::test]
async fn process_memory_store_protects_a_validated_fallback_after_adoption_repair_fails() {
    let snapshots = Arc::new(MemorySnapshotStore::default());
    let source = FixtureSource::available(BROWSE_M3U);
    let first = SparrowCore::bootstrap(
        Some(source_configuration()),
        adapters(source.clone(), Arc::clone(&snapshots)),
    )
    .await
    .expect("the initial catalog bootstraps");
    let fallback_generation = first.status().generation().unwrap();

    source.set_available(REORDERED_BROWSE_M3U);
    first.refresh(RefreshTrigger::Manual).await;
    assert_ne!(first.status().generation(), Some(fallback_generation));
    drop(first);

    snapshots
        .corrupt_active_payload()
        .expect("the active fixture candidate is corrupted");
    snapshots
        .fail_next_adoption()
        .expect("the fallback adoption fault is armed");
    source.set_unavailable();
    let recovered = SparrowCore::bootstrap(
        Some(source_configuration()),
        adapters(source.clone(), Arc::clone(&snapshots)),
    )
    .await
    .expect("the parse-valid fallback remains usable when repair fails");
    assert_eq!(recovered.status().generation(), Some(fallback_generation));

    source.set_available(REPLACEMENT_M3U);
    recovered.refresh(RefreshTrigger::Manual).await;
    let replacement = recovered
        .list_channels(ChannelQuery::all(PageRequest::first(page_limit(10))))
        .expect("refresh publishes over the protected fallback");
    assert_eq!(replacement.items().len(), 1);
    assert_eq!(replacement.items()[0].name(), "Replacement Channel");

    assert_eq!(
        snapshots
            .retained_snapshot_count()
            .expect("retained candidates remain readable"),
        2
    );
}

async fn configured_core(
    source: FixtureSource,
    snapshots: Arc<MemorySnapshotStore>,
) -> Arc<SparrowCore> {
    configured_core_with_configuration(source, snapshots, source_configuration()).await
}

async fn configured_core_with_configuration(
    source: FixtureSource,
    snapshots: Arc<MemorySnapshotStore>,
    configuration: SourceConfiguration,
) -> Arc<SparrowCore> {
    Arc::new(
        SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots))
            .await
            .expect("the fixture core bootstraps"),
    )
}

fn source_configuration() -> SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        SOURCE_LOCATION,
        None::<String>,
    ))
    .expect("the fixture source configuration is valid")
}

fn source_configuration_with_epg() -> SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        SOURCE_LOCATION,
        Some(EPG_SOURCE_LOCATION),
    ))
    .expect("the enriched fixture source configuration is valid")
}

fn adapters(source: FixtureSource, snapshots: Arc<MemorySnapshotStore>) -> CoreAdapters {
    CoreAdapters::new(Arc::new(source), snapshots, Arc::new(SystemClock))
}

fn page_limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("fixture page limit is valid")
}

fn channel_id_named<'a>(channels: &'a Value, name: &str) -> &'a str {
    channels["items"]
        .as_array()
        .expect("the Channel page has items")
        .iter()
        .find(|channel| channel["name"] == name)
        .and_then(|channel| channel["id"].as_str())
        .expect("the named fixture Channel exists")
}

fn request(method: Method, uri: &str, password: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(password) = password {
        builder = builder.header(header::AUTHORIZATION, basic(password, "Basic"));
    }
    builder
        .body(Body::empty())
        .expect("fixture request is valid")
}

fn request_with_scheme(method: Method, uri: &str, password: &str, scheme: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, basic(password, scheme))
        .body(Body::empty())
        .expect("fixture request is valid")
}

fn basic(password: &str, scheme: &str) -> String {
    format!(
        "{scheme} {}",
        STANDARD.encode(format!("sparrow:{password}"))
    )
}

struct ObservedResponse {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
    json: Value,
}

async fn send(app: &Router, request: Request<Body>) -> ObservedResponse {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the infallible router responds");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the response body is readable")
        .to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("fixture responses are UTF-8");
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    ObservedResponse {
        status,
        headers,
        text,
        json,
    }
}

async fn get_json(app: &Router, uri: &str) -> Value {
    let response = send(app, request(Method::GET, uri, Some(PASSWORD))).await;
    assert_eq!(response.status, StatusCode::OK, "{uri}: {}", response.text);
    response.json
}

fn assert_authentication_required(response: &ObservedResponse) {
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.json,
        json!({
            "error": { "_tag": "authentication-required" }
        })
    );
    assert_eq!(
        response.headers.get(header::WWW_AUTHENTICATE).unwrap(),
        "Basic realm=\"sparrow\", charset=\"UTF-8\""
    );
}

fn assert_no_cors(headers: &HeaderMap) {
    assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_METHODS));
    assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_HEADERS));
}

fn assert_invalid_input(response: &ObservedResponse, field: &str, reason: &str) {
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.text
    );
    assert_eq!(
        response.json,
        json!({
            "error": {
                "_tag": "invalid-input",
                "field": field,
                "reason": reason,
            }
        })
    );
}
