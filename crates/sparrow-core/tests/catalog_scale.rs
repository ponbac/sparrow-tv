mod support;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bytes::Bytes;
use sparrow_core::{
    ChannelQuery, CoreAdapters, PageLimit, PageRequest, SearchRequest, SearchTerm,
    SourceConfigurationInput, SparrowCore,
};
use support::{FixedClock, MemorySnapshotStore, ScriptedSource};

const CHANNEL_COUNT: usize = 512;
const PROGRAMMES_PER_CHANNEL: usize = 24;
const PRE_FIX_RETAINED_CATALOG_BYTES: usize = 7_200_467;
const PRE_FIX_RETAINED_PAGE_BYTES: usize = 102_520;
const PRE_FIX_FULL_CATALOG_SEARCH_ALLOCATIONS: usize = 51_202;
const MAX_FULL_CATALOG_SEARCH_ALLOCATIONS: usize = 6;
const PRE_FIX_BROAD_SEARCH_ALLOCATIONS: usize = 42;
const MAX_BROAD_SEARCH_ALLOCATIONS: usize = 28;
const PRE_FIX_BROAD_SEARCH_PEAK_BYTES: usize = 230_037;
const MAX_BROAD_SEARCH_PEAK_BYTES: usize = 4_096;

static LIVE_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_OPERATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

// SAFETY: Every operation delegates to `System` with the caller-provided layout and
// changes only an independent atomic counter after a successful allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is passed through unchanged to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATION_OPERATIONS.fetch_add(1, Ordering::SeqCst);
            let live = LIVE_ALLOCATED_BYTES
                .fetch_add(layout.size(), Ordering::SeqCst)
                .saturating_add(layout.size());
            PEAK_LIVE_ALLOCATED_BYTES.fetch_max(live, Ordering::SeqCst);
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
            ALLOCATION_OPERATIONS.fetch_add(1, Ordering::SeqCst);
            if new_size >= layout.size() {
                let growth = new_size - layout.size();
                let live = LIVE_ALLOCATED_BYTES
                    .fetch_add(growth, Ordering::SeqCst)
                    .saturating_add(growth);
                PEAK_LIVE_ALLOCATED_BYTES.fetch_max(live, Ordering::SeqCst);
            } else {
                LIVE_ALLOCATED_BYTES.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[tokio::test]
async fn generated_catalog_has_bounded_retained_ownership_and_page_projection() {
    let snapshots = MemorySnapshotStore::default();
    let source = ScriptedSource::from_bytes(Bytes::from(generated_m3u()))
        .with_epg_bytes(Bytes::from(generated_epg()));
    let seeded = SparrowCore::bootstrap(
        Some(configuration()),
        CoreAdapters::new(
            Arc::new(source.clone()),
            Arc::new(snapshots.clone()),
            Arc::new(FixedClock::default()),
        ),
    )
    .await
    .expect("generated Source Snapshots seed a catalog");
    assert!(seeded.status().generation().is_some());
    drop(seeded);
    drop(source);
    tokio::task::yield_now().await;

    let baseline = live_allocated_bytes();
    let core = SparrowCore::bootstrap_from_snapshots(
        Some(configuration()),
        CoreAdapters::new(
            Arc::new(ScriptedSource::unavailable()),
            Arc::new(snapshots.clone()),
            Arc::new(FixedClock::default()),
        ),
    )
    .await
    .expect("generated Source Snapshots recover offline");
    let retained_catalog_bytes = live_allocated_bytes().saturating_sub(baseline);

    let page = core
        .list_channels(ChannelQuery::all(PageRequest::first(
            PageLimit::new(24).expect("the fixture page limit is valid"),
        )))
        .expect("the generated catalog is queryable");
    assert_eq!(page.items().len(), 24);

    let search_request = SearchRequest::new(
        SearchTerm::parse("alpha omega").expect("the fixture search term is valid"),
        PageRequest::first(PageLimit::new(8).expect("the fixture page limit is valid")),
        PageRequest::first(PageLimit::new(8).expect("the fixture page limit is valid")),
    );
    let allocation_baseline = ALLOCATION_OPERATIONS.load(Ordering::SeqCst);
    let search = core
        .search(search_request)
        .expect("the generated catalog is searchable");
    let search_allocations = ALLOCATION_OPERATIONS
        .load(Ordering::SeqCst)
        .saturating_sub(allocation_baseline);
    assert!(search.channels().items().is_empty());
    assert!(search.programmes().items().is_empty());
    eprintln!(
        "full-catalog no-match search allocation operations {PRE_FIX_FULL_CATALOG_SEARCH_ALLOCATIONS} -> {search_allocations}"
    );
    assert!(
        search_allocations <= MAX_FULL_CATALOG_SEARCH_ALLOCATIONS,
        "full-catalog no-match search used {search_allocations} allocation operations"
    );
    assert!(
        PRE_FIX_FULL_CATALOG_SEARCH_ALLOCATIONS.saturating_sub(search_allocations) >= 50_000,
        "full-catalog no-match search saved fewer than 50,000 allocation operations"
    );
    drop(search);

    let broad_search_request = SearchRequest::new(
        SearchTerm::parse("scale").expect("the fixture search term is valid"),
        PageRequest::first(PageLimit::new(8).expect("the fixture page limit is valid")),
        PageRequest::first(PageLimit::new(8).expect("the fixture page limit is valid")),
    );
    let search_live_baseline = reset_peak_allocated_bytes();
    let broad_allocation_baseline = ALLOCATION_OPERATIONS.load(Ordering::SeqCst);
    let broad_search = core
        .search(broad_search_request)
        .expect("the generated catalog supports a broad search");
    let broad_search_allocations = ALLOCATION_OPERATIONS
        .load(Ordering::SeqCst)
        .saturating_sub(broad_allocation_baseline);
    let broad_search_peak_bytes = PEAK_LIVE_ALLOCATED_BYTES
        .load(Ordering::SeqCst)
        .saturating_sub(search_live_baseline);
    assert_eq!(broad_search.channels().items().len(), 8);
    assert_eq!(broad_search.programmes().items().len(), 8);
    assert!(broad_search.channels().next().is_some());
    assert!(broad_search.programmes().next().is_some());
    eprintln!(
        "full-catalog broad search allocation operations {PRE_FIX_BROAD_SEARCH_ALLOCATIONS} -> {broad_search_allocations}; peak transient bytes {PRE_FIX_BROAD_SEARCH_PEAK_BYTES} -> {broad_search_peak_bytes}"
    );
    assert!(
        broad_search_allocations <= MAX_BROAD_SEARCH_ALLOCATIONS,
        "full-catalog broad search used {broad_search_allocations} allocation operations"
    );
    assert!(
        broad_search_peak_bytes <= MAX_BROAD_SEARCH_PEAK_BYTES,
        "full-catalog broad search used {broad_search_peak_bytes} peak transient bytes"
    );
    assert!(
        PRE_FIX_BROAD_SEARCH_PEAK_BYTES.saturating_sub(broad_search_peak_bytes) >= 225_000,
        "full-catalog broad search saved fewer than 225,000 peak transient bytes"
    );

    assert_broad_search_continuation_matches_larger_prefix(&core, &broad_search);
    drop(broad_search);
    drop(core);
    tokio::task::yield_now().await;
    let retained_page_bytes = live_allocated_bytes().saturating_sub(baseline);
    drop(snapshots);

    eprintln!(
        "generated catalog retained {PRE_FIX_RETAINED_CATALOG_BYTES} -> {retained_catalog_bytes} bytes; one 24-Channel page retained {PRE_FIX_RETAINED_PAGE_BYTES} -> {retained_page_bytes} bytes"
    );

    // These initial ceilings intentionally describe the desired bounded ownership.
    // The pre-fix implementation exceeds them because CoreView retains parser models
    // alongside separately materialized catalog records, and a page pins all summaries.
    assert!(
        retained_catalog_bytes <= 6_500_000,
        "generated catalog retained {retained_catalog_bytes} bytes"
    );
    assert!(
        PRE_FIX_RETAINED_CATALOG_BYTES.saturating_sub(retained_catalog_bytes) >= 700_000,
        "generated catalog saved fewer than 700,000 retained bytes"
    );
    assert!(
        retained_page_bytes <= 16_000,
        "one bounded page retained {retained_page_bytes} bytes"
    );
    assert!(
        retained_page_bytes <= PRE_FIX_RETAINED_PAGE_BYTES / 10,
        "one bounded page still pins more than a tenth of the prior backing collection"
    );
}

fn live_allocated_bytes() -> usize {
    LIVE_ALLOCATED_BYTES.load(Ordering::SeqCst)
}

fn reset_peak_allocated_bytes() -> usize {
    let live = live_allocated_bytes();
    PEAK_LIVE_ALLOCATED_BYTES.store(live, Ordering::SeqCst);
    live
}

fn assert_broad_search_continuation_matches_larger_prefix(
    core: &SparrowCore,
    first: &sparrow_core::SearchResults,
) {
    let second = core
        .search(SearchRequest::new(
            SearchTerm::parse("scale").expect("the fixture search term is valid"),
            PageRequest::after(
                first
                    .channels()
                    .next()
                    .expect("broad Channel matches continue")
                    .clone(),
                PageLimit::new(5).expect("the fixture page limit is valid"),
            ),
            PageRequest::after(
                first
                    .programmes()
                    .next()
                    .expect("broad Programme matches continue")
                    .clone(),
                PageLimit::new(5).expect("the fixture page limit is valid"),
            ),
        ))
        .expect("broad search cursors continue");
    let prefix = core
        .search(SearchRequest::new(
            SearchTerm::parse("scale").expect("the fixture search term is valid"),
            PageRequest::first(PageLimit::new(13).expect("the fixture page limit is valid")),
            PageRequest::first(PageLimit::new(13).expect("the fixture page limit is valid")),
        ))
        .expect("the larger broad-search prefix is queryable");

    let paged_channels = first
        .channels()
        .items()
        .iter()
        .chain(second.channels().items())
        .map(|channel| channel.name())
        .collect::<Vec<_>>();
    let prefix_channels = prefix
        .channels()
        .items()
        .iter()
        .map(|channel| channel.name())
        .collect::<Vec<_>>();
    assert_eq!(paged_channels, prefix_channels);

    let paged_programmes = first
        .programmes()
        .items()
        .iter()
        .chain(second.programmes().items())
        .map(|programme| programme.title())
        .collect::<Vec<_>>();
    let prefix_programmes = prefix
        .programmes()
        .items()
        .iter()
        .map(|programme| programme.title())
        .collect::<Vec<_>>();
    assert_eq!(paged_programmes, prefix_programmes);
}

fn configuration() -> sparrow_core::SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://catalog-scale.fixture.invalid/channels.m3u",
        Some("https://catalog-scale.fixture.invalid/guide.xml"),
    ))
    .expect("the generated Source Configuration is valid")
}

fn generated_m3u() -> Vec<u8> {
    let mut m3u = String::from("#EXTM3U\n");
    for channel in 0..CHANNEL_COUNT {
        writeln!(
            m3u,
            "#EXTINF:-1 tvg-id=\"channel-{channel}\" group-title=\"Group {:02}\",Scale Channel {channel:04}",
            channel % 32,
        )
        .expect("writing to a String succeeds");
        writeln!(
            m3u,
            "https://media.fixture.invalid/channel/{channel}?fixture-token={channel:08}"
        )
        .expect("writing to a String succeeds");
    }
    m3u.into_bytes()
}

fn generated_epg() -> Vec<u8> {
    let mut epg = String::from("<?xml version=\"1.0\"?><tv>");
    for channel in 0..CHANNEL_COUNT {
        write!(
            epg,
            "<channel id=\"channel-{channel}\"><display-name>Scale Channel {channel:04}</display-name></channel>"
        )
        .expect("writing to a String succeeds");
    }
    for channel in 0..CHANNEL_COUNT {
        for programme in 0..PROGRAMMES_PER_CHANNEL {
            write!(
                epg,
                "<programme start=\"20260831000000 +0000\" stop=\"20260831003000 +0000\" channel=\"channel-{channel}\"><title>Scale Programme {channel:04}-{programme:02}</title><desc>Generated deterministic description {channel:04}-{programme:02} alpha beta gamma delta epsilon zeta eta theta iota kappa lambda.</desc></programme>"
            )
            .expect("writing to a String succeeds");
        }
    }
    epg.push_str("</tv>");
    epg.into_bytes()
}
