mod support;

use std::cell::Cell;

use sparrow_core::{
    CoreError, InputField, InputReason, PageCursor, PageLimit, PageRequest, SearchRequest,
    SearchTerm, SourceConfigurationInput, SourceKind, SparrowCore,
};
use support::{MemorySnapshotStore, ScriptedSource, adapters};

const CHANNELS: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="channel.exact" group-title="Search",News
https://media.fixture.invalid/channel-exact?token=private-exact
#EXTINF:-1 tvg-id="channel.prefix" group-title="Search",Newsroom
https://media.fixture.invalid/channel-prefix?token=private-prefix
#EXTINF:-1 tvg-id="channel.token" group-title="Search",Daily News Europe
https://media.fixture.invalid/channel-token?token=private-token
#EXTINF:-1 tvg-id="channel.substring" group-title="Search",Goodnews Archive
https://media.fixture.invalid/channel-substring?token=private-substring
#EXTINF:-1 tvg-id="programme.exact" group-title="Programmes",Alpha Channel
https://media.fixture.invalid/programme-exact
#EXTINF:-1 tvg-id="programme.prefix" group-title="Programmes",Beta Channel
https://media.fixture.invalid/programme-prefix
#EXTINF:-1 tvg-id="programme.token" group-title="Programmes",Gamma Channel
https://media.fixture.invalid/programme-token
#EXTINF:-1 tvg-id="programme.substring" group-title="Programmes",Delta Channel
https://media.fixture.invalid/programme-substring
"#;

const GUIDE: &[u8] = br#"<?xml version="1.0"?>
<tv>
  <channel id="programme.exact"><display-name>Alpha Channel</display-name></channel>
  <channel id="programme.prefix"><display-name>Beta Channel</display-name></channel>
  <channel id="programme.token"><display-name>Gamma Channel</display-name></channel>
  <channel id="programme.substring"><display-name>Delta Channel</display-name></channel>
  <programme start="19990101000000 +0000" stop="19990101010000 +0000" channel="programme.exact">
    <title>News</title><desc>Historical exact result</desc>
  </programme>
  <programme start="20990101000000 +0000" stop="20990101010000 +0000" channel="programme.prefix">
    <title>News Tonight</title>
  </programme>
  <programme start="20350101000000 +0000" stop="20350101010000 +0000" channel="programme.token">
    <title>Evening News Bulletin</title>
  </programme>
  <programme start="20010101000000 +0000" stop="20010101010000 +0000" channel="programme.substring">
    <title>Goodnews Story</title>
  </programme>
</tv>
"#;

#[tokio::test]
async fn channels_and_programmes_are_ranked_and_paginated_independently() {
    let (core, source) = core_with_guide(CHANNELS, GUIDE).await;
    let term = SearchTerm::parse("  ＮEWS\u{a0}").expect("fixture term is valid");
    let first = core
        .search(SearchRequest::new(
            term.clone(),
            PageRequest::first(limit(2)),
            PageRequest::first(limit(1)),
        ))
        .expect("the first search pages are available");

    assert_eq!(names(first.channels()), ["News", "Newsroom"]);
    assert_eq!(titles(first.programmes()), ["News"]);
    assert_eq!(first.generation(), first.channels().generation());
    assert_eq!(
        first.channels().generation(),
        first.programmes().generation()
    );
    let diagnostics = format!("{first:?}");
    for private in [
        "source-user",
        "source-secret",
        "guide-user",
        "guide-secret",
        "media.fixture.invalid",
        "private-exact",
    ] {
        assert!(
            !diagnostics.contains(private),
            "private marker leaked: {private}"
        );
        assert!(
            !first
                .channels()
                .next()
                .expect("Channel matches remain")
                .as_str()
                .contains(private),
            "private marker leaked through a search cursor: {private}"
        );
    }
    let channel_cursor = round_trip(first.channels().next().expect("Channel matches remain"));
    let programme_cursor = round_trip(first.programmes().next().expect("Programme matches remain"));

    let second = core
        .search(SearchRequest::new(
            term.clone(),
            PageRequest::after(channel_cursor, limit(2)),
            PageRequest::after(programme_cursor, limit(2)),
        ))
        .expect("both independent continuation pages are available");
    assert_eq!(
        names(second.channels()),
        ["Daily News Europe", "Goodnews Archive"]
    );
    assert!(second.channels().next().is_none());
    assert_eq!(
        titles(second.programmes()),
        ["News Tonight", "Evening News Bulletin"]
    );
    let final_programme_cursor = round_trip(
        second
            .programmes()
            .next()
            .expect("one Programme match remains"),
    );

    let final_programme = core
        .search(SearchRequest::new(
            term,
            PageRequest::first(limit(1)),
            PageRequest::after(final_programme_cursor, limit(2)),
        ))
        .expect("Programme pagination does not depend on Channel pagination");
    assert_eq!(names(final_programme.channels()), ["News"]);
    assert_eq!(titles(final_programme.programmes()), ["Goodnews Story"]);
    assert!(final_programme.programmes().next().is_none());

    let repeated = core
        .search(SearchRequest::new(
            SearchTerm::parse("news").expect("fixture term is valid"),
            PageRequest::first(limit(100)),
            PageRequest::first(limit(100)),
        ))
        .expect("the immutable search documents remain queryable");
    assert_eq!(repeated.channels().items().len(), 4);
    assert_eq!(repeated.programmes().items().len(), 4);
    let description_match = core
        .search(request("historical", 100, 100))
        .expect("Programme descriptions are indexed without reparsing EPG");
    assert!(description_match.channels().items().is_empty());
    assert_eq!(titles(description_match.programmes()), ["News"]);
    assert_eq!(source.open_count_for(SourceKind::M3u), 1);
    assert_eq!(source.open_count_for(SourceKind::Epg), 1);
}

#[tokio::test]
async fn lane_searches_page_only_the_requested_result_kind() {
    let (core, source) = core_with_guide(CHANNELS, GUIDE).await;
    let term = SearchTerm::parse("news").expect("fixture term is valid");

    let channels = core
        .search_channels(term.clone(), PageRequest::first(limit(2)))
        .expect("the Channel lane is searchable");
    let programmes = core
        .search_programmes(term.clone(), PageRequest::first(limit(1)))
        .expect("the Programme lane is searchable");
    let combined = core
        .search(SearchRequest::new(
            term.clone(),
            PageRequest::first(limit(2)),
            PageRequest::first(limit(1)),
        ))
        .expect("the combined search remains available");

    assert_eq!(names(&channels), ["News", "Newsroom"]);
    assert_eq!(titles(&programmes), ["News"]);
    assert_eq!(channels.generation(), programmes.generation());
    assert_eq!(channels.next(), combined.channels().next());
    assert_eq!(programmes.next(), combined.programmes().next());

    let channel_cursor = round_trip(channels.next().expect("Channel matches remain"));
    let programme_cursor = round_trip(programmes.next().expect("Programme matches remain"));
    let remaining_channels = core
        .search_channels(term.clone(), PageRequest::after(channel_cursor, limit(2)))
        .expect("the Channel lane cursor continues independently");
    let remaining_programmes = core
        .search_programmes(
            term.clone(),
            PageRequest::after(programme_cursor.clone(), limit(3)),
        )
        .expect("the Programme lane cursor continues independently");

    assert_eq!(
        names(&remaining_channels),
        ["Daily News Europe", "Goodnews Archive"]
    );
    assert_eq!(
        titles(&remaining_programmes),
        ["News Tonight", "Evening News Bulletin", "Goodnews Story"]
    );
    assert!(matches!(
        core.search_channels(term, PageRequest::after(programme_cursor, limit(1))),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));
    assert_eq!(source.open_count_for(SourceKind::M3u), 1);
    assert_eq!(source.open_count_for(SourceKind::Epg), 1);
}

#[tokio::test]
async fn search_cooperatively_stops_after_adapter_cancellation() {
    let (core, source) = core_with_guide(CHANNELS, GUIDE).await;
    let probes = Cell::new(0_u8);

    let result = core.search_with_cancellation(request("news", 100, 100), || {
        let next = probes.get().saturating_add(1);
        probes.set(next);
        next >= 3
    });

    assert!(matches!(result, Err(CoreError::Cancelled)));
    assert_eq!(probes.get(), 3);
    assert_eq!(source.open_count_for(SourceKind::M3u), 1);
    assert_eq!(source.open_count_for(SourceKind::Epg), 1);
}

#[tokio::test]
async fn search_cursors_are_bound_to_result_kind_term_and_catalog_generation() {
    let (first, _) = core_with_guide(CHANNELS, GUIDE).await;
    let first_results = first
        .search(request("news", 1, 1))
        .expect("the first catalog is searchable");
    let channel_cursor = round_trip(
        first_results
            .channels()
            .next()
            .expect("Channel matches remain"),
    );
    let programme_cursor = round_trip(
        first_results
            .programmes()
            .next()
            .expect("Programme matches remain"),
    );

    assert!(matches!(
        first.search(SearchRequest::new(
            SearchTerm::parse("news").expect("fixture term is valid"),
            PageRequest::first(limit(1)),
            PageRequest::after(channel_cursor.clone(), limit(1)),
        )),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));
    assert!(matches!(
        first.search(SearchRequest::new(
            SearchTerm::parse("weather").expect("fixture term is valid"),
            PageRequest::after(channel_cursor, limit(1)),
            PageRequest::first(limit(1)),
        )),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));

    let (same_restart, _) = core_with_guide(CHANNELS, GUIDE).await;
    assert!(
        same_restart
            .search(SearchRequest::new(
                SearchTerm::parse("news").expect("fixture term is valid"),
                PageRequest::first(limit(1)),
                PageRequest::after(programme_cursor.clone(), limit(1)),
            ))
            .is_ok(),
        "content-derived generations keep cursors compatible across restarts"
    );

    let mut changed_guide = GUIDE.to_vec();
    changed_guide.extend_from_slice(b"\n");
    let (changed, _) = core_with_guide(CHANNELS, &changed_guide).await;
    let current = changed
        .status()
        .generation()
        .expect("the changed catalog is published");
    assert!(matches!(
        changed.search(SearchRequest::new(
            SearchTerm::parse("news").expect("fixture term is valid"),
            PageRequest::first(limit(1)),
            PageRequest::after(programme_cursor, limit(1)),
        )),
        Err(CoreError::StaleCursor { current: actual }) if actual == current
    ));
}

#[tokio::test]
async fn channel_search_succeeds_without_epg_and_does_not_reopen_the_source() {
    let source = ScriptedSource::from_bytes(CHANNELS);
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/channels.m3u",
        None::<String>,
    ))
    .expect("the Channel-only configuration is valid");
    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), MemorySnapshotStore::default()),
    )
    .await
    .expect("Channel-only bootstrap is usable");

    for _ in 0..2 {
        let results = core
            .search(request("news", 100, 100))
            .expect("Channel search remains available without EPG");
        assert_eq!(
            names(results.channels()),
            ["News", "Newsroom", "Daily News Europe", "Goodnews Archive"]
        );
        assert!(results.programmes().items().is_empty());
        assert!(results.programmes().next().is_none());
    }
    assert_eq!(source.open_count_for(SourceKind::M3u), 1);
    assert_eq!(source.open_count_for(SourceKind::Epg), 0);
}

async fn core_with_guide(channels: &[u8], guide: &[u8]) -> (SparrowCore, ScriptedSource) {
    let source = ScriptedSource::from_bytes(channels.to_vec()).with_epg_bytes(guide.to_vec());
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://source-user:source-secret@provider.fixture.invalid/channels.m3u",
        Some("https://guide-user:guide-secret@provider.fixture.invalid/schedules.xml"),
    ))
    .expect("the fixture Source Configuration is valid");
    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), MemorySnapshotStore::default()),
    )
    .await
    .expect("fixture bootstrap remains usable");
    (core, source)
}

fn request(term: &str, channel_limit: u16, programme_limit: u16) -> SearchRequest {
    SearchRequest::new(
        SearchTerm::parse(term).expect("fixture term is valid"),
        PageRequest::first(limit(channel_limit)),
        PageRequest::first(limit(programme_limit)),
    )
}

fn limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("fixture limit is valid")
}

fn round_trip(cursor: &PageCursor) -> PageCursor {
    PageCursor::parse(cursor.as_str()).expect("generated cursor round-trips through transport")
}

fn names(page: &sparrow_core::Page<sparrow_core::ChannelSummary>) -> Vec<&str> {
    page.items().iter().map(|channel| channel.name()).collect()
}

fn titles(page: &sparrow_core::Page<sparrow_core::ProgrammeSummary>) -> Vec<&str> {
    page.items()
        .iter()
        .map(|programme| programme.title())
        .collect()
}
