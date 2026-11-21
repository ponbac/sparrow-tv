use std::{
    alloc::{GlobalAlloc, Layout, System},
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream;
use sparrow_core::{
    CatalogCacheMapping, ChannelQuery, Clock, CoreAdapters, GuideWindowQuery, PageLimit,
    PageRequest, SearchRequest, SearchTerm, SnapshotCandidate, SnapshotRevalidation, SnapshotScan,
    SnapshotSource, SnapshotStage, SnapshotStageRequest, SnapshotStore, SourceAccess,
    SourceAccessError, SourceAccessFailure, SourceByteStream, SourceConfiguration,
    SourceConfigurationInput, SourceKind, SourceRequest, SourceResponse, SparrowCore, StoreError,
    ValidatedStage,
};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use tempfile::TempDir;

const CHANNEL_COUNT: usize = 2_048;
const PROGRAMMES_PER_CHANNEL: usize = 16;

static LIVE_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

// SAFETY: Every operation delegates to `System` with the caller-provided layout and
// updates only an independent atomic counter.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is passed through unchanged to `System`.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE_ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::SeqCst);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_ALLOCATED_BYTES.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: `pointer` and `layout` are the exact pair supplied by the caller.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The allocation pair and requested size are passed through unchanged.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                LIVE_ALLOCATED_BYTES.fetch_add(new_size - layout.size(), Ordering::SeqCst);
            } else {
                LIVE_ALLOCATED_BYTES.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct StaticSource {
    m3u: Bytes,
    epg: Bytes,
    online: bool,
}

#[async_trait]
impl SourceAccess for StaticSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        if !self.online {
            return Err(SourceAccessError::Unavailable.into());
        }
        let payload = match request.kind() {
            SourceKind::M3u => self.m3u.clone(),
            SourceKind::Epg => self.epg.clone(),
        };
        let len = payload.len() as u64;
        let body: SourceByteStream = Box::pin(stream::iter([Ok(payload)]));
        Ok(SourceResponse::new(Some(len), body))
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

struct ObservingStore {
    inner: Arc<AtomicFileSnapshotStore>,
    cache_writes: Arc<AtomicUsize>,
    candidate_opens: Arc<AtomicUsize>,
}

#[async_trait]
impl SnapshotStore for ObservingStore {
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
        let mapping = self
            .inner
            .write_mapped_catalog_cache(key, candidates, bytes, is_cancelled);
        if mapping.is_some() {
            self.cache_writes.fetch_add(1, Ordering::SeqCst);
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

#[tokio::test]
async fn warm_mmap_open_and_post_write_promotion_retain_near_zero_rust_heap() {
    let dir = TempDir::new().unwrap();
    let disk = Arc::new(AtomicFileSnapshotStore::open(dir.path()).unwrap());
    let cache_writes = Arc::new(AtomicUsize::new(0));
    let candidate_opens = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(ObservingStore {
        inner: Arc::clone(&disk),
        cache_writes: Arc::clone(&cache_writes),
        candidate_opens: Arc::clone(&candidate_opens),
    });
    let now = DateTime::parse_from_rfc3339("2026-08-31T00:15:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Seed raw snapshots on disk.
    let seeded = SparrowCore::bootstrap(
        Some(configuration()),
        CoreAdapters::new(
            Arc::new(StaticSource {
                m3u: Bytes::from(generated_m3u()),
                epg: Bytes::from(generated_epg()),
                online: true,
            }),
            store.clone(),
            Arc::new(FixedClock(now)),
        ),
    )
    .await
    .unwrap();
    assert!(seeded.status().generation().is_some());
    drop(seeded);
    tokio::task::yield_now().await;

    // Cold recovery builds an owned FlatCatalog in heap, then writes and
    // promotes it to an immutable mmap backing in the background.
    let baseline = LIVE_ALLOCATED_BYTES.load(Ordering::SeqCst);
    let cold = SparrowCore::bootstrap_from_snapshots(
        Some(configuration()),
        CoreAdapters::new(
            Arc::new(StaticSource {
                m3u: Bytes::new(),
                epg: Bytes::new(),
                online: false,
            }),
            store.clone(),
            Arc::new(FixedClock(now)),
        ),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while cache_writes.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("background cache write completes");
    // Give the background thread a moment to drop the old Owned Vec<u8> after promotion.
    let cache_bytes = std::fs::metadata(dir.path().join("catalog-cache-v1"))
        .unwrap()
        .len() as usize;
    assert!(
        cache_bytes > 5_000_000,
        "fixture flat catalog should exceed 5 MB (got {cache_bytes} bytes)"
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while LIVE_ALLOCATED_BYTES
            .load(Ordering::SeqCst)
            .saturating_sub(baseline)
            > cache_bytes / 10
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Owned -> Mapped promotion must release the owned catalog buffer");
    let promoted_retained = LIVE_ALLOCATED_BYTES
        .load(Ordering::SeqCst)
        .saturating_sub(baseline);
    drop(cold);
    tokio::task::yield_now().await;

    // Warm installed startup opens the mmap directly with zero raw snapshot reads
    // and retains near-zero Rust heap bytes relative to the multi-MB catalog file.
    candidate_opens.store(0, Ordering::SeqCst);
    let warm_baseline = LIVE_ALLOCATED_BYTES.load(Ordering::SeqCst);
    let warm = SparrowCore::bootstrap_installed(
        Some(configuration()),
        CoreAdapters::new(
            Arc::new(StaticSource {
                m3u: Bytes::new(),
                epg: Bytes::new(),
                online: false,
            }),
            store.clone(),
            Arc::new(FixedClock(now)),
        ),
    )
    .await
    .unwrap();
    let warm_retained = LIVE_ALLOCATED_BYTES
        .load(Ordering::SeqCst)
        .saturating_sub(warm_baseline);
    assert_eq!(candidate_opens.load(Ordering::SeqCst), 0);

    // Verify full query parity from the mapped catalog.
    let channels = warm
        .list_channels(ChannelQuery::all(PageRequest::first(
            PageLimit::new(40).unwrap(),
        )))
        .unwrap();
    assert_eq!(channels.items().len(), 40);

    let guide = warm
        .guide_window(
            GuideWindowQuery::new(
                now,
                now + chrono::Duration::hours(2),
                ChannelQuery::all(PageRequest::first(PageLimit::new(40).unwrap())),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(guide.items().len(), 40);
    assert!(!guide.items()[0].programmes().is_empty());

    let search = warm
        .search(SearchRequest::new(
            SearchTerm::parse("channel 0512").unwrap(),
            PageRequest::first(PageLimit::new(10).unwrap()),
            PageRequest::first(PageLimit::new(10).unwrap()),
        ))
        .unwrap();
    assert_eq!(search.channels().items().len(), 1);
    assert_eq!(
        search.channels().items()[0].name(),
        "Footprint Channel 0512"
    );

    drop(channels);
    drop(guide);
    drop(search);
    drop(warm);

    eprintln!(
        "mmap footprint ({CHANNEL_COUNT} channels, {} programmes): cache_file={cache_bytes} B, post_promotion_heap={promoted_retained} B, warm_open_heap={warm_retained} B",
        CHANNEL_COUNT * PROGRAMMES_PER_CHANNEL
    );
    assert!(
        warm_retained < 128 * 1024,
        "warm mmap open should retain < 128 KiB of Rust heap (got {warm_retained} B for {cache_bytes} B catalog)"
    );
}

fn configuration() -> SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://footprint.fixture.invalid/channels.m3u",
        Some("https://footprint.fixture.invalid/guide.xml"),
    ))
    .unwrap()
}

fn generated_m3u() -> Vec<u8> {
    let mut m3u = String::from("#EXTM3U\n");
    for channel in 0..CHANNEL_COUNT {
        writeln!(
            m3u,
            "#EXTINF:-1 tvg-id=\"fp-{channel}\" group-title=\"Group {:02}\",Footprint Channel {channel:04}",
            channel % 32,
        )
        .unwrap();
        writeln!(
            m3u,
            "https://media.fixture.invalid/fp/{channel}?token={channel:08}"
        )
        .unwrap();
    }
    m3u.into_bytes()
}

fn generated_epg() -> Vec<u8> {
    let mut epg = String::from("<?xml version=\"1.0\"?><tv>");
    for channel in 0..CHANNEL_COUNT {
        write!(
            epg,
            "<channel id=\"fp-{channel}\"><display-name>Footprint Channel {channel:04}</display-name></channel>"
        )
        .unwrap();
    }
    for channel in 0..CHANNEL_COUNT {
        for programme in 0..PROGRAMMES_PER_CHANNEL {
            write!(
                epg,
                "<programme start=\"20260831{:02}0000 +0000\" stop=\"20260831{:02}3000 +0000\" channel=\"fp-{channel}\"><title>Footprint Programme {channel:04}-{programme:02}</title><desc>Deterministic description for channel {channel:04} programme {programme:02} with extra prose to exercise zero-copy mmap footprint.</desc></programme>",
                programme % 24,
                programme % 24,
            )
            .unwrap();
        }
    }
    epg.push_str("</tv>");
    epg.into_bytes()
}
