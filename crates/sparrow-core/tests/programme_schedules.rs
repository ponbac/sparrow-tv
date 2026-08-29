mod support;

use std::collections::BTreeMap;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use sparrow_core::{
    ChannelId, ChannelQuery, CoreError, EpgFailureKind, InputField, InputReason, PageCursor,
    PageLimit, PageRequest, SafeFailure, ScheduleQuery, SourceAccessError,
    SourceConfigurationInput, SourceKind, SourceState, SparrowCore,
};
use support::{MemorySnapshotStore, ScriptedSource, adapters};

const CHANNELS: &[u8] = include_bytes!("fixtures/programme_channels.m3u");
const GUIDE: &[u8] = include_bytes!("fixtures/programme_schedules.xml");
const MALFORMED_GUIDE: &[u8] = include_bytes!("fixtures/malformed_programme_schedules.xml");
const MALFORMED_DOCUMENT: &[u8] = include_bytes!("fixtures/malformed_programme_document.xml");
const RECORD_QUIRKS: &[u8] = include_bytes!("fixtures/programme_record_quirks.xml");

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
    assert_eq!(first.items().as_ptr(), repeated.items().as_ptr());
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
        Some(SourceState::Unavailable {
            failure: Some(SafeFailure::SourceAccess {
                kind: SourceKind::Epg,
                reason: SourceAccessError::Unavailable,
            }),
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
        Some(SourceState::Unavailable {
            failure: Some(SafeFailure::InvalidEpgFormat {
                reason: EpgFailureKind::MalformedXml,
            }),
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
        Some(SourceState::Unavailable {
            failure: Some(SafeFailure::DecodedLimitExceeded {
                kind: SourceKind::Epg,
                limit_bytes: 67_108_864,
            }),
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
            Some(SourceState::Unavailable {
                failure: Some(SafeFailure::InvalidEpgFormat {
                    reason: EpgFailureKind::MalformedXml,
                }),
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
        Some(SourceState::Unavailable {
            failure: Some(SafeFailure::NoEpgChannels),
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
        Some(SourceState::Unavailable {
            failure: Some(SafeFailure::DecodedLimitExceeded {
                kind: SourceKind::Epg,
                limit_bytes: 67_108_864,
            }),
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
