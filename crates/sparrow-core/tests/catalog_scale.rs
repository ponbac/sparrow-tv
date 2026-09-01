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
    ChannelQuery, CoreAdapters, PageLimit, PageRequest, SourceConfigurationInput, SparrowCore,
};
use support::{FixedClock, MemorySnapshotStore, ScriptedSource};

const CHANNEL_COUNT: usize = 512;
const PROGRAMMES_PER_CHANNEL: usize = 24;
const PRE_FIX_RETAINED_CATALOG_BYTES: usize = 7_200_467;
const PRE_FIX_RETAINED_PAGE_BYTES: usize = 102_520;

static LIVE_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

// SAFETY: Every operation delegates to `System` with the caller-provided layout and
// changes only an independent atomic counter after a successful allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is passed through unchanged to the system allocator.
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
