mod support;

use std::collections::BTreeMap;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use sparrow_core::{
    ChannelGroupFilter, ChannelId, ChannelQuery, CoreError, EpgFailureKind, GuideProgramme,
    GuideWindowQuery, InputField, InputReason, PageCursor, PageLimit, PageRequest, SafeFailure,
    ScheduleQuery, SourceAccessError, SourceConfigurationInput, SourceKind, SourceState,
    SparrowCore,
};
use support::{MemorySnapshotStore, ScriptedSource, adapters};

const CHANNELS: &[u8] = include_bytes!("fixtures/programme_channels.m3u");
const GUIDE: &[u8] = include_bytes!("fixtures/programme_schedules.xml");
const MALFORMED_GUIDE: &[u8] = include_bytes!("fixtures/malformed_programme_schedules.xml");
const MALFORMED_DOCUMENT: &[u8] = include_bytes!("fixtures/malformed_programme_document.xml");
const RECORD_QUIRKS: &[u8] = include_bytes!("fixtures/programme_record_quirks.xml");
// One Programme outlasts two shorter ones that start after it and end first.
const NESTED_OVERLAP_GUIDE: &[u8] = br#"<tv>
    <channel id="exact.id"><display-name>Exact</display-name></channel>
    <programme start="20260829070000 +0000" stop="20260829120000 +0000" channel="exact.id"><title>Long Running</title></programme>
    <programme start="20260829080000 +0000" stop="20260829083000 +0000" channel="exact.id"><title>Expired One</title></programme>
    <programme start="20260829090000 +0000" stop="20260829093000 +0000" channel="exact.id"><title>Expired Two</title></programme>
    <programme start="20260829103000 +0000" stop="20260829110000 +0000" channel="exact.id"><title>Future</title></programme>
</tv>"#;

#[tokio::test]
async fn guide_window_returns_one_channel_page_with_only_overlapping_programmes() {
    let (core, _, _) = core_with_guide(GUIDE).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");

    let page = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:30:00Z"),
                utc("2026-08-29T10:30:00Z"),
                first_channels(),
            )
            .expect("the three-hour guide window is valid"),
        )
        .expect("the guide window is queryable");
    let row = page
        .items()
        .iter()
        .find(|row| row.channel().id() == exact)
        .expect("the exact fixture Channel is in the guide page");
    let titles = row
        .programmes()
        .iter()
        .map(|programme| programme.title())
        .collect::<Vec<_>>();

    assert_eq!(titles, ["Earlier & First", "Later Programme"]);
    assert!(page.items().iter().all(|row| {
        row.programmes().iter().all(|programme| {
            programme.starts_at() < utc("2026-08-29T10:30:00Z")
                && programme.ends_at() > utc("2026-08-29T07:30:00Z")
        })
    }));

    let touching = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T08:00:00Z"),
                utc("2026-08-29T10:00:00Z"),
                first_channels(),
            )
            .expect("the boundary window is valid"),
        )
        .expect("the boundary window is queryable");
    let touching_row = touching
        .items()
        .iter()
        .find(|row| row.channel().id() == exact)
        .expect("the exact fixture Channel remains in the guide page");
    assert!(
        touching_row.programmes().is_empty(),
        "half-open windows exclude Programmes that only touch either boundary"
    );
}

#[tokio::test]
async fn guide_window_keeps_a_long_running_overlap_behind_expired_history() {
    let (core, _, _) = core_with_guide(NESTED_OVERLAP_GUIDE).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");

    let page = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T10:00:00Z"),
                utc("2026-08-29T10:15:00Z"),
                first_channels(),
            )
            .expect("the nested-overlap window is valid"),
        )
        .expect("the nested-overlap guide window is queryable");
    let row = page
        .items()
        .iter()
        .find(|row| row.channel().id() == exact)
        .expect("the exact fixture Channel is in the guide page");
    let titles = row
        .programmes()
        .iter()
        .map(|programme| programme.title())
        .collect::<Vec<_>>();

    assert_eq!(titles, ["Long Running"]);
    assert!(!row.programmes_truncated());
}

#[tokio::test]
async fn guide_window_pagination_is_scoped_to_the_exact_time_window() {
    let (core, _, _) = core_with_guide(GUIDE).await;
    let first = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::all(PageRequest::first(limit(1))),
            )
            .expect("the first guide window is valid"),
        )
        .expect("the first guide window is queryable");
    let cursor = round_trip(first.next().expect("the Channel page continues"));

    assert!(matches!(
        core.guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:01Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::all(PageRequest::after(cursor, limit(1))),
            )
            .expect("the shifted guide window is valid"),
        ),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));

    let first = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::all(PageRequest::first(limit(1))),
            )
            .expect("the ungrouped query is valid"),
        )
        .expect("the ungrouped query is available");
    let cursor = round_trip(first.next().expect("the ungrouped page continues"));
    let news = ChannelGroupFilter::parse("News").expect("the fixture group is valid");
    assert!(matches!(
        core.guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::in_group(news, PageRequest::after(cursor, limit(1))),
            )
            .expect("the grouped query is valid"),
        ),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));

    let unknown = ChannelGroupFilter::parse("Unknown").expect("the unknown group filter is valid");
    let empty = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::in_group(unknown, PageRequest::first(limit(10))),
            )
            .expect("the unknown-group query is valid"),
        )
        .expect("unknown groups produce an empty guide page");
    assert!(empty.items().is_empty());
    assert!(empty.next().is_none());
}

#[tokio::test]
async fn guide_window_around_a_channel_places_it_half_a_page_into_catalog_order() {
    let (core, _, _) = core_with_guide(GUIDE).await;
    let window = |channels: ChannelQuery| {
        GuideWindowQuery::new(
            utc("2026-08-29T07:00:00Z"),
            utc("2026-08-29T12:00:00Z"),
            channels,
        )
        .expect("the guide window is valid")
    };
    let catalog = core
        .guide_window(window(first_channels()))
        .expect("the whole fixture catalog fits one guide page");
    let rows = catalog.items();
    assert_eq!(rows.len(), 7);
    assert_ne!(
        rows[5].channel().group(),
        rows[6].channel().group(),
        "the fixture catalog spans two Channel Groups"
    );
    let around = |position: usize| {
        core.guide_window(
            window(ChannelQuery::all(PageRequest::first(limit(3))))
                .around(rows[position].channel().id().clone())
                .expect("a first all-Channels page can be placed around a Channel"),
        )
        .expect("a catalogued Channel has a guide page around it")
    };

    let middle = around(3);
    assert_eq!(middle.items(), &rows[2..5]);
    assert_eq!(middle.generation(), catalog.generation());
    let continued = core
        .guide_window(window(ChannelQuery::all(PageRequest::after(
            round_trip(middle.next().expect("Channels follow the middle page")),
            limit(3),
        ))))
        .expect("the continuation is an ordinary all-Channels cursor");
    assert_eq!(continued.items(), &rows[5..]);
    assert!(continued.next().is_none());

    assert_eq!(around(0).items(), &rows[..3]);

    let last = around(6);
    assert_eq!(last.items(), &rows[5..]);
    assert!(last.next().is_none());
}

#[tokio::test]
async fn guide_window_around_rejects_unknown_channels_groups_and_cursors() {
    let (core, _, _) = core_with_guide(GUIDE).await;
    let window = |channels: ChannelQuery| {
        GuideWindowQuery::new(
            utc("2026-08-29T07:00:00Z"),
            utc("2026-08-29T12:00:00Z"),
            channels,
        )
        .expect("the guide window is valid")
    };
    let first = core
        .guide_window(window(ChannelQuery::all(PageRequest::first(limit(1)))))
        .expect("the first guide page is queryable");
    let known = first.items()[0].channel().id().clone();

    let unknown = ChannelId::parse(format!("ch1_{}", "0".repeat(64)))
        .expect("the unknown identifier is canonical");
    assert_eq!(
        core.guide_window(
            window(ChannelQuery::all(PageRequest::first(limit(3))))
                .around(unknown.clone())
                .expect("an unknown Channel is only known to be missing by the catalog"),
        )
        .expect_err("an unknown Channel has no guide page around it"),
        CoreError::ChannelNotFound { id: unknown }
    );

    let news = ChannelGroupFilter::parse("News").expect("the fixture group is valid");
    assert_eq!(
        window(ChannelQuery::in_group(news, PageRequest::first(limit(3)))).around(known.clone()),
        Err(CoreError::InvalidInput {
            field: InputField::ChannelGroup,
            reason: InputReason::OutOfRange,
        })
    );
    let cursor = round_trip(first.next().expect("the Channel page continues"));
    assert_eq!(
        window(ChannelQuery::all(PageRequest::after(cursor, limit(3)))).around(known),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    );
}

#[tokio::test]
async fn guide_window_reports_when_overlapping_programmes_reach_the_row_cap() {
    let records = (0..=sparrow_core::GuideWindowChannel::MAX_PROGRAMMES)
        .map(|index| {
            format!(
                r#"<programme start="20260829070000 +0000" stop="20260829120000 +0000" channel="exact.id"><title>Programme {index}</title></programme>"#
            )
        })
        .collect::<String>();
    let guide = format!(
        r#"<tv><channel id="exact.id"><display-name>Exact</display-name></channel>{records}</tv>"#
    );
    let (core, _, _) = core_with_guide(guide.as_bytes()).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");
    let page = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T08:00:00Z"),
                utc("2026-08-29T09:00:00Z"),
                first_channels(),
            )
            .expect("the cap fixture window is valid"),
        )
        .expect("the cap fixture window is queryable");
    let row = page
        .items()
        .iter()
        .find(|row| row.channel().id() == exact)
        .expect("the exact fixture Channel is in the guide page");

    assert_eq!(
        row.programmes().len(),
        sparrow_core::GuideWindowChannel::MAX_PROGRAMMES
    );
    assert!(row.programmes_truncated());
}

#[tokio::test]
async fn guide_window_bounds_titles_and_omits_full_schedule_descriptions() {
    let title = "é".repeat(GuideProgramme::MAX_TITLE_BYTES);
    let description = "private-detail".repeat(1_024);
    let guide = format!(
        r#"<tv><channel id="exact.id"><display-name>Exact</display-name></channel><programme start="20260829070000 +0000" stop="20260829120000 +0000" channel="exact.id"><title>{title}</title><desc>{description}</desc></programme></tv>"#
    );
    let (core, _, _) = core_with_guide(guide.as_bytes()).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");
    let page = core
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T08:00:00Z"),
                utc("2026-08-29T09:00:00Z"),
                first_channels(),
            )
            .expect("the bounded-title window is valid"),
        )
        .expect("the bounded-title window is queryable");
    let programme = page
        .items()
        .iter()
        .find(|row| row.channel().id() == exact)
        .and_then(|row| row.programmes().first())
        .expect("the exact fixture Programme is in the guide window");

    assert_eq!(programme.title().len(), GuideProgramme::MAX_TITLE_BYTES);
    assert_eq!(
        programme.title().chars().count(),
        GuideProgramme::MAX_TITLE_BYTES / 2
    );
    assert!(programme.title_truncated());

    let schedule_page = core
        .schedule(schedule(exact.clone(), PageRequest::first(limit(1))))
        .expect("the ordinary schedule remains queryable");
    assert_eq!(schedule_page.items()[0].title(), title);
    assert_eq!(
        schedule_page.items()[0].description(),
        Some(description.as_str())
    );
}

#[test]
fn guide_windows_must_be_positive_and_no_longer_than_one_day() {
    for ends_at in [utc("2026-08-29T07:00:00Z"), utc("2026-08-30T07:00:01Z")] {
        assert!(matches!(
            GuideWindowQuery::new(utc("2026-08-29T07:00:00Z"), ends_at, first_channels(),),
            Err(CoreError::InvalidInput {
                field: InputField::GuideWindowEndsAt,
                reason: InputReason::OutOfRange,
            })
        ));
    }
}

#[test]
fn guide_window_transport_instants_share_one_bounded_core_parser() {
    let parsed = GuideWindowQuery::parse(
        "2026-08-29T09:00:00+02:00".to_owned(),
        "2026-08-29T10:00:00+02:00".to_owned(),
        first_channels(),
    )
    .expect("RFC 3339 offsets normalize at the core boundary");
    assert_eq!(parsed.starts_at(), utc("2026-08-29T07:00:00Z"));
    assert_eq!(parsed.ends_at(), utc("2026-08-29T08:00:00Z"));

    for (starts_at, ends_at, field, reason) in [
        (
            "not-an-instant".to_owned(),
            "2026-08-29T08:00:00Z".to_owned(),
            InputField::GuideWindowStartsAt,
            InputReason::InvalidFormat,
        ),
        (
            "2026-08-29T07:00:00Z".to_owned(),
            "x".repeat(GuideWindowQuery::MAX_INSTANT_BYTES + 1),
            InputField::GuideWindowEndsAt,
            InputReason::TooLong {
                max_bytes: GuideWindowQuery::MAX_INSTANT_BYTES,
            },
        ),
    ] {
        assert_eq!(
            GuideWindowQuery::parse(starts_at, ends_at, first_channels()),
            Err(CoreError::InvalidInput { field, reason })
        );
    }
}

#[tokio::test]
async fn exact_and_unique_name_matches_yield_ordered_bounded_utc_schedules() {
    let (core, source, snapshots) = core_with_guide(GUIDE).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");
    let fallback = one(&channels, "fallback one");

    let first = core
        .schedule(schedule(exact.clone(), PageRequest::first(limit(1))))
        .expect("the exact-ID schedule is queryable");
    assert_eq!(first.items().len(), 1);
    assert_eq!(first.items()[0].channel_id(), exact);
    assert_eq!(first.items()[0].title(), "Earlier & First");
    assert_eq!(
        first.items()[0].description(),
        Some("A normalized description")
    );
    assert_eq!(first.items()[0].starts_at(), utc("2026-08-29T07:00:00Z"));
    assert_eq!(first.items()[0].ends_at(), utc("2026-08-29T08:00:00Z"));

    let cursor = round_trip(first.next().expect("the exact schedule has another page"));
    let second = core
        .schedule(schedule(
            exact.clone(),
            PageRequest::after(cursor, limit(1)),
        ))
        .expect("the second exact-ID schedule page is queryable");
    assert_eq!(second.items()[0].title(), "Later Programme");
    assert_eq!(second.items()[0].starts_at(), utc("2026-08-29T10:00:00Z"));
    assert!(second.next().is_none());

    let fallback_page = core
        .schedule(schedule(fallback.clone(), PageRequest::first(limit(10))))
        .expect("the unique normalized-name fallback is queryable");
    assert_eq!(fallback_page.items().len(), 1);
    assert_eq!(fallback_page.items()[0].title(), "Fallback Programme");
    assert_eq!(fallback_page.items()[0].channel_id(), fallback);
    assert_eq!(
        fallback_page.items()[0].starts_at(),
        utc("2026-08-29T11:00:00Z")
    );

    let repeated = core
        .schedule(schedule(exact.clone(), PageRequest::first(limit(1))))
        .expect("the immutable schedule remains queryable");
    assert_eq!(first.items(), repeated.items());
    assert_eq!(source.open_count_for(SourceKind::M3u), 1);
    assert_eq!(source.open_count_for(SourceKind::Epg), 1);
    assert_eq!(snapshots.activation_count(), 2);
    assert!(matches!(core.status().m3u(), SourceState::Fresh { .. }));
    assert!(matches!(
        core.status().epg(),
        Some(SourceState::Fresh { .. })
    ));
}

#[tokio::test]
async fn schedule_from_an_instant_keeps_every_programme_still_to_end_and_pages_them() {
    let (core, _, _) = core_with_guide(NESTED_OVERLAP_GUIDE).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");
    let from = |instant: &str, page: PageRequest| {
        core.schedule(schedule(exact.clone(), page).with_from(utc(instant)))
    };
    let titles = |instant: &str| {
        from(instant, PageRequest::first(limit(10)))
            .expect("the schedule is queryable from an instant")
            .items()
            .iter()
            .map(|programme| programme.title().to_owned())
            .collect::<Vec<_>>()
    };

    assert_eq!(titles("2026-08-29T10:00:00Z"), ["Long Running", "Future"]);
    assert_eq!(
        titles("2026-08-29T09:29:59Z"),
        ["Long Running", "Expired Two", "Future"]
    );
    assert_eq!(
        titles("2026-08-29T09:30:00Z"),
        ["Long Running", "Future"],
        "a Programme that ends exactly at the instant is over"
    );
    assert_eq!(titles("2026-08-29T12:00:00Z"), Vec::<String>::new());

    let first = from("2026-08-29T10:00:00Z", PageRequest::first(limit(1)))
        .expect("the first page from an instant is queryable");
    assert_eq!(first.items().len(), 1);
    assert_eq!(first.items()[0].title(), "Long Running");
    let cursor = round_trip(first.next().expect("a later Programme remains"));
    let second = from(
        "2026-08-29T10:00:00Z",
        PageRequest::after(cursor.clone(), limit(1)),
    )
    .expect("the second page from the same instant is queryable");
    assert_eq!(second.items().len(), 1);
    assert_eq!(second.items()[0].title(), "Future");
    assert!(second.next().is_none());

    for mismatched in [
        from(
            "2026-08-29T10:00:01Z",
            PageRequest::after(cursor.clone(), limit(1)),
        ),
        core.schedule(schedule(
            exact.clone(),
            PageRequest::after(cursor, limit(1)),
        )),
    ] {
        assert!(matches!(
            mismatched,
            Err(CoreError::InvalidInput {
                field: InputField::PageCursor,
                reason: InputReason::CursorQueryMismatch,
            })
        ));
    }
}

#[test]
fn schedule_from_shares_the_bounded_core_instant_parser() {
    let channel = ChannelId::parse(format!("ch1_{}", "0".repeat(64)))
        .expect("the fixture identifier is canonical");
    let parse = |from: Option<String>| {
        ScheduleQuery::parse(channel.clone(), from, PageRequest::first(limit(10)))
    };

    assert_eq!(
        parse(Some("2026-08-29T12:00:00+02:00".to_owned()))
            .expect("RFC 3339 offsets normalize at the core boundary")
            .from(),
        Some(utc("2026-08-29T10:00:00Z"))
    );
    assert_eq!(parse(None).expect("the instant is optional").from(), None);
    for (from, reason) in [
        ("not-an-instant".to_owned(), InputReason::InvalidFormat),
        (
            "x".repeat(GuideWindowQuery::MAX_INSTANT_BYTES + 1),
            InputReason::TooLong {
                max_bytes: GuideWindowQuery::MAX_INSTANT_BYTES,
            },
        ),
    ] {
        assert_eq!(
            parse(Some(from)),
            Err(CoreError::InvalidInput {
                field: InputField::ScheduleFrom,
                reason,
            })
        );
    }
}

#[tokio::test]
async fn fallback_never_guesses_across_ambiguity_or_a_present_unmatched_id() {
    let (core, _, _) = core_with_guide(GUIDE).await;
    let channels = channel_ids_by_normalized_name(&core);

    assert_eq!(channels["ambiguous playlist"].len(), 2);
    for channel in &channels["ambiguous playlist"] {
        assert_schedule_empty(&core, channel);
    }
    assert_schedule_empty(&core, one(&channels, "ambiguous guide"));
    assert_schedule_empty(&core, one(&channels, "present id must not fallback"));
    assert_schedule_empty(&core, one(&channels, "unmatched channel"));

    let exact = one(&channels, "misleading name");
    let exact_schedule = core
        .schedule(schedule(exact.clone(), PageRequest::first(limit(10))))
        .expect("the exact-ID schedule is queryable");
    let titles = exact_schedule
        .items()
        .iter()
        .map(|programme| programme.title())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Earlier & First", "Later Programme"]);
    assert!(!titles.contains(&"Unassociated Programme"));
}

#[tokio::test]
async fn unusable_records_are_skipped_without_discarding_valid_programmes() {
    let (core, _, snapshots) = core_with_guide(RECORD_QUIRKS).await;
    let channels = channel_ids_by_normalized_name(&core);
    let exact = one(&channels, "misleading name");

    let page = core
        .schedule(schedule(exact.clone(), PageRequest::first(limit(10))))
        .expect("the guide remains queryable around unusable records");
    let titles = page
        .items()
        .iter()
        .map(|programme| programme.title())
        .collect::<Vec<_>>();

    assert_eq!(titles, ["Valid Before", "Valid After"]);
    assert!(page.next().is_none());
    assert!(matches!(
        core.status().epg(),
        Some(SourceState::Fresh { .. })
    ));
    assert_eq!(snapshots.activation_count(), 2);
    assert_eq!(snapshots.discard_count(), 0);
}

#[tokio::test]
async fn schedule_cursors_are_scoped_to_channel_and_epg_content_generation() {
    let (first, _, _) = core_with_guide(GUIDE).await;
    let first_channels = channel_ids_by_normalized_name(&first);
    let exact = one(&first_channels, "misleading name").clone();
    let cursor = first
        .schedule(schedule(exact.clone(), PageRequest::first(limit(1))))
        .expect("the first guide is queryable")
        .next()
        .expect("the first guide has another Programme")
        .clone();
    let fallback = one(&first_channels, "fallback one").clone();
    let guide_cursor = first
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::all(PageRequest::first(limit(1))),
            )
            .expect("the first guide window is valid"),
        )
        .expect("the first guide window is queryable")
        .next()
        .expect("the first guide Channel page continues")
        .clone();

    assert!(matches!(
        first.schedule(schedule(
            fallback,
            PageRequest::after(round_trip(&cursor), limit(1)),
        )),
        Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorQueryMismatch,
        })
    ));

    let changed_guide = String::from_utf8(GUIDE.to_vec())
        .expect("the fixture is UTF-8")
        .replace("Later Programme", "Changed Later Programme");
    let (changed, _, _) = core_with_guide(changed_guide.as_bytes()).await;
    let changed_generation = changed
        .status()
        .generation()
        .expect("the changed catalog is published");
    assert_ne!(
        first.status().generation(),
        Some(changed_generation),
        "the optional EPG checksum contributes to catalog generation"
    );
    assert!(matches!(
        changed.schedule(schedule(
            exact,
            PageRequest::after(round_trip(&cursor), limit(1)),
        )),
        Err(CoreError::StaleCursor { current }) if current == changed_generation
    ));
    assert!(matches!(
        changed.guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                ChannelQuery::all(PageRequest::after(
                    round_trip(&guide_cursor),
                    limit(1),
                )),
            )
            .expect("the changed guide window is valid"),
        ),
        Err(CoreError::StaleCursor { current }) if current == changed_generation
    ));
}

#[tokio::test]
async fn missing_or_failed_epg_keeps_the_channel_catalog_usable() {
    let no_guide_source = ScriptedSource::from_bytes(CHANNELS);
    let no_guide_configuration =
        SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/channels.m3u",
            None::<String>,
        ))
        .expect("the channel-only configuration is valid");
    let no_guide = SparrowCore::bootstrap(
        Some(no_guide_configuration),
        adapters(no_guide_source, MemorySnapshotStore::default()),
    )
    .await
    .expect("channel-only bootstrap remains usable");
    let no_guide_channel = channel_ids_by_normalized_name(&no_guide)["misleading name"]
        .first()
        .expect("the fixture Channel exists")
        .clone();
    assert_eq!(no_guide.status().epg(), None);
    assert_schedule_empty(&no_guide, &no_guide_channel);
    assert!(
        no_guide
            .schedule(
                schedule(no_guide_channel.clone(), PageRequest::first(limit(10)))
                    .with_from(utc("2026-08-29T07:00:00Z")),
            )
            .expect("a schedule without an EPG Source is queryable from an instant")
            .items()
            .is_empty()
    );
    let channel_only_guide = no_guide
        .guide_window(
            GuideWindowQuery::new(
                utc("2026-08-29T07:00:00Z"),
                utc("2026-08-29T12:00:00Z"),
                first_channels(),
            )
            .expect("the channel-only guide window is valid"),
        )
        .expect("the channel-only guide remains queryable");
    assert!(!channel_only_guide.items().is_empty());
    assert!(
        channel_only_guide
            .items()
            .iter()
            .all(|row| row.programmes().is_empty() && !row.programmes_truncated())
    );

    let failed_source = ScriptedSource::from_bytes(CHANNELS);
    let failed_snapshots = MemorySnapshotStore::default();
    let failed_configuration = configured_with_epg();
    let failed = SparrowCore::bootstrap(
        Some(failed_configuration),
        adapters(failed_source.clone(), failed_snapshots.clone()),
    )
    .await
    .expect("EPG access failure does not reject bootstrap");
    assert!(failed.list_channels(first_channels()).is_ok());
    assert!(matches!(
        failed.status().epg(),
        Some(SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::SourceAccess {
                kind: SourceKind::Epg,
                reason: SourceAccessError::Unavailable,
                ..
            },
            ..
        })
    ));
    assert_eq!(failed_source.open_count_for(SourceKind::M3u), 1);
    assert_eq!(failed_source.open_count_for(SourceKind::Epg), 1);
    assert_eq!(failed_snapshots.activation_count(), 1);
}

#[tokio::test]
async fn malformed_or_oversized_epg_is_typed_and_never_invalidates_m3u() {
    let malformed_source = ScriptedSource::from_bytes(CHANNELS).with_epg_bytes(MALFORMED_GUIDE);
    let malformed_snapshots = MemorySnapshotStore::default();
    let malformed = SparrowCore::bootstrap(
        Some(configured_with_epg()),
        adapters(malformed_source, malformed_snapshots.clone()),
    )
    .await
    .expect("malformed EPG does not reject bootstrap");
    assert!(malformed.list_channels(first_channels()).is_ok());
    assert!(matches!(
        malformed.status().epg(),
        Some(SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::InvalidEpgFormat {
                reason: EpgFailureKind::MalformedXml,
            },
            ..
        })
    ));
    assert_eq!(malformed_snapshots.activation_count(), 1);
    assert_eq!(malformed_snapshots.discard_count(), 1);

    let oversized_source = ScriptedSource::from_bytes(CHANNELS).with_epg_chunks(
        vec![Ok(Bytes::from_static(b"<tv/>"))],
        Some(64 * 1024 * 1024 + 1),
    );
    let oversized_snapshots = MemorySnapshotStore::default();
    let oversized = SparrowCore::bootstrap(
        Some(configured_with_epg()),
        adapters(oversized_source, oversized_snapshots.clone()),
    )
    .await
    .expect("oversized EPG does not reject bootstrap");
    assert!(oversized.list_channels(first_channels()).is_ok());
    assert!(matches!(
        oversized.status().epg(),
        Some(SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::DecodedLimitExceeded {
                kind: SourceKind::Epg,
                limit_bytes: 67_108_864,
            },
            ..
        })
    ));
    assert_eq!(oversized_snapshots.activation_count(), 1);
    assert_eq!(oversized_snapshots.discard_count(), 0);
}

#[tokio::test]
async fn malformed_document_state_and_no_valid_channels_are_typed() {
    let malformed_documents: [&[u8]; 9] = [
        MALFORMED_DOCUMENT,
        br#"<?xml?><tv><channel id="exact.id" /></tv>"#,
        br#"<?xml version="1.1"?><tv><channel id="exact.id" /></tv>"#,
        br#" <?xml version="1.0"?><tv><channel id="exact.id" /></tv>"#,
        br#"<!DOCTYPE tv><?xml version="1.0"?><tv><channel id="exact.id" /></tv>"#,
        br#"<?xml version="1.0"?><tv><channel id="exact.id" /></tv><?xml version="1.0"?>"#,
        br#"<!DOCTYPE tv><!DOCTYPE tv><tv><channel id="exact.id" /></tv>"#,
        br#"<tv><!DOCTYPE tv><channel id="exact.id" /></tv>"#,
        br#"<tv><channel id="exact.id" /></tv><![CDATA[ ]]>
"#,
    ];

    for document in malformed_documents {
        let (core, _, snapshots) = core_with_guide(document).await;
        assert!(core.list_channels(first_channels()).is_ok());
        assert!(matches!(
            core.status().epg(),
            Some(SourceState::Failed {
                validated_at: None,
                failure: SafeFailure::InvalidEpgFormat {
                    reason: EpgFailureKind::MalformedXml,
                },
                ..
            })
        ));
        assert_eq!(snapshots.activation_count(), 1);
        assert_eq!(snapshots.discard_count(), 1);
    }

    let (no_valid_channels, _, snapshots) =
        core_with_guide(br#"<tv><channel><display-name>Missing ID</display-name></channel></tv>"#)
            .await;
    assert!(no_valid_channels.list_channels(first_channels()).is_ok());
    assert!(matches!(
        no_valid_channels.status().epg(),
        Some(SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::NoEpgChannels,
            ..
        })
    ));
    assert_eq!(snapshots.activation_count(), 1);
    assert_eq!(snapshots.discard_count(), 1);
}

#[tokio::test]
async fn streamed_epg_cannot_bypass_its_decoded_size_limit() {
    let one_mebibyte = Bytes::from(vec![b'x'; 1024 * 1024]);
    let chunks = (0..65)
        .map(|_| Ok(one_mebibyte.clone()))
        .collect::<Vec<_>>();
    let source = ScriptedSource::from_bytes(CHANNELS).with_epg_chunks(chunks, None);
    let snapshots = MemorySnapshotStore::default();

    let core = SparrowCore::bootstrap(
        Some(configured_with_epg()),
        adapters(source, snapshots.clone()),
    )
    .await
    .expect("streamed EPG overflow does not reject bootstrap");

    assert!(core.list_channels(first_channels()).is_ok());
    assert!(matches!(
        core.status().epg(),
        Some(SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::DecodedLimitExceeded {
                kind: SourceKind::Epg,
                limit_bytes: 67_108_864,
            },
            ..
        })
    ));
    assert_eq!(snapshots.activation_count(), 1);
    assert_eq!(snapshots.discard_count(), 1);
}

async fn core_with_guide(guide: &[u8]) -> (SparrowCore, ScriptedSource, MemorySnapshotStore) {
    let source = ScriptedSource::from_bytes(CHANNELS).with_epg_bytes(guide.to_vec());
    let snapshots = MemorySnapshotStore::default();
    let core = SparrowCore::bootstrap(
        Some(configured_with_epg()),
        adapters(source.clone(), snapshots.clone()),
    )
    .await
    .expect("fixture bootstrap remains usable");
    (core, source, snapshots)
}

fn configured_with_epg() -> sparrow_core::SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://source-user:source-secret@provider.fixture.invalid/channels.m3u",
        Some("https://guide-user:guide-secret@provider.fixture.invalid/schedules.xml"),
    ))
    .expect("the fixture Source Configuration is valid")
}

fn channel_ids_by_normalized_name(core: &SparrowCore) -> BTreeMap<String, Vec<ChannelId>> {
    let page = core
        .list_channels(first_channels())
        .expect("the Channel Catalog is available");
    let mut channels = BTreeMap::<String, Vec<ChannelId>>::new();
    for channel in page.items() {
        channels
            .entry(channel.name().to_lowercase())
            .or_default()
            .push(channel.id().clone());
    }
    channels
}

fn one<'a>(channels: &'a BTreeMap<String, Vec<ChannelId>>, name: &str) -> &'a ChannelId {
    let matches = &channels[name];
    assert_eq!(
        matches.len(),
        1,
        "expected one fixture Channel named {name}"
    );
    &matches[0]
}

fn assert_schedule_empty(core: &SparrowCore, channel: &ChannelId) {
    let page = core
        .schedule(schedule(channel.clone(), PageRequest::first(limit(10))))
        .expect("an unmatched Channel has a valid empty schedule");
    assert!(page.items().is_empty());
    assert!(page.next().is_none());
}

fn first_channels() -> ChannelQuery {
    ChannelQuery::all(PageRequest::first(limit(100)))
}

fn schedule(channel_id: ChannelId, page: PageRequest) -> ScheduleQuery {
    ScheduleQuery::new(channel_id, page)
}

fn limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("the fixture page limit is valid")
}

fn round_trip(cursor: &PageCursor) -> PageCursor {
    PageCursor::parse(cursor.as_str()).expect("a generated cursor round-trips")
}

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("the expected timestamp is valid")
        .with_timezone(&Utc)
}
