mod support;

use std::collections::BTreeMap;

use sparrow_core::{
    ChannelQuery, CoreError, InputField, InputReason, PageCursor, PageLimit, PageRequest,
    SourceConfigurationInput, SparrowCore,
};
use support::{MemorySnapshotStore, ScriptedSource, adapters};

const BROWSE_M3U: &[u8] = include_bytes!("fixtures/browse_channels.m3u");
const REORDERED_BROWSE_M3U: &[u8] = include_bytes!("fixtures/browse_channels_reordered.m3u");
const SOURCE_LOCATION: &str = "https://source-user:source-secret@private-provider.fixture.invalid/browse.m3u?token=source-canary";

#[tokio::test]
async fn source_groups_are_deterministically_ordered_counted_and_bounded() {
    let (core, source) = browse_core(BROWSE_M3U).await;
    let first = core
        .list_groups(PageRequest::first(page_limit(2)))
        .expect("the first group page is available");

    assert_eq!(group_observations(&first), [("", 1), ("Culture", 1)]);
    assert_ne!(first.generation().get(), 0);
    assert_eq!(core.status().generation(), Some(first.generation()));
    let second_cursor = round_trip(first.next().expect("more groups remain"));
    let second = core
        .list_groups(PageRequest::after(second_cursor, page_limit(2)))
        .expect("the second group page is available");
    assert_eq!(group_observations(&second), [("Kids", 1), ("News", 3)]);
    let third_cursor = round_trip(second.next().expect("one group remains"));
    let third = core
        .list_groups(PageRequest::after(third_cursor, page_limit(2)))
        .expect("the final group page is available");
    assert_eq!(group_observations(&third), [("Sports", 2)]);
    assert!(third.next().is_none());
    assert_eq!(source.open_count(), 1);
}

#[tokio::test]
async fn channel_pages_are_stable_filtered_and_cover_exact_boundaries() {
    let (core, _) = browse_core(BROWSE_M3U).await;
    let first = core
        .list_channels(ChannelQuery::all(PageRequest::first(page_limit(4))))
        .expect("the first Channel page is available");
    let second = core
        .list_channels(ChannelQuery::all(PageRequest::after(
            round_trip(first.next().expect("an exact second page remains")),
            page_limit(4),
        )))
        .expect("the exact final Channel page is available");

    assert_eq!(first.items().len(), 4);
    assert_eq!(second.items().len(), 4);
    assert!(second.next().is_none());

    let observed = collect_all_channels(&core, 3);
    assert_eq!(observed.len(), 8);
    assert_eq!(
        observed
            .iter()
            .map(|(_, name, group)| (name.as_str(), group.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Ungrouped", ""),
            ("Museum", "Culture"),
            ("Junior", "Kids"),
            ("Alpha", "News"),
            ("Daily", "News"),
            ("Daily", "News"),
            ("Arena", "Sports"),
            ("Arena Two", "Sports"),
        ]
    );
    let mut ids = observed
        .iter()
        .map(|(id, _, _)| id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), observed.len());

    let news = core
        .list_channels(ChannelQuery::in_group(
            "News",
            PageRequest::first(page_limit(10)),
        ))
        .expect("the News group is queryable");
    assert_eq!(
        news.items()
            .iter()
            .map(|channel| channel.name())
            .collect::<Vec<_>>(),
        ["Alpha", "Daily", "Daily"]
    );
    assert!(news.next().is_none());

    let missing = core
        .list_channels(ChannelQuery::in_group(
            "Missing",
            PageRequest::first(page_limit(10)),
        ))
        .expect("an unknown group has an empty result");
    assert!(missing.items().is_empty());
    assert!(missing.next().is_none());
}

#[tokio::test]
async fn recognizable_entries_keep_duplicate_aware_ids_across_reorder_and_location_changes() {
    let (first, _) = browse_core(BROWSE_M3U).await;
    let (reordered, _) = browse_core(REORDERED_BROWSE_M3U).await;
    let first_ids = ids_by_recognizable_seed(&first);
    let reordered_ids = ids_by_recognizable_seed(&reordered);

    assert_eq!(first_ids, reordered_ids);
    assert_eq!(
        first_ids[&(String::from("news"), String::from("daily"))].len(),
        2
    );
    assert_ne!(
        first_ids[&(String::from("news"), String::from("daily"))][0],
        first_ids[&(String::from("news"), String::from("daily"))][1]
    );
    for ids in first_ids.values() {
        for id in ids {
            assert!(id.starts_with("ch1_"));
        }
    }
}

#[tokio::test]
async fn cursors_are_scoped_to_their_query_shape_and_expose_no_source_data() {
    let (core, _) = browse_core(BROWSE_M3U).await;
    let groups = core
        .list_groups(PageRequest::first(page_limit(1)))
        .expect("group page is available");
    let groups_cursor = round_trip(groups.next().expect("more groups remain"));

    assert!(matches!(
        core.list_channels(ChannelQuery::all(PageRequest::after(
            groups_cursor,
            page_limit(1),
        ))),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));

    let news = core
        .list_channels(ChannelQuery::in_group(
            "News",
            PageRequest::first(page_limit(1)),
        ))
        .expect("News page is available");
    let news_cursor = round_trip(news.next().expect("more News Channels remain"));
    assert!(matches!(
        core.list_channels(ChannelQuery::in_group(
            "Sports",
            PageRequest::after(news_cursor, page_limit(1)),
        )),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));

    let all = core
        .list_channels(ChannelQuery::all(PageRequest::first(page_limit(1))))
        .expect("Channel page is available");
    let diagnostic = format!("{groups:?} {all:?}");
    for private in [
        "source-user",
        "source-secret",
        "private-provider.fixture.invalid",
        "playback-user",
        "playback-secret",
        "private-media.fixture.invalid",
        "browse-canary",
        "source-canary",
    ] {
        assert!(
            !diagnostic.contains(private),
            "private marker leaked: {private}"
        );
        assert!(
            !all.next()
                .expect("more Channels remain")
                .as_str()
                .contains(private),
            "private marker leaked through cursor: {private}"
        );
    }
}

#[tokio::test]
async fn cursor_generations_are_restart_stable_but_invalidate_changed_catalog_inputs() {
    let (first, _) = browse_core(BROWSE_M3U).await;
    let first_page = first
        .list_channels(ChannelQuery::all(PageRequest::first(page_limit(1))))
        .expect("the first catalog is queryable");
    let cursor = round_trip(first_page.next().expect("more Channels remain"));

    let (same_restart, _) = browse_core(BROWSE_M3U).await;
    let continued = same_restart
        .list_channels(ChannelQuery::all(PageRequest::after(
            cursor.clone(),
            page_limit(1),
        )))
        .expect("the same configuration and snapshot retain cursor compatibility");
    assert_eq!(continued.generation(), first_page.generation());

    let (changed_snapshot, _) = browse_core(REORDERED_BROWSE_M3U).await;
    let changed_snapshot_generation = changed_snapshot
        .status()
        .generation()
        .expect("the changed snapshot published a catalog");
    assert_ne!(changed_snapshot_generation, first_page.generation());
    assert_eq!(
        changed_snapshot
            .list_channels(ChannelQuery::all(PageRequest::after(
                cursor.clone(),
                page_limit(1),
            )))
            .expect_err("a cursor cannot cross M3U snapshot content"),
        CoreError::StaleCursor {
            current: changed_snapshot_generation,
        }
    );

    let (changed_configuration, _) = browse_core_at(
        BROWSE_M3U,
        "https://private-provider.fixture.invalid/another-source.m3u?token=other-secret",
    )
    .await;
    let changed_configuration_generation = changed_configuration
        .status()
        .generation()
        .expect("the changed configuration published a catalog");
    assert_ne!(changed_configuration_generation, first_page.generation());
    assert_eq!(
        changed_configuration
            .list_channels(ChannelQuery::all(
                PageRequest::after(cursor, page_limit(1),)
            ))
            .expect_err("a cursor cannot cross Source Configurations"),
        CoreError::StaleCursor {
            current: changed_configuration_generation,
        }
    );
}

async fn browse_core(m3u: &[u8]) -> (SparrowCore, ScriptedSource) {
    browse_core_at(m3u, SOURCE_LOCATION).await
}

async fn browse_core_at(m3u: &[u8], source_location: &str) -> (SparrowCore, ScriptedSource) {
    let source = ScriptedSource::from_bytes(m3u.to_vec());
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        source_location,
        None::<String>,
    ))
    .expect("the fixture Source Configuration is valid");
    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), MemorySnapshotStore::default()),
    )
    .await
    .expect("bootstrap remains usable");
    (core, source)
}

fn collect_all_channels(core: &SparrowCore, limit: u16) -> Vec<(String, String, String)> {
    let mut request = PageRequest::first(page_limit(limit));
    let mut observed = Vec::new();
    loop {
        let page = core
            .list_channels(ChannelQuery::all(request))
            .expect("Channel page is available");
        assert!(page.items().len() <= usize::from(limit));
        observed.extend(page.items().iter().map(|channel| {
            (
                channel.id().as_str().to_owned(),
                channel.name().to_owned(),
                channel.group().to_owned(),
            )
        }));
        let Some(next) = page.next() else {
            break;
        };
        request = PageRequest::after(round_trip(next), page_limit(limit));
    }
    observed
}

fn ids_by_recognizable_seed(core: &SparrowCore) -> BTreeMap<(String, String), Vec<String>> {
    let mut by_seed = BTreeMap::<(String, String), Vec<String>>::new();
    for (id, name, group) in collect_all_channels(core, 2) {
        by_seed
            .entry((group.to_lowercase(), name.to_lowercase()))
            .or_default()
            .push(id);
    }
    for ids in by_seed.values_mut() {
        ids.sort_unstable();
    }
    by_seed
}

fn group_observations(
    page: &sparrow_core::Page<sparrow_core::ChannelGroupView>,
) -> Vec<(&str, u32)> {
    page.items()
        .iter()
        .map(|group| (group.name(), group.channel_count()))
        .collect()
}

fn round_trip(cursor: &PageCursor) -> PageCursor {
    PageCursor::parse(cursor.as_str()).expect("a generated cursor round-trips at the boundary")
}

fn page_limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("fixture page limit is valid")
}
