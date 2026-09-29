use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream;
use sparrow_core::{
    ChannelQuery, Clock, CoreAdapters, PageLimit, PageRequest, PrivateSourceValidators,
    RefreshOutcome, RefreshTrigger, ScheduleQuery, SnapshotCandidate, SnapshotRevalidation,
    SnapshotScan, SnapshotSource, SnapshotStage, SnapshotStageRequest, SnapshotStore, SourceAccess,
    SourceAccessError, SourceAccessFailure, SourceByteStream, SourceConfiguration,
    SourceConfigurationInput, SourceKind, SourceRequest, SourceResponse, SourceState, SparrowCore,
    StoreError, ValidatedStage,
};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use tempfile::TempDir;

const M3U_URL: &str = "https://private.fixture.invalid/channels.m3u";
const EPG_URL: &str = "https://private.fixture.invalid/guide.xml";
const M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="alpha.id" group-title="News",Alpha
https://media.fixture.invalid/alpha?token=private-playback
"#;
const EPG: &[u8] = br#"<tv>
<channel id="alpha.id"><display-name>Alpha</display-name></channel>
<programme start="20260829120000 +0000" stop="20260829130000 +0000" channel="alpha.id">
<title>Persisted Programme</title>
</programme>
</tv>"#;

#[tokio::test]
async fn public_core_recovers_m3u_and_epg_after_the_production_adapter_restarts() {
    let directory = TempDir::new().expect("temporary directory");
    let online = FixtureSource::online();
    let store = Arc::new(AtomicFileSnapshotStore::open(directory.path()).expect("store opens"));
    let seeded = bootstrap(configuration(M3U_URL, EPG_URL), online.clone(), store).await;

    let seeded_generation = seeded
        .status()
        .generation()
        .expect("online bootstrap publishes a generation");
    assert_eq!(online.opens(SourceKind::M3u), 1);
    assert_eq!(online.opens(SourceKind::Epg), 1);
    assert_eq!(channel_names(&seeded), ["Alpha"]);
    assert_eq!(programme_titles(&seeded), ["Persisted Programme"]);
    drop(seeded);

    // Validators are private persisted metadata. Inspect them only in this
    // adapter test; public Debug/error surfaces remain redacted.
    let m3u_manifest = std::fs::read_to_string(directory.path().join("m3u/slot-a.manifest.json"))
        .expect("M3U manifest persists");
    let epg_manifest = std::fs::read_to_string(directory.path().join("epg/slot-a.manifest.json"))
        .expect("EPG manifest persists");
    assert!(m3u_manifest.contains("m3u-etag-canary"));
    assert!(m3u_manifest.contains("m3u-last-modified-canary"));
    assert!(epg_manifest.contains("epg-etag-canary"));

    let rejecting = FixtureSource::rejecting();
    let restarted = Arc::new(
        AtomicFileSnapshotStore::open(directory.path()).expect("production adapter restarts"),
    );
    let recovered = bootstrap(
        configuration(M3U_URL, EPG_URL),
        rejecting.clone(),
        restarted,
    )
    .await;

    assert_eq!(rejecting.total_opens(), 0);
    assert_eq!(recovered.status().generation(), Some(seeded_generation));
    assert!(matches!(
        recovered.status().m3u(),
        SourceState::Fresh { .. }
    ));
    assert!(matches!(
        recovered.status().epg(),
        Some(SourceState::Fresh { .. })
    ));
    assert_eq!(channel_names(&recovered), ["Alpha"]);
    assert_eq!(programme_titles(&recovered), ["Persisted Programme"]);

    let changed_epg_source = FixtureSource::rejecting();
    let changed_epg_store = Arc::new(
        AtomicFileSnapshotStore::open(directory.path()).expect("adapter opens for changed EPG"),
    );
    let changed_epg = bootstrap(
        configuration(M3U_URL, "https://private.fixture.invalid/other-guide.xml"),
        changed_epg_source.clone(),
        changed_epg_store,
    )
    .await;
    assert_eq!(changed_epg_source.total_opens(), 0);
    assert_eq!(channel_names(&changed_epg), ["Alpha"]);
    assert!(programme_titles(&changed_epg).is_empty());
    assert!(matches!(
        changed_epg.status().epg(),
        Some(SourceState::Unavailable { .. })
    ));

    let changed_m3u_source = FixtureSource::rejecting();
    let changed_m3u_store = Arc::new(
        AtomicFileSnapshotStore::open(directory.path()).expect("adapter opens for changed M3U"),
    );
    let changed_m3u = bootstrap(
        configuration(
            "https://private.fixture.invalid/other-channels.m3u",
            EPG_URL,
        ),
        changed_m3u_source.clone(),
        changed_m3u_store,
    )
    .await;
    assert_eq!(changed_m3u_source.opens(SourceKind::M3u), 1);
    assert_eq!(changed_m3u_source.opens(SourceKind::Epg), 0);
    assert!(changed_m3u.status().generation().is_none());
}

#[tokio::test]
async fn conditional_revalidation_survives_a_production_adapter_restart_without_reactivation() {
    let directory = TempDir::new().expect("temporary directory");
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let online = ConditionalFixtureSource::new();
    let inner = Arc::new(AtomicFileSnapshotStore::open(directory.path()).expect("store opens"));
    let capturing = CapturingSnapshotStore::new(Arc::clone(&inner));
    let core = SparrowCore::bootstrap(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::new(online.clone()),
            Arc::new(capturing.clone()),
            Arc::new(clock.clone()),
        ),
    )
    .await
    .expect("online bootstrap remains usable");

    let generation = core
        .status()
        .generation()
        .expect("bootstrap publishes a catalog");
    let channels = channel_names(&core);
    let programmes = programme_titles(&core);
    let m3u_source = capturing.source(SourceKind::M3u);
    let epg_source = capturing.source(SourceKind::Epg);
    let before_m3u = active_candidate(&inner, m3u_source).await;
    let before_epg = active_candidate(&inner, epg_source).await;
    assert_eq!(capturing.activation_count(), 2);
    assert_eq!(capturing.revalidation_count(), 0);

    clock.set("2026-08-29T13:00:00Z");
    let report = core.refresh(RefreshTrigger::Manual).await;
    assert!(matches!(
        report.m3u(),
        RefreshOutcome::NotModified { validated_at } if *validated_at == clock.now()
    ));
    assert!(matches!(
        report.epg(),
        Some(RefreshOutcome::NotModified { validated_at }) if *validated_at == clock.now()
    ));
    assert_eq!(report.status().generation(), Some(generation));
    assert_eq!(core.status().generation(), Some(generation));
    assert_eq!(channel_names(&core), channels);
    assert_eq!(programme_titles(&core), programmes);
    assert_eq!(capturing.activation_count(), 2);
    assert_eq!(capturing.revalidation_count(), 2);
    assert_eq!(capturing.requests().len(), 2);

    let after_m3u = active_candidate(&inner, m3u_source).await;
    let after_epg = active_candidate(&inner, epg_source).await;
    assert_revalidated_in_place(
        &before_m3u,
        &after_m3u,
        clock.now(),
        "m3u-etag-revalidated",
        Some("m3u-last-modified-canary"),
    );
    assert_revalidated_in_place(
        &before_epg,
        &after_epg,
        clock.now(),
        "epg-etag-revalidated",
        Some("epg-last-modified-revalidated"),
    );
    assert_eq!(online.opens(SourceKind::M3u), 2);
    assert_eq!(online.opens(SourceKind::Epg), 2);
    online.assert_conditional_request(SourceKind::M3u, "m3u-etag-canary");
    online.assert_conditional_request(SourceKind::Epg, "epg-etag-canary");

    drop(core);
    drop(capturing);
    drop(inner);

    let offline = FixtureSource::rejecting();
    let reopened = Arc::new(
        AtomicFileSnapshotStore::open(directory.path()).expect("production adapter reopens"),
    );
    let recovered = SparrowCore::bootstrap(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(Arc::new(offline.clone()), reopened, Arc::new(clock.clone())),
    )
    .await
    .expect("offline restart recovers revalidated snapshots");

    assert_eq!(offline.total_opens(), 0);
    assert_eq!(recovered.status().generation(), Some(generation));
    assert_eq!(channel_names(&recovered), channels);
    assert_eq!(programme_titles(&recovered), programmes);
    assert!(matches!(
        recovered.status().m3u(),
        SourceState::Fresh { validated_at } if *validated_at == clock.now()
    ));
    assert!(matches!(
        recovered.status().epg(),
        Some(SourceState::Fresh { validated_at }) if *validated_at == clock.now()
    ));
}

#[tokio::test]
async fn production_adapter_rejects_unknown_and_wrong_source_protection_handles() {
    let directory = TempDir::new().expect("temporary directory");
    let inner = Arc::new(AtomicFileSnapshotStore::open(directory.path()).expect("store opens"));
    let capturing = CapturingSnapshotStore::new(Arc::clone(&inner));
    let _seeded = bootstrap(
        configuration(M3U_URL, EPG_URL),
        FixtureSource::online(),
        Arc::new(capturing.clone()),
    )
    .await;

    let requests = capturing.requests();
    let m3u = requests
        .iter()
        .find(|request| request.source().kind() == SourceKind::M3u)
        .expect("bootstrap stages M3U")
        .source();
    let epg = requests
        .iter()
        .find(|request| request.source().kind() == SourceKind::Epg)
        .expect("bootstrap stages EPG")
        .source();
    let retained = inner
        .scan_candidates(m3u)
        .await
        .expect("M3U candidates scan")
        .into_candidates()
        .next()
        .expect("M3U candidate persists");

    let unknown = SnapshotCandidate::new(
        u64::MAX,
        retained.metadata().clone(),
        retained.requires_adoption(),
    );
    assert!(matches!(
        inner.begin_stage(SnapshotStageRequest::new(m3u, Some(unknown))),
        Err(StoreError::Corrupt)
    ));
    assert!(matches!(
        inner.begin_stage(SnapshotStageRequest::new(epg, Some(retained.clone()))),
        Err(StoreError::Corrupt)
    ));

    let valid = inner
        .begin_stage(SnapshotStageRequest::new(m3u, Some(retained)))
        .expect("failed forged requests do not reserve a stage");
    inner.discard(valid).expect("valid stage discards");
}

#[derive(Clone)]
struct CapturingSnapshotStore {
    inner: Arc<AtomicFileSnapshotStore>,
    requests: Arc<std::sync::Mutex<Vec<SnapshotStageRequest>>>,
    activations: Arc<AtomicUsize>,
    revalidations: Arc<AtomicUsize>,
    candidate_opens: Arc<AtomicUsize>,
    cache_writes: Arc<AtomicUsize>,
    cache_gate: Option<Arc<CacheWriteGate>>,
}

impl CapturingSnapshotStore {
    fn new(inner: Arc<AtomicFileSnapshotStore>) -> Self {
        Self {
            inner,
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            activations: Arc::new(AtomicUsize::new(0)),
            revalidations: Arc::new(AtomicUsize::new(0)),
            candidate_opens: Arc::new(AtomicUsize::new(0)),
            cache_writes: Arc::new(AtomicUsize::new(0)),
            cache_gate: None,
        }
    }

    fn requests(&self) -> Vec<SnapshotStageRequest> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn source(&self, kind: SourceKind) -> SnapshotSource {
        self.requests()
            .into_iter()
            .find(|request| request.source().kind() == kind)
            .expect("bootstrap stages every configured source")
            .source()
    }

    fn activation_count(&self) -> usize {
        self.activations.load(Ordering::SeqCst)
    }

    fn revalidation_count(&self) -> usize {
        self.revalidations.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SnapshotStore for CapturingSnapshotStore {
    fn catalog_cache_enabled(&self) -> bool {
        true
    }
    fn read_catalog_cache(&self, key: &[u8; 32]) -> Option<Vec<u8>> {
        self.inner.read_catalog_cache(key)
    }
    fn write_catalog_cache(&self, key: &[u8; 32], bytes: &[u8]) {
        if let Some(gate) = &self.cache_gate {
            gate.started.notify_one();
            let ready = gate.ready.lock().unwrap();
            drop(gate.changed.wait_while(ready, |ready| !*ready).unwrap());
        }
        self.inner.write_catalog_cache(key, bytes);
        self.cache_writes.fetch_add(1, Ordering::SeqCst);
    }

    async fn scan_candidates(&self, source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        self.inner.scan_candidates(source).await
    }

    async fn open_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<Box<dyn std::io::BufRead + Send>, StoreError> {
        self.candidate_opens.fetch_add(1, Ordering::SeqCst);
        self.inner.open_candidate(candidate).await
    }

    async fn adopt_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        self.inner.adopt_candidate(candidate).await
    }

    async fn revalidate_candidate(
        &self,
        candidate: &SnapshotCandidate,
        revalidation: &SnapshotRevalidation,
    ) -> Result<SnapshotCandidate, StoreError> {
        let result = self
            .inner
            .revalidate_candidate(candidate, revalidation)
            .await;
        if result.is_ok() {
            self.revalidations.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.clone());
        self.inner.begin_stage(request)
    }

    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError> {
        self.inner.append(stage, chunk).await
    }

    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn std::io::BufRead + Send>, StoreError> {
        self.inner.open_staged(stage).await
    }

    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        self.inner.prepare_activation(validated).await
    }

    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        let result = self.inner.activate(validated);
        if result.is_ok() {
            self.activations.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
        self.inner.discard(stage)
    }
}

#[derive(Clone)]
struct ConditionalFixtureSource {
    m3u_opens: Arc<AtomicUsize>,
    epg_opens: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<(SourceKind, PrivateSourceValidators)>>>,
}

impl ConditionalFixtureSource {
    fn new() -> Self {
        Self {
            m3u_opens: Arc::new(AtomicUsize::new(0)),
            epg_opens: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn opens(&self, kind: SourceKind) -> usize {
        match kind {
            SourceKind::M3u => self.m3u_opens.load(Ordering::SeqCst),
            SourceKind::Epg => self.epg_opens.load(Ordering::SeqCst),
        }
    }

    fn assert_conditional_request(&self, kind: SourceKind, expected_etag: &str) {
        let requests = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let validators = requests
            .iter()
            .filter(|(requested_kind, _)| *requested_kind == kind)
            .nth(1)
            .expect("manual refresh sends a second request")
            .1
            .clone();
        assert_eq!(validators.expose_etag(), Some(expected_etag));
    }
}

#[async_trait]
impl SourceAccess for ConditionalFixtureSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        let kind = request.kind();
        let requested = request.validators().clone();
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((kind, requested.clone()));

        let (payload, initial, revalidated, counter) = match kind {
            SourceKind::M3u => (
                M3U,
                PrivateSourceValidators::parse(
                    Some("m3u-etag-canary".to_owned()),
                    Some("m3u-last-modified-canary".to_owned()),
                )
                .expect("M3U validators are valid"),
                PrivateSourceValidators::parse(Some("m3u-etag-revalidated".to_owned()), None)
                    .expect("revalidated M3U validators are valid"),
                &self.m3u_opens,
            ),
            SourceKind::Epg => (
                EPG,
                PrivateSourceValidators::parse(Some("epg-etag-canary".to_owned()), None)
                    .expect("EPG validators are valid"),
                PrivateSourceValidators::parse(
                    Some("epg-etag-revalidated".to_owned()),
                    Some("epg-last-modified-revalidated".to_owned()),
                )
                .expect("revalidated EPG validators are valid"),
                &self.epg_opens,
            ),
        };
        counter.fetch_add(1, Ordering::SeqCst);

        if requested.is_empty() {
            let body: SourceByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(payload))]));
            return Ok(SourceResponse::with_validators(
                Some(payload.len() as u64),
                body,
                initial,
            ));
        }
        if requested != initial {
            return Err(SourceAccessError::InvalidResponse.into());
        }
        Ok(SourceResponse::not_modified(revalidated))
    }
}

#[derive(Clone)]
struct FixtureSource {
    available: bool,
    channel_name: &'static str,
    m3u_opens: Arc<AtomicUsize>,
    epg_opens: Arc<AtomicUsize>,
}

impl FixtureSource {
    fn online() -> Self {
        Self {
            available: true,
            channel_name: "Alpha",
            m3u_opens: Arc::new(AtomicUsize::new(0)),
            epg_opens: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn rejecting() -> Self {
        Self {
            available: false,
            channel_name: "Alpha",
            m3u_opens: Arc::new(AtomicUsize::new(0)),
            epg_opens: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn opens(&self, kind: SourceKind) -> usize {
        match kind {
            SourceKind::M3u => self.m3u_opens.load(Ordering::SeqCst),
            SourceKind::Epg => self.epg_opens.load(Ordering::SeqCst),
        }
    }

    fn total_opens(&self) -> usize {
        self.opens(SourceKind::M3u) + self.opens(SourceKind::Epg)
    }
}

#[async_trait]
impl SourceAccess for FixtureSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        let (payload, validators, counter) = match request.kind() {
            SourceKind::M3u => (
                M3U,
                PrivateSourceValidators::parse(
                    Some("m3u-etag-canary".to_owned()),
                    Some("m3u-last-modified-canary".to_owned()),
                )
                .expect("M3U validators are valid"),
                &self.m3u_opens,
            ),
            SourceKind::Epg => (
                EPG,
                PrivateSourceValidators::parse(Some("epg-etag-canary".to_owned()), None)
                    .expect("EPG validators are valid"),
                &self.epg_opens,
            ),
        };
        counter.fetch_add(1, Ordering::SeqCst);
        if !self.available {
            return Err(SourceAccessError::Unavailable.into());
        }
        let payload = if request.kind() == SourceKind::M3u {
            Bytes::from(String::from_utf8_lossy(payload).replace("Alpha", self.channel_name))
        } else {
            Bytes::from_static(payload)
        };
        let length = payload.len();
        let body: SourceByteStream = Box::pin(stream::iter([Ok(payload)]));
        Ok(SourceResponse::with_validators(
            Some(length as u64),
            body,
            validators,
        ))
    }
}

struct FixedClock(DateTime<Utc>);

#[async_trait]
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }

    async fn wait_until(&self, _deadline: DateTime<Utc>) {
        std::future::pending().await
    }
}

#[derive(Clone)]
struct ControlledClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl ControlledClock {
    fn at(value: &str) -> Self {
        Self {
            now: Arc::new(Mutex::new(parse_time(value))),
        }
    }

    fn set(&self, value: &str) {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = parse_time(value);
    }
}

#[async_trait]
impl Clock for ControlledClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn wait_until(&self, _deadline: DateTime<Utc>) {
        std::future::pending().await
    }
}

async fn bootstrap<S: SnapshotStore + 'static>(
    configuration: SourceConfiguration,
    source: FixtureSource,
    store: Arc<S>,
) -> SparrowCore {
    let clock = FixedClock(
        DateTime::parse_from_rfc3339("2026-08-29T12:00:00Z")
            .expect("fixture time parses")
            .with_timezone(&Utc),
    );
    SparrowCore::bootstrap(
        Some(configuration),
        CoreAdapters::new(Arc::new(source), store, Arc::new(clock)),
    )
    .await
    .expect("bootstrap remains usable")
}

fn configuration(m3u: &str, epg: &str) -> SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(m3u, Some(epg)))
        .expect("fixture source configuration is valid")
}

fn channel_names(core: &SparrowCore) -> Vec<String> {
    core.list_channels(ChannelQuery::all(PageRequest::first(limit(10))))
        .expect("catalog is queryable")
        .items()
        .iter()
        .map(|channel| channel.name().to_owned())
        .collect()
}

fn programme_titles(core: &SparrowCore) -> Vec<String> {
    let channels = core
        .list_channels(ChannelQuery::all(PageRequest::first(limit(10))))
        .expect("catalog is queryable");
    let channel = channels.items().first().expect("Alpha channel exists");
    core.schedule(ScheduleQuery::new(
        channel.id().clone(),
        PageRequest::first(limit(10)),
    ))
    .expect("schedule is queryable")
    .items()
    .iter()
    .map(|programme| programme.title().to_owned())
    .collect()
}

async fn active_candidate(
    store: &AtomicFileSnapshotStore,
    source: SnapshotSource,
) -> SnapshotCandidate {
    store
        .scan_candidates(source)
        .await
        .expect("production snapshot scan succeeds")
        .into_candidates()
        .next()
        .expect("active candidate persists")
}

fn assert_revalidated_in_place(
    before: &SnapshotCandidate,
    after: &SnapshotCandidate,
    expected_time: DateTime<Utc>,
    expected_etag: &str,
    expected_last_modified: Option<&str>,
) {
    assert_ne!(before.metadata().validated_at(), expected_time);
    assert_eq!(after.token(), before.token());
    assert_eq!(
        after.metadata().decoded_bytes(),
        before.metadata().decoded_bytes()
    );
    assert_eq!(after.metadata().checksum(), before.metadata().checksum());
    assert_eq!(after.metadata().validated_at(), expected_time);
    assert_ne!(
        after.metadata().validators(),
        before.metadata().validators()
    );
    assert_eq!(
        after.metadata().validators().expose_etag(),
        Some(expected_etag)
    );
    assert_eq!(
        after.metadata().validators().expose_last_modified(),
        expected_last_modified
    );
    assert!(!after.requires_adoption());
}

fn parse_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("fixture time parses")
        .with_timezone(&Utc)
}

fn limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("fixture page limit is valid")
}

async fn snapshot_restart(store: Arc<CapturingSnapshotStore>, progressive: bool) -> SparrowCore {
    let adapters = CoreAdapters::new(
        Arc::new(FixtureSource::rejecting()),
        store,
        Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
    );
    if progressive {
        SparrowCore::bootstrap_installed(Some(configuration(M3U_URL, EPG_URL)), adapters).await
    } else {
        SparrowCore::bootstrap_from_snapshots(Some(configuration(M3U_URL, EPG_URL)), adapters).await
    }
    .expect("saved catalog opens offline")
}

#[tokio::test]
async fn processed_catalog_reopens_without_reading_source_payloads_and_recovers_bad_caches() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(disk));
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    let first = snapshot_restart(Arc::clone(&store), false).await;
    let generation = first.status().generation();
    let channel = first
        .list_channels(ChannelQuery::all(PageRequest::first(limit(10))))
        .unwrap()
        .items()[0]
        .id()
        .clone();
    wait_for_cache_write(&store, 0).await;
    drop(first);
    let cache_path = directory.path().join("catalog-cache-v1");
    let cache = std::fs::read(&cache_path).expect("processed catalog is persisted");
    store.candidate_opens.store(0, Ordering::SeqCst);
    let warm = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&warm), ["Alpha"]);
    assert_eq!(programme_titles(&warm), ["Persisted Programme"]);
    assert_eq!(warm.status().generation(), generation);
    assert_eq!(
        store.candidate_opens.load(Ordering::SeqCst),
        0,
        "warm startup must not parse raw sources"
    );
    assert_eq!(warm.channel(&channel).unwrap().name(), "Alpha");
    let results = warm
        .search_channels(
            sparrow_core::SearchTerm::parse("alpha").unwrap(),
            PageRequest::first(limit(10)),
        )
        .unwrap();
    assert_eq!(
        results.items().len(),
        1,
        "persisted search index remains usable"
    );
    drop(warm);

    // Wrong key/version, checksum damage, truncation, and a valid checksum over
    // malformed data all fall back to the still-valid Source Snapshots.
    let mut wrong_key = cache.clone();
    wrong_key[0] ^= 1;
    let mut damaged = cache.clone();
    *damaged.last_mut().unwrap() ^= 1;
    let mut malformed = cache[..32].to_vec();
    malformed.extend_from_slice(blake3::hash(b"{}").as_bytes());
    malformed.extend_from_slice(b"{}");
    for invalid in [wrong_key, damaged, cache[..40].to_vec(), malformed] {
        std::fs::write(&cache_path, invalid).unwrap();
        store.candidate_opens.store(0, Ordering::SeqCst);
        let writes = store.cache_writes.load(Ordering::SeqCst);
        let recovered = snapshot_restart(Arc::clone(&store), false).await;
        wait_for_cache_write(&store, writes).await;
        assert_eq!(programme_titles(&recovered), ["Persisted Programme"]);
        assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 2);
    }
    // Switching Source Configuration must never load the prior owner's catalog.
    let other = SparrowCore::bootstrap_installed(
        Some(configuration("https://other.fixture.invalid/list", EPG_URL)),
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            store,
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert!(other.status().generation().is_none());
}

#[tokio::test]
async fn installed_cache_miss_opens_channels_before_restoring_the_saved_guide() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(disk));
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    let core = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&core), ["Alpha"]);
    assert!(programme_titles(&core).is_empty());
    assert_eq!(
        store.candidate_opens.load(Ordering::SeqCst),
        1,
        "EPG payload must not delay opening Channels"
    );
    let mut events = core.subscribe();
    core.activate_automation();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while programme_titles(&core).is_empty() {
            events.recv().await.unwrap();
        }
    })
    .await
    .expect("saved guide publishes in the background while offline");
    assert_eq!(programme_titles(&core), ["Persisted Programme"]);
}

async fn wait_for_cache_write(store: &CapturingSnapshotStore, previous: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while store.cache_writes.load(Ordering::SeqCst) <= previous {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("background cache write completes");
}

#[tokio::test]
async fn changed_source_bytes_replace_the_processed_catalog_and_survive_restart() {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(CapturingSnapshotStore::new(Arc::new(
        AtomicFileSnapshotStore::open(directory.path()).unwrap(),
    )));
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    let first = snapshot_restart(Arc::clone(&store), false).await;
    wait_for_cache_write(&store, 0).await;
    let old_generation = first.status().generation();
    drop(first);
    let source = FixtureSource {
        channel_name: "Beta",
        ..FixtureSource::online()
    };
    let core = SparrowCore::bootstrap_installed(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::new(source),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert_eq!(channel_names(&core), ["Alpha"]);
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let report = core.refresh(RefreshTrigger::Manual).await;
    assert_ne!(report.status().generation(), old_generation);
    assert_eq!(channel_names(&core), ["Beta"]);
    wait_for_cache_write(&store, writes).await;
    drop(core);
    store.candidate_opens.store(0, Ordering::SeqCst);
    let reopened = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&reopened), ["Beta"]);
    assert_eq!(programme_titles(&reopened), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[derive(Default)]
struct CacheWriteGate {
    started: tokio::sync::Notify,
    ready: Mutex<bool>,
    changed: std::sync::Condvar,
}

struct ReleaseCacheWrite(Arc<CacheWriteGate>);
impl Drop for ReleaseCacheWrite {
    fn drop(&mut self) {
        *self.0.ready.lock().unwrap() = true;
        self.0.changed.notify_all();
    }
}

#[tokio::test]
async fn slow_cache_storage_does_not_delay_installed_startup_or_refresh_completion() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&disk),
        )
        .await,
    );
    let gate = Arc::new(CacheWriteGate::default());
    // Always release a worker even if an assertion fails, so runtime teardown
    // cannot hang while diagnosing a regression.
    let release = ReleaseCacheWrite(Arc::clone(&gate));
    let store = Arc::new(CapturingSnapshotStore {
        cache_gate: Some(Arc::clone(&gate)),
        ..CapturingSnapshotStore::new(disk)
    });
    let config = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        M3U_URL,
        None::<String>,
    ))
    .unwrap();
    let opening = SparrowCore::bootstrap_installed(
        Some(config),
        CoreAdapters::new(
            Arc::new(FixtureSource::online()),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    );
    let core = tokio::time::timeout(std::time::Duration::from_secs(2), opening)
        .await
        .expect("startup never awaits the cache writer")
        .unwrap();
    assert_eq!(channel_names(&core), ["Alpha"]);
    assert_eq!(store.cache_writes.load(Ordering::SeqCst), 0);
    core.activate_automation();
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.started.notified())
        .await
        .unwrap();
    let refresh = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        core.refresh(RefreshTrigger::Manual),
    )
    .await;
    drop(release);
    assert!(
        refresh.is_ok(),
        "a blocked cache write must not block refresh completion"
    );
    wait_for_cache_write(&store, 0).await;
}
