use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream;
use sparrow_core::{
    ChannelQuery, Clock, CoreAdapters, PageLimit, PageRequest, PrivateSourceValidators,
    ScheduleQuery, SourceAccess, SourceAccessError, SourceByteStream, SourceConfiguration,
    SourceConfigurationInput, SourceKind, SourceRequest, SourceResponse, SourceState, SparrowCore,
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

#[derive(Clone)]
struct FixtureSource {
    available: bool,
    m3u_opens: Arc<AtomicUsize>,
    epg_opens: Arc<AtomicUsize>,
}

impl FixtureSource {
    fn online() -> Self {
        Self {
            available: true,
            m3u_opens: Arc::new(AtomicUsize::new(0)),
            epg_opens: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn rejecting() -> Self {
        Self {
            available: false,
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
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessError> {
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
            return Err(SourceAccessError::Unavailable);
        }
        let body: SourceByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(payload))]));
        Ok(SourceResponse::with_validators(
            Some(payload.len() as u64),
            body,
            validators,
        ))
    }
}

struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

async fn bootstrap(
    configuration: SourceConfiguration,
    source: FixtureSource,
    store: Arc<AtomicFileSnapshotStore>,
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

fn limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("fixture page limit is valid")
}
