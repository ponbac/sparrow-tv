use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream;
use sparrow_core::{
    CatalogCacheMapping, ChannelQuery, Clock, CoreAdapters, PageLimit, PageRequest,
    PrivateSourceValidators, RefreshOutcome, RefreshTrigger, ScheduleQuery, SnapshotCandidate,
    SnapshotRevalidation, SnapshotScan, SnapshotSource, SnapshotStage, SnapshotStageRequest,
    SnapshotStore, SourceAccess, SourceAccessError, SourceAccessFailure, SourceByteStream,
    SourceConfiguration, SourceConfigurationInput, SourceKind, SourceRequest, SourceResponse,
    SourceState, SparrowCore, StoreError, ValidatedStage,
};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use tempfile::TempDir;

const M3U_URL: &str = "https://private.fixture.invalid/channels.m3u";
const EPG_URL: &str = "https://private.fixture.invalid/guide.xml";
const M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="alpha.id" tvg-logo="https://icons.example/alpha.png" group-title="News",Alpha
https://media.fixture.invalid/alpha?token=private-playback
"#;
const ALPHA_ICON: &str = "https://icons.example/alpha.png";
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
    cancelled_cache_writes: Arc<AtomicUsize>,
    fail_next_cache_writes: Arc<AtomicUsize>,
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
            cancelled_cache_writes: Arc::new(AtomicUsize::new(0)),
            fail_next_cache_writes: Arc::new(AtomicUsize::new(0)),
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
    fn open_mapped_catalog_cache(
        &self,
        key: &[u8; 32],
        candidates: &[&SnapshotCandidate],
        validate: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Option<CatalogCacheMapping> {
        self.inner
            .open_mapped_catalog_cache(key, candidates, validate)
    }
    fn write_mapped_catalog_cache(
        &self,
        key: &[u8; 32],
        candidates: &[&SnapshotCandidate],
        bytes: &[u8],
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Option<CatalogCacheMapping> {
        if let Some(gate) = &self.cache_gate {
            gate.started.notify_one();
            let ready = gate.ready.lock().unwrap();
            drop(gate.changed.wait_while(ready, |ready| !*ready).unwrap());
        }
        let remaining_failures = self.fail_next_cache_writes.load(Ordering::SeqCst);
        let mapping = if remaining_failures > 0
            && self
                .fail_next_cache_writes
                .compare_exchange(
                    remaining_failures,
                    remaining_failures - 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            None
        } else {
            self.inner
                .write_mapped_catalog_cache(key, candidates, bytes, is_cancelled)
        };
        if mapping.is_some() {
            self.cache_writes.fetch_add(1, Ordering::SeqCst);
        } else if is_cancelled() {
            self.cancelled_cache_writes.fetch_add(1, Ordering::SeqCst);
        }
        if let Some(gate) = &self.cache_gate {
            gate.finished.notify_one();
        }
        mapping
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

fn channel_icons(core: &SparrowCore) -> Vec<Option<String>> {
    core.list_channels(ChannelQuery::all(PageRequest::first(limit(10))))
        .expect("catalog is queryable")
        .items()
        .iter()
        .map(|channel| channel.icon().map(|icon| icon.as_str().to_owned()))
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
    assert_eq!(channel_icons(&warm), [Some(ALPHA_ICON.to_owned())]);
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
    // A previous processed-catalog format under the current key and a valid
    // checksum: the store accepts the envelope and core rejects the layout.
    let mut previous_format = cache[64..].to_vec();
    previous_format[10] = 3;
    let mut previous_format_envelope = cache[..32].to_vec();
    previous_format_envelope.extend_from_slice(blake3::hash(&previous_format).as_bytes());
    previous_format_envelope.extend_from_slice(&previous_format);
    for (index, invalid) in [
        wrong_key,
        damaged,
        cache[..40].to_vec(),
        malformed,
        previous_format_envelope,
    ]
    .into_iter()
    .enumerate()
    {
        // Replace atomically rather than truncating in place so no background
        // worker holding a mapping to the prior inode can observe truncation.
        let staging = directory.path().join(format!("test-damage-{index}.tmp"));
        std::fs::write(&staging, invalid).unwrap();
        std::fs::rename(&staging, &cache_path).unwrap();
        store.candidate_opens.store(0, Ordering::SeqCst);
        let writes = store.cache_writes.load(Ordering::SeqCst);
        let recovered = snapshot_restart(Arc::clone(&store), false).await;
        wait_for_cache_write(&store, writes).await;
        assert_eq!(programme_titles(&recovered), ["Persisted Programme"]);
        assert_eq!(channel_icons(&recovered), [Some(ALPHA_ICON.to_owned())]);
        assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 2);
        drop(recovered);

        // The rebuilt cache serves the icon again without reading raw sources.
        store.candidate_opens.store(0, Ordering::SeqCst);
        let rewarmed = snapshot_restart(Arc::clone(&store), true).await;
        assert_eq!(channel_icons(&rewarmed), [Some(ALPHA_ICON.to_owned())]);
        assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
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
    finished: tokio::sync::Notify,
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

#[tokio::test]
async fn installed_startup_respects_the_six_hour_freshness_window() {
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
    let seed = snapshot_restart(Arc::clone(&store), false).await;
    wait_for_cache_write(&store, 0).await;
    drop(seed);

    // Reopening 2 hours later (within the 6-hour freshness window) must keep
    // snapshots fresh and avoid re-downloading M3U/EPG over the network.
    let fresh_source = Arc::new(FixtureSource::online());
    let fresh = SparrowCore::bootstrap_installed(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::clone(&fresh_source) as _,
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T14:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        fresh.status().m3u(),
        sparrow_core::SourceState::Fresh { .. }
    ));
    assert!(matches!(
        fresh.status().epg(),
        Some(sparrow_core::SourceState::Fresh { .. })
    ));
    fresh.activate_automation();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(fresh_source.m3u_opens.load(Ordering::SeqCst), 0);
    assert_eq!(fresh_source.epg_opens.load(Ordering::SeqCst), 0);
    drop(fresh);

    // Reopening 7 hours later (past the 6-hour window) marks snapshots stale
    // and refreshes them in the background once automation starts.
    let stale_source = Arc::new(FixtureSource::online());
    let stale = SparrowCore::bootstrap_installed(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::clone(&stale_source) as _,
            store,
            Arc::new(FixedClock(parse_time("2026-08-29T19:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        stale.status().m3u(),
        sparrow_core::SourceState::Stale { .. }
    ));
    stale.activate_automation();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while stale_source.m3u_opens.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("stale installed snapshot refreshes in background");
}

#[tokio::test]
async fn same_length_active_payload_corruption_falls_back_to_previous_slot_even_with_warm_cache() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
    // Seed slot-a with Alpha + EPG, then refresh to slot-b with Beta so slot-a
    // stays intact as the fallback snapshot.
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    let beta_source = FixtureSource {
        channel_name: "Beta",
        ..FixtureSource::online()
    };
    let core = SparrowCore::bootstrap_from_snapshots(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::new(beta_source),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    wait_for_cache_write(&store, 0).await;
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let _ = core.refresh(RefreshTrigger::Manual).await;
    assert_eq!(channel_names(&core), ["Beta"]);
    wait_for_cache_write(&store, writes).await;
    drop(core);

    // Confirm warm cache hits before corruption.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let warm = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&warm), ["Beta"]);
    assert_eq!(programme_titles(&warm), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
    drop(warm);

    // Back up active slot-b payload and pointer (like android-catalog-acceptance.ts),
    // then flip 1 byte in-place inside active m3u/slot-b.payload with NO sleep so
    // even same-tick coarse-clock mutations must be caught deterministically.
    let active_payload = directory.path().join("m3u/slot-b.payload");
    let active_pointer = directory.path().join("m3u/active.json");
    let payload_backup = std::fs::read(&active_payload).unwrap();
    let pointer_backup = std::fs::read(&active_pointer).unwrap();
    let mut corrupted = payload_backup.clone();
    assert!(!corrupted.is_empty());
    corrupted[0] ^= 0xFF;
    std::fs::write(&active_payload, &corrupted).unwrap();

    // Offline restart must detect the corrupted active payload, bypass the warm
    // processed cache, adopt fallback slot-a, and persist a fresh cache for slot-a.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let recovered = snapshot_restart(Arc::clone(&store), false).await;
    assert_eq!(channel_names(&recovered), ["Alpha"]);
    assert_eq!(programme_titles(&recovered), ["Persisted Programme"]);
    assert!(store.candidate_opens.load(Ordering::SeqCst) > 0);
    let pointer = std::fs::read_to_string(&active_pointer).unwrap();
    assert!(
        pointer.contains("\"a\"") || pointer.contains("\"slot\":\"a\""),
        "fallback slot-a should be adopted: {pointer}"
    );
    wait_for_cache_write(&store, writes).await;
    drop(recovered);

    // Simulate android-catalog-acceptance.ts byte-identical restore: copy the
    // original payload and pointer back, leaving slot-b.manifest.json older than
    // slot-b.payload. Two subsequent offline restarts must recover Beta and then
    // hit the warm cache cleanly (warm startup is never permanently disabled).
    let active_manifest = directory.path().join("m3u/slot-b.manifest.json");
    wait_for_fs_tick_after(directory.path(), &[&active_manifest]);
    std::fs::write(&active_payload, &payload_backup).unwrap();
    std::fs::write(&active_pointer, &pointer_backup).unwrap();

    // Offline restart 1 after restore: verifies restored slot-b and refreshes cache.
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let after_restore_1 = snapshot_restart(Arc::clone(&store), false).await;
    assert_eq!(channel_names(&after_restore_1), ["Beta"]);
    assert_eq!(programme_titles(&after_restore_1), ["Persisted Programme"]);
    wait_for_cache_write(&store, writes).await;
    drop(after_restore_1);

    // Offline restart 2 after restore: warm cache hit with zero raw candidate opens.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let after_restore_2 = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&after_restore_2), ["Beta"]);
    assert_eq!(programme_titles(&after_restore_2), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unused_corrupt_epg_snapshot_does_not_veto_m3u_only_warm_cache() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
    // Seed both M3U and EPG snapshots first.
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    // Switch to an M3U-only Source Configuration and persist its warm cache.
    let m3u_only = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        M3U_URL,
        None::<String>,
    ))
    .unwrap();
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let seeded_m3u_only = SparrowCore::bootstrap_from_snapshots(
        Some(m3u_only),
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    wait_for_cache_write(&store, writes).await;
    drop(seeded_m3u_only);

    // Damage the abandoned EPG directory completely.
    std::fs::write(directory.path().join("epg/active.json"), b"{corrupt").unwrap();
    std::fs::remove_file(directory.path().join("epg/slot-a.payload")).unwrap();

    // Reopening the M3U-only configuration must still hit the warm cache.
    let m3u_only = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        M3U_URL,
        None::<String>,
    ))
    .unwrap();
    store.candidate_opens.store(0, Ordering::SeqCst);
    let warm = SparrowCore::bootstrap_installed(
        Some(m3u_only),
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert_eq!(channel_names(&warm), ["Alpha"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[test]
fn deferred_epg_recovery_and_concurrent_m3u_refresh_do_not_deadlock_one_blocking_worker() {
    use std::fmt::Write as _;

    struct LargeM3uSource {
        m3u_payload: Bytes,
        epg_payload: Option<Bytes>,
    }

    #[async_trait]
    impl SourceAccess for LargeM3uSource {
        async fn open(
            &self,
            request: SourceRequest,
        ) -> Result<SourceResponse, SourceAccessFailure> {
            let payload = match request.kind() {
                SourceKind::M3u => self.m3u_payload.clone(),
                SourceKind::Epg => self
                    .epg_payload
                    .clone()
                    .ok_or_else(|| SourceAccessFailure::from(SourceAccessError::Unavailable))?,
            };
            let len = payload.len() as u64;
            Ok(SourceResponse::new(
                Some(len),
                Box::pin(stream::iter([Ok(payload)])),
            ))
        }
    }

    let make_m3u = |prefix: &str| {
        let mut m3u = String::from("#EXTM3U\n");
        // Exceed BLOCKING_OFFLOAD_CHANNELS so publish_m3u offloads onto the
        // Tokio blocking pool while holding `catalog_build`.
        for i in 0..600 {
            writeln!(
                m3u,
                "#EXTINF:-1 tvg-id=\"alpha.id\" group-title=\"News\",{prefix} {i:04}\nhttps://media.fixture.invalid/ch/{i}"
            )
            .unwrap();
        }
        Bytes::from(m3u)
    };

    let make_large_epg = || {
        // Exceed both BLOCKING_OFFLOAD_CHANNELS (64) and BLOCKING_OFFLOAD_BYTES
        // (32 KiB) so deferred EPG recovery MUST offload both `xmltv::parse` and
        // `with_updated_guide` onto the Tokio blocking pool; a `spawn_blocking` +
        // `block_on` wrapper would deterministically self-deadlock on 1 worker.
        let mut xml = String::from(
            "<tv><channel id=\"alpha.id\"><display-name>Alpha</display-name></channel>\n",
        );
        let padding = "x".repeat(512);
        for i in 0..80 {
            let start_min = i * 5;
            let end_min = start_min + 5;
            let sh = 12 + start_min / 60;
            let sm = start_min % 60;
            let eh = 12 + end_min / 60;
            let em = end_min % 60;
            writeln!(
                xml,
                "<programme start=\"20260829{sh:02}{sm:02}00 +0000\" stop=\"20260829{eh:02}{em:02}00 +0000\" channel=\"alpha.id\"><title>Persisted Programme {i:04}</title><desc>{padding}</desc></programme>"
            )
            .unwrap();
        }
        xml.push_str("</tv>\n");
        Bytes::from(xml)
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();

    let outcome: Result<(), String> = rt.block_on(async {
        let directory = TempDir::new().unwrap();
        let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
        let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
        let seed_source = Arc::new(LargeM3uSource {
            m3u_payload: make_m3u("Alpha"),
            epg_payload: Some(make_large_epg()),
        });
        let seeded = SparrowCore::bootstrap(
            Some(configuration(M3U_URL, EPG_URL)),
            CoreAdapters::new(
                seed_source,
                store.clone(),
                Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
            ),
        )
        .await
        .unwrap();
        drop(seeded);
        let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1"));

        // Progressive installed restart defers EPG recovery until activate_automation().
        // Network EPG is disabled so restored programmes can come ONLY from the
        // deferred saved-snapshot recovery path.
        let refresh_source = Arc::new(LargeM3uSource {
            m3u_payload: make_m3u("Beta"),
            epg_payload: None,
        });
        let core = SparrowCore::bootstrap_installed(
            Some(configuration(M3U_URL, EPG_URL)),
            CoreAdapters::new(
                refresh_source,
                store.clone(),
                Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
            ),
        )
        .await
        .unwrap();
        assert!(programme_titles(&core).is_empty());

        // Start deferred EPG recovery and a blocking-offloaded M3U refresh at the
        // same time on a 1-worker blocking pool; both must complete without deadlock.
        core.activate_automation();
        let report = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            core.refresh(RefreshTrigger::Manual),
        )
        .await
        {
            Ok(report) => report,
            Err(_) => {
                return Err(
                    "1-worker blocking pool deadlocked between deferred EPG and M3U refresh"
                        .to_owned(),
                );
            }
        };
        if !matches!(report.m3u(), RefreshOutcome::Updated { .. }) {
            return Err(format!(
                "expected updated M3U outcome, got {:?}",
                report.m3u()
            ));
        }
        if channel_names(&core).first().map(String::as_str) != Some("Beta 0000") {
            return Err(format!(
                "unexpected first channel: {:?}",
                channel_names(&core).first()
            ));
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while programme_titles(&core).is_empty() {
            if tokio::time::Instant::now() >= deadline {
                return Err(
                    "deferred saved EPG failed to finish restoring on a 1-worker blocking pool"
                        .to_owned(),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        if programme_titles(&core).first().map(String::as_str) != Some("Persisted Programme 0000") {
            return Err(format!(
                "unexpected first programme: {:?}",
                programme_titles(&core).first()
            ));
        }
        Ok(())
    });
    // Bound teardown so a deadlocked blocking worker cannot hang `cargo test`.
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
    if let Err(message) = outcome {
        panic!("{message}");
    }
}

struct OrderedConcurrentSource {
    m3u_name: Mutex<&'static str>,
    m3u_gate: Option<Arc<tokio::sync::Semaphore>>,
    epg_gate: Option<Arc<tokio::sync::Semaphore>>,
    m3u_entered: Arc<tokio::sync::Notify>,
    epg_entered: Arc<tokio::sync::Notify>,
    epg_returns_not_modified: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl SourceAccess for OrderedConcurrentSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        match request.kind() {
            SourceKind::M3u => {
                self.m3u_entered.notify_one();
                if let Some(gate) = &self.m3u_gate {
                    let _ = gate.acquire().await;
                }
                let name = *self.m3u_name.lock().unwrap();
                let payload = Bytes::from(String::from_utf8_lossy(M3U).replace("Alpha", name));
                let len = payload.len() as u64;
                Ok(SourceResponse::with_validators(
                    Some(len),
                    Box::pin(stream::iter([Ok(payload)])),
                    PrivateSourceValidators::parse(Some(format!("m3u-{name}")), None).unwrap(),
                ))
            }
            SourceKind::Epg => {
                self.epg_entered.notify_one();
                if let Some(gate) = &self.epg_gate {
                    let _ = gate.acquire().await;
                }
                if self.epg_returns_not_modified.load(Ordering::SeqCst) {
                    return Ok(SourceResponse::not_modified(
                        PrivateSourceValidators::parse(
                            Some("epg-304-revalidated".to_owned()),
                            None,
                        )
                        .unwrap(),
                    ));
                }
                let payload = Bytes::from_static(EPG);
                let len = payload.len() as u64;
                Ok(SourceResponse::with_validators(
                    Some(len),
                    Box::pin(stream::iter([Ok(payload)])),
                    PrivateSourceValidators::parse(Some("epg-initial".to_owned()), None).unwrap(),
                ))
            }
        }
    }
}

#[tokio::test]
async fn concurrent_m3u_update_and_epg_not_modified_preserve_guide_in_both_orders() {
    for epg_finishes_first in [true, false] {
        let directory = TempDir::new().unwrap();
        let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
        let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
        let clock = ControlledClock::at("2026-08-29T12:00:00Z");

        let m3u_gate = Arc::new(tokio::sync::Semaphore::new(1));
        let epg_gate = Arc::new(tokio::sync::Semaphore::new(1));
        let m3u_entered = Arc::new(tokio::sync::Notify::new());
        let epg_entered = Arc::new(tokio::sync::Notify::new());
        let epg_not_modified = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let source = Arc::new(OrderedConcurrentSource {
            m3u_name: Mutex::new("Alpha"),
            m3u_gate: Some(Arc::clone(&m3u_gate)),
            epg_gate: Some(Arc::clone(&epg_gate)),
            m3u_entered: Arc::clone(&m3u_entered),
            epg_entered: Arc::clone(&epg_entered),
            epg_returns_not_modified: Arc::clone(&epg_not_modified),
        });

        // Seed Alpha + EPG snapshots and persist the warm mmap cache first so
        // the concurrent refresh below exercises rematching cold EPG tables
        // directly from a mapped catalog in both publication orderings.
        drop(
            SparrowCore::bootstrap(
                Some(configuration(M3U_URL, EPG_URL)),
                CoreAdapters::new(
                    Arc::clone(&source) as _,
                    store.clone(),
                    Arc::new(clock.clone()),
                ),
            )
            .await
            .unwrap(),
        );
        let builder = SparrowCore::bootstrap_from_snapshots(
            Some(configuration(M3U_URL, EPG_URL)),
            CoreAdapters::new(
                Arc::clone(&source) as _,
                store.clone(),
                Arc::new(clock.clone()),
            ),
        )
        .await
        .unwrap();
        wait_for_cache_write(&store, 0).await;
        drop(builder);

        store.candidate_opens.store(0, Ordering::SeqCst);
        let core = SparrowCore::bootstrap_installed(
            Some(configuration(M3U_URL, EPG_URL)),
            CoreAdapters::new(
                Arc::clone(&source) as _,
                store.clone(),
                Arc::new(clock.clone()),
            ),
        )
        .await
        .unwrap();
        assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
        assert_eq!(channel_names(&core), ["Alpha"]);
        assert_eq!(programme_titles(&core), ["Persisted Programme"]);

        // Now change M3U to Beta and make EPG return 304 Not Modified, gating
        // the two concurrent refreshes to exercise both publication orderings.
        *source.m3u_name.lock().unwrap() = "Beta";
        epg_not_modified.store(true, Ordering::SeqCst);
        clock.set("2026-08-29T13:00:00Z");
        let _m3u_hold = m3u_gate.acquire().await.unwrap();
        let _epg_hold = epg_gate.acquire().await.unwrap();

        let refresh_task = tokio::spawn({
            let core = core.clone();
            async move { core.refresh(RefreshTrigger::Manual).await }
        });
        m3u_entered.notified().await;
        epg_entered.notified().await;

        let writes = store.cache_writes.load(Ordering::SeqCst);
        if epg_finishes_first {
            drop(_epg_hold);
            // Wait until EPG revalidation writes the new manifest on disk.
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while store.revalidation_count() == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("EPG revalidates before M3U publishes");
            drop(_m3u_hold);
        } else {
            drop(_m3u_hold);
            // Wait until M3U publishes Beta while EPG 304 is still in flight.
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while channel_names(&core) != ["Beta"] {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("M3U publishes before EPG 304 finishes");
            drop(_epg_hold);
        }

        let report = refresh_task.await.expect("refresh must not panic");
        assert!(matches!(report.m3u(), RefreshOutcome::Updated { .. }));
        assert!(matches!(
            report.epg(),
            Some(RefreshOutcome::NotModified { .. })
        ));
        assert_eq!(channel_names(&core), ["Beta"]);
        assert_eq!(
            programme_titles(&core),
            ["Persisted Programme"],
            "EPG must remain attached (epg_finishes_first={epg_finishes_first})"
        );
        assert!(matches!(
            core.status().epg(),
            Some(SourceState::Fresh { .. })
        ));

        wait_for_cache_write(&store, writes).await;
        drop(core);

        store.candidate_opens.store(0, Ordering::SeqCst);
        let reopened = snapshot_restart(Arc::clone(&store), true).await;
        assert_eq!(channel_names(&reopened), ["Beta"]);
        assert_eq!(programme_titles(&reopened), ["Persisted Programme"]);
        assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn configuration_replacement_queues_recovered_catalog_cache_even_when_refresh_fails() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1"));

    let core = SparrowCore::bootstrap(
        None,
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let status = core
        .replace_source_configuration(Some(configuration(M3U_URL, EPG_URL)))
        .await;
    assert!(status.generation().is_some());
    assert_eq!(channel_names(&core), ["Alpha"]);
    assert_eq!(programme_titles(&core), ["Persisted Programme"]);
    wait_for_cache_write(&store, writes).await;
    drop(core);

    store.candidate_opens.store(0, Ordering::SeqCst);
    let reopened = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&reopened), ["Alpha"]);
    assert_eq!(programme_titles(&reopened), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn blocked_cache_writer_cancels_superseded_generation_and_persists_latest_across_two_updates()
{
    struct MutableNameSource(Mutex<&'static str>);

    #[async_trait]
    impl SourceAccess for MutableNameSource {
        async fn open(
            &self,
            request: SourceRequest,
        ) -> Result<SourceResponse, SourceAccessFailure> {
            let payload = match request.kind() {
                SourceKind::M3u => {
                    let name = *self.0.lock().unwrap();
                    Bytes::from(String::from_utf8_lossy(M3U).replace("Alpha", name))
                }
                SourceKind::Epg => Bytes::from_static(EPG),
            };
            let len = payload.len() as u64;
            Ok(SourceResponse::new(
                Some(len),
                Box::pin(stream::iter([Ok(payload)])),
            ))
        }
    }

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
    let release = ReleaseCacheWrite(Arc::clone(&gate));
    let store = Arc::new(CapturingSnapshotStore {
        cache_gate: Some(Arc::clone(&gate)),
        ..CapturingSnapshotStore::new(disk)
    });
    let source = Arc::new(MutableNameSource(Mutex::new("Beta")));
    let core = SparrowCore::bootstrap_installed(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::clone(&source) as _,
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    // Wait until the background writer enters write_catalog_cache for Alpha.
    core.activate_automation();
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.started.notified())
        .await
        .unwrap();

    // Publish two content-changing generations (Beta, then Gamma) while write A
    // is blocked inside write_catalog_cache. Neither refresh may stall.
    let beta_report = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        core.refresh(RefreshTrigger::Manual),
    )
    .await
    .expect("first update completes while cache writer is blocked");
    assert!(matches!(beta_report.m3u(), RefreshOutcome::Updated { .. }));
    assert_eq!(channel_names(&core), ["Beta"]);

    *source.0.lock().unwrap() = "Gamma";
    let gamma_report = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        core.refresh(RefreshTrigger::Manual),
    )
    .await
    .expect("second update completes while cache writer is blocked");
    assert!(matches!(gamma_report.m3u(), RefreshOutcome::Updated { .. }));
    assert_eq!(channel_names(&core), ["Gamma"]);

    // Release the gate: the superseded write should cancel and the worker must
    // immediately persist the final generation (Gamma).
    drop(release);
    wait_for_cache_write(&store, 0).await;
    assert!(
        store.cancelled_cache_writes.load(Ordering::SeqCst) >= 1,
        "superseded blocked write must observe cancellation"
    );
    drop(core);

    store.candidate_opens.store(0, Ordering::SeqCst);
    let reopened = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&reopened), ["Gamma"]);
    assert_eq!(programme_titles(&reopened), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn delayed_cache_write_does_not_certify_corrupted_raw_snapshot() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let m3u_only = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        M3U_URL,
        None::<String>,
    ))
    .unwrap();
    drop(bootstrap(m3u_only, FixtureSource::online(), Arc::clone(&disk)).await);
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1"));
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1.stamp"));
    // Advance past the payload's filesystem tick before reopening so the
    // subsequent check cannot rely on tied timestamps to catch the corruption.
    let active_payload = directory.path().join("m3u/slot-a.payload");
    wait_for_fs_tick_after(directory.path(), &[&active_payload]);

    // Pause the background cache writer right after recovery and before
    // `AtomicFileSnapshotStore::write_mapped_catalog_cache` runs.
    let gate = Arc::new(CacheWriteGate::default());
    let release = ReleaseCacheWrite(Arc::clone(&gate));
    let store = Arc::new(CapturingSnapshotStore {
        cache_gate: Some(Arc::clone(&gate)),
        ..CapturingSnapshotStore::new(Arc::clone(&disk))
    });
    let m3u_only = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        M3U_URL,
        None::<String>,
    ))
    .unwrap();
    let core = SparrowCore::bootstrap_installed(
        Some(m3u_only),
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert_eq!(channel_names(&core), ["Alpha"]);
    core.activate_automation();
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.started.notified())
        .await
        .unwrap();

    // Corrupt 1 byte in-place inside the sole M3U payload without changing its
    // length, then release the delayed cache writer and wait for the live,
    // uncancelled write attempt to finish BEFORE dropping `core`.
    let active_payload = directory.path().join("m3u/slot-a.payload");
    let mut corrupted = std::fs::read(&active_payload).unwrap();
    corrupted[0] ^= 0xFF;
    std::fs::write(&active_payload, &corrupted).unwrap();
    let finished = gate.finished.notified();
    drop(release);
    tokio::time::timeout(std::time::Duration::from_secs(2), finished)
        .await
        .expect("delayed cache write must finish while core is alive");

    assert_eq!(
        store.cancelled_cache_writes.load(Ordering::SeqCst),
        0,
        "write must complete live rather than being cancelled by drop"
    );
    assert_eq!(
        store.cache_writes.load(Ordering::SeqCst),
        0,
        "corrupted payload must cause write_mapped_catalog_cache to return None"
    );
    assert!(
        !directory.path().join("catalog-cache-v1").exists(),
        "rejected write must leave catalog-cache-v1 absent"
    );
    drop(core);

    // Offline launch must reject the sole corrupted M3U snapshot instead of
    // serving cached Alpha.
    let m3u_only = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        M3U_URL,
        None::<String>,
    ))
    .unwrap();
    let offline = SparrowCore::bootstrap_installed(
        Some(m3u_only),
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            Arc::new(CapturingSnapshotStore::new(disk)),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert!(
        offline.status().generation().is_none(),
        "corrupted sole snapshot must not be masked by a delayed cache write"
    );
}

#[tokio::test]
async fn configured_unavailable_epg_still_persists_and_promotes_m3u_cache() {
    struct M3uOnlyOnlineSource;

    #[async_trait]
    impl SourceAccess for M3uOnlyOnlineSource {
        async fn open(
            &self,
            request: SourceRequest,
        ) -> Result<SourceResponse, SourceAccessFailure> {
            match request.kind() {
                SourceKind::M3u => Ok(SourceResponse::new(
                    Some(M3U.len() as u64),
                    Box::pin(stream::iter([Ok(Bytes::from_static(M3U))])),
                )),
                SourceKind::Epg => Err(SourceAccessError::Unavailable.into()),
            }
        }
    }

    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));

    // Seed a valid M3U snapshot with a configured EPG that has no saved snapshot.
    drop(
        SparrowCore::bootstrap(
            Some(configuration(M3U_URL, EPG_URL)),
            CoreAdapters::new(
                Arc::new(M3uOnlyOnlineSource),
                store.clone(),
                Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
            ),
        )
        .await
        .unwrap(),
    );
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1"));
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1.stamp"));

    // Progressive installed restart defers cache persistence until deferred EPG
    // recovery finishes. When no saved EPG exists, the owned M3U catalog must
    // still be persisted and promoted after deferred recovery completes.
    let writes = store.cache_writes.load(Ordering::SeqCst);
    let core = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&core), ["Alpha"]);
    core.activate_automation();
    wait_for_cache_write(&store, writes).await;
    drop(core);

    // Subsequent offline restart must hit the warm M3U-only cache with zero raw
    // candidate opens.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let warm = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&warm), ["Alpha"]);
    assert!(programme_titles(&warm).is_empty());
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn same_key_byte_identical_restore_keeps_cache_inode_and_refreshes_stamp_sidecar() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
    drop(
        bootstrap(
            configuration(M3U_URL, EPG_URL),
            FixtureSource::online(),
            Arc::clone(&store),
        )
        .await,
    );
    let seed = snapshot_restart(Arc::clone(&store), false).await;
    wait_for_cache_write(&store, 0).await;
    drop(seed);

    let cache_path = directory.path().join("catalog-cache-v1");
    let stamp_path = directory.path().join("catalog-cache-v1.stamp");
    let cache_inode = std::fs::metadata(&cache_path).unwrap().ino();
    let initial_stamp_bytes = std::fs::read(&stamp_path).unwrap();

    // Byte-identical external restore keeping the exact same cache key (slot-a)
    // and preserving private 0o600 permissions so scan does not touch ctime.
    let payload_path = directory.path().join("m3u/slot-a.payload");
    let payload_bytes = std::fs::read(&payload_path).unwrap();
    let restored_tmp = directory.path().join("m3u/restore-same-key.tmp");
    std::fs::write(&restored_tmp, &payload_bytes).unwrap();
    std::fs::set_permissions(&restored_tmp, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::rename(&restored_tmp, &payload_path).unwrap();
    let epg_payload = directory.path().join("epg/slot-a.payload");
    wait_for_fs_tick_after(directory.path(), &[&payload_path, &epg_payload]);

    // Offline restart 1: verifies raw payload, serves mapped cache with zero
    // raw candidate opens, preserves catalog-cache-v1 inode, and refreshes
    // catalog-cache-v1.stamp atomically.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let after_restore_1 = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&after_restore_1), ["Alpha"]);
    assert_eq!(programme_titles(&after_restore_1), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::metadata(&cache_path).unwrap().ino(), cache_inode);
    let refreshed_stamp_bytes = std::fs::read(&stamp_path).unwrap();
    let refreshed_stamp_inode = std::fs::metadata(&stamp_path).unwrap().ino();
    assert_ne!(refreshed_stamp_bytes, initial_stamp_bytes);
    drop(after_restore_1);

    // Offline restart 2: fast path hits without rewriting either file.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let after_restore_2 = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&after_restore_2), ["Alpha"]);
    assert_eq!(programme_titles(&after_restore_2), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::metadata(&cache_path).unwrap().ino(), cache_inode);
    assert_eq!(
        std::fs::metadata(&stamp_path).unwrap().ino(),
        refreshed_stamp_inode
    );
}

#[tokio::test]
async fn transient_cache_write_failure_retries_promotion_on_subsequent_not_modified_refresh() {
    let directory = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap());
    let online = ConditionalFixtureSource::new();
    let seed_store = Arc::new(CapturingSnapshotStore::new(Arc::clone(&disk)));
    drop(
        SparrowCore::bootstrap(
            Some(configuration(M3U_URL, EPG_URL)),
            CoreAdapters::new(
                Arc::new(online.clone()),
                seed_store.clone(),
                Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
            ),
        )
        .await
        .unwrap(),
    );
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1"));
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1.stamp"));

    // Fail the initial background cache write transiently (e.g., disk full),
    // leaving the published catalog Owned.
    let gate = Arc::new(CacheWriteGate::default());
    let release = ReleaseCacheWrite(Arc::clone(&gate));
    let store = Arc::new(CapturingSnapshotStore {
        fail_next_cache_writes: Arc::new(AtomicUsize::new(1)),
        cache_gate: Some(Arc::clone(&gate)),
        ..CapturingSnapshotStore::new(Arc::clone(&disk))
    });
    let core = SparrowCore::bootstrap_from_snapshots(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::new(online.clone()),
            store.clone(),
            Arc::new(FixedClock(parse_time("2026-08-29T12:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    let first_attempt_done = gate.finished.notified();
    drop(release);
    tokio::time::timeout(std::time::Duration::from_secs(2), first_attempt_done)
        .await
        .expect("initial failed cache write attempt finishes");
    assert_eq!(store.cache_writes.load(Ordering::SeqCst), 0);

    // Space becomes available; a 304 NotModified refresh must queue a retry for
    // the still-Owned catalog and persist/promote it.
    let report = core.refresh(RefreshTrigger::Manual).await;
    assert!(matches!(report.m3u(), RefreshOutcome::NotModified { .. }));
    assert!(matches!(
        report.epg(),
        Some(RefreshOutcome::NotModified { .. })
    ));
    wait_for_cache_write(&store, 0).await;
    drop(core);

    // Next offline restart hits the persisted warm cache cleanly.
    store.candidate_opens.store(0, Ordering::SeqCst);
    let warm = snapshot_restart(Arc::clone(&store), true).await;
    assert_eq!(channel_names(&warm), ["Alpha"]);
    assert_eq!(programme_titles(&warm), ["Persisted Programme"]);
    assert_eq!(store.candidate_opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn deferred_epg_adoption_does_not_roll_back_concurrent_epg_refresh_on_disk() {
    struct AdoptGatedStore {
        inner: Arc<AtomicFileSnapshotStore>,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        finished: Arc<tokio::sync::Notify>,
        gate_epg_adopt: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl SnapshotStore for AdoptGatedStore {
        fn catalog_cache_enabled(&self) -> bool {
            self.inner.catalog_cache_enabled()
        }

        fn open_mapped_catalog_cache(
            &self,
            key: &[u8; 32],
            candidates: &[&SnapshotCandidate],
            validate: &mut dyn FnMut(&[u8]) -> bool,
        ) -> Option<CatalogCacheMapping> {
            self.inner
                .open_mapped_catalog_cache(key, candidates, validate)
        }

        fn write_mapped_catalog_cache(
            &self,
            key: &[u8; 32],
            candidates: &[&SnapshotCandidate],
            bytes: &[u8],
            is_cancelled: &(dyn Fn() -> bool + Send + Sync),
        ) -> Option<CatalogCacheMapping> {
            self.inner
                .write_mapped_catalog_cache(key, candidates, bytes, is_cancelled)
        }

        async fn scan_candidates(
            &self,
            source: SnapshotSource,
        ) -> Result<SnapshotScan, StoreError> {
            self.inner.scan_candidates(source).await
        }

        async fn open_candidate(
            &self,
            candidate: &SnapshotCandidate,
        ) -> Result<Box<dyn std::io::BufRead + Send>, StoreError> {
            self.inner.open_candidate(candidate).await
        }

        async fn adopt_candidate(
            &self,
            candidate: &SnapshotCandidate,
        ) -> Result<SnapshotCandidate, StoreError> {
            let gated = candidate.metadata().source().kind() == SourceKind::Epg
                && self
                    .gate_epg_adopt
                    .swap(false, std::sync::atomic::Ordering::AcqRel);
            if gated {
                self.entered.notify_one();
                self.release.notified().await;
            }
            let result = self.inner.adopt_candidate(candidate).await;
            if gated {
                self.finished.notify_one();
            }
            result
        }

        async fn revalidate_candidate(
            &self,
            candidate: &SnapshotCandidate,
            revalidation: &SnapshotRevalidation,
        ) -> Result<SnapshotCandidate, StoreError> {
            self.inner
                .revalidate_candidate(candidate, revalidation)
                .await
        }

        fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
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
            self.inner.activate(validated)
        }

        fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
            self.inner.discard(stage)
        }
    }

    struct RefreshedEpgSource;

    #[async_trait]
    impl SourceAccess for RefreshedEpgSource {
        async fn open(
            &self,
            request: SourceRequest,
        ) -> Result<SourceResponse, SourceAccessFailure> {
            let payload = match request.kind() {
                SourceKind::M3u => Bytes::from_static(M3U),
                SourceKind::Epg => Bytes::from(
                    String::from_utf8_lossy(EPG)
                        .replace("Persisted Programme", "Refreshed Programme"),
                ),
            };
            let len = payload.len() as u64;
            Ok(SourceResponse::new(
                Some(len),
                Box::pin(stream::iter([Ok(payload)])),
            ))
        }
    }

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
    // Remove cache and active EPG pointer so deferred EPG recovery must adopt
    // the fallback slot-a candidate.
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1"));
    let _ = std::fs::remove_file(directory.path().join("catalog-cache-v1.stamp"));
    std::fs::remove_file(directory.path().join("epg/active.json")).unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(tokio::sync::Notify::new());
    let gated_store = Arc::new(AdoptGatedStore {
        inner: Arc::clone(&disk),
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
        finished: Arc::clone(&finished),
        gate_epg_adopt: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    });

    let core = SparrowCore::bootstrap_installed(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::new(RefreshedEpgSource),
            gated_store,
            Arc::new(FixedClock(parse_time("2026-08-29T13:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    core.activate_automation();

    // Wait until deferred EPG recovery pauses immediately before adopting slot-a.
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .expect("deferred EPG recovery reaches adopt_candidate");

    // Manual refresh activates slot-b with "Refreshed Programme" while adoption
    // of slot-a is paused.
    let report = core.refresh(RefreshTrigger::Manual).await;
    assert!(matches!(report.epg(), Some(RefreshOutcome::Updated { .. })));
    assert_eq!(programme_titles(&core), ["Refreshed Programme"]);

    // Resume deferred EPG adoption and wait for it to finish.
    let adopt_done = finished.notified();
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), adopt_done)
        .await
        .expect("paused adopt_candidate completes");
    assert_eq!(programme_titles(&core), ["Refreshed Programme"]);
    drop(core);

    // Next offline restart MUST restore "Refreshed Programme" from slot-b,
    // never rolling back to "Persisted Programme" in slot-a.
    let reopened = SparrowCore::bootstrap_from_snapshots(
        Some(configuration(M3U_URL, EPG_URL)),
        CoreAdapters::new(
            Arc::new(FixtureSource::rejecting()),
            Arc::new(AtomicFileSnapshotStore::open(directory.path()).unwrap()),
            Arc::new(FixedClock(parse_time("2026-08-29T13:00:00Z"))),
        ),
    )
    .await
    .unwrap();
    assert_eq!(programme_titles(&reopened), ["Refreshed Programme"]);
}

fn wait_for_fs_tick_after(root: &std::path::Path, paths: &[&std::path::Path]) {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    let freshness = |meta: &std::fs::Metadata| -> (i64, i64) {
        #[cfg(unix)]
        {
            (meta.mtime(), meta.mtime_nsec()).max((meta.ctime(), meta.ctime_nsec()))
        }
        #[cfg(not(unix))]
        {
            let d = meta
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap();
            (d.as_secs() as i64, i64::from(d.subsec_nanos()))
        }
    };
    let target = paths
        .iter()
        .map(|p| freshness(&std::fs::symlink_metadata(p).expect("target file exists")))
        .max()
        .expect("at least one path");
    let probe = root.join(".test-fs-tick-probe.tmp");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        std::fs::write(&probe, b"").expect("probe writes");
        let meta = std::fs::symlink_metadata(&probe).expect("probe stats");
        let _ = std::fs::remove_file(&probe);
        #[cfg(unix)]
        let probe_mtime = (meta.mtime(), meta.mtime_nsec());
        #[cfg(not(unix))]
        let probe_mtime = freshness(&meta);
        if probe_mtime > target {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "filesystem timestamp did not advance past {target:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
