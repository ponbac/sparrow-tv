mod support;

use std::{
    collections::BTreeMap,
    future::Future,
    task::{Context, Poll},
};

use bytes::Bytes;
use sparrow_core::{
    ChannelId, ChannelQuery, CoreError, M3uFailureKind, PageLimit, PageRequest, SafeFailure,
    SnapshotOperation, SourceConfigurationInput, SourceKind, SourceReadError, SourceState,
    SparrowCore, StoreError,
};
use support::{
    CountingSnapshotStore, MemorySnapshotStore, PendingActivationSnapshotStore,
    PendingAppendSnapshotStore, ScriptedSource, adapters, counting_adapters,
    pending_activation_adapters, pending_append_adapters,
};

const VALID_M3U: &[u8] = include_bytes!("fixtures/valid_minimal.m3u");
const VALID_THEN_MALFORMED_M3U: &[u8] = include_bytes!("fixtures/valid_then_malformed.m3u");

#[tokio::test]
async fn valid_m3u_is_activated_and_published_as_a_queryable_catalog() {
    let source = ScriptedSource::from_bytes(VALID_M3U);
    let snapshots = MemorySnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://source-user:source-canary@provider.fixture.invalid/list.m3u?token=source-secret",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");

    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), snapshots.clone()),
    )
    .await
    .expect("bootstrap remains usable");
    let page = core
        .list_channels(first_channels(10))
        .expect("catalog is available");

    assert_ne!(page.generation().get(), 0);
    assert_eq!(core.status().generation(), Some(page.generation()));
    assert_eq!(page.items().len(), 2);
    assert_eq!(page.items()[0].name(), "Culture One");
    assert_eq!(page.items()[0].group(), "Culture");
    let parsed_id = ChannelId::parse(page.items()[0].id().as_str())
        .expect("a generated Channel Identifier round-trips at the public boundary");
    assert_eq!(&parsed_id, page.items()[0].id());

    let details = core
        .channel(page.items()[0].id())
        .expect("listed Channel is queryable");
    let repeated_page = core
        .list_channels(first_channels(10))
        .expect("catalog remains available");
    assert_eq!(details.name(), "Culture One");
    assert_eq!(details.group(), "Culture");
    assert_eq!(page.items().as_ptr(), repeated_page.items().as_ptr());
    assert_eq!(source.open_count(), 1);
    assert_eq!(snapshots.activation_count(), 1);
    assert_eq!(snapshots.discard_count(), 0);
}

#[tokio::test]
async fn malformed_tail_never_publishes_or_activates_a_partial_catalog() {
    let source = ScriptedSource::from_bytes(VALID_THEN_MALFORMED_M3U);
    let snapshots = MemorySnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/list.m3u?token=configuration-canary",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");

    let core = SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots.clone()))
        .await
        .expect("bootstrap remains usable when its M3U Source is invalid");

    assert_eq!(core.status().generation(), None);
    assert!(matches!(
        core.status().m3u(),
        SourceState::Unavailable {
            failure: Some(SafeFailure::InvalidFormat {
                entry: Some(2),
                reason: M3uFailureKind::IncompleteEntry,
            }),
        }
    ));
    assert!(matches!(
        core.list_channels(first_channels(10)),
        Err(CoreError::CatalogUnavailable { .. })
    ));
    assert_eq!(snapshots.activation_count(), 0);
    assert_eq!(snapshots.discard_count(), 1);
}

#[tokio::test]
async fn provider_quirks_are_normalized_without_replacing_the_display_name() {
    let mut m3u = vec![0xef, 0xbb, 0xbf];
    m3u.extend_from_slice(
        b"#EXTM3U url-tvg=\"https://guide.fixture.invalid/epg.xml\"\r\n\r\n\
#EXTINF:-1 xui-id=\"42\" group-title=\"  World   News \" tvg-name=\"EPG Alias, HD\" tvg-id=\"world.one\",  Display, One  \r\n\
#EXTVLCOPT:http-user-agent=fixture\r\n\
https://media.fixture.invalid/live/world?token=provider-quirk-canary\r\n",
    );
    let source = ScriptedSource::from_bytes(m3u);
    let snapshots = MemorySnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/list.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");

    let core = SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots))
        .await
        .expect("bootstrap remains usable");
    let page = core
        .list_channels(first_channels(10))
        .expect("catalog is available");

    assert_eq!(page.items().len(), 1);
    assert_eq!(page.items()[0].name(), "Display, One");
    assert_eq!(page.items()[0].group(), "World News");
}

#[tokio::test]
async fn unescaped_inner_quotes_in_a_provider_attribute_do_not_reject_the_source() {
    let m3u = br#"#EXTM3U
#EXTINF:-1 tvg-id="" tvg-name="Friday Premiere "Royal Hearts" (2026)" group-title="Drama",
https://media.fixture.invalid/live/friday-premiere
"#;
    let source = ScriptedSource::from_bytes(Bytes::from_static(m3u));
    let snapshots = MemorySnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/list.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");

    let core = SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots.clone()))
        .await
        .expect("bootstrap remains usable");

    assert!(matches!(core.status().m3u(), SourceState::Fresh { .. }));
    assert_eq!(snapshots.activation_count(), 1);
}

#[tokio::test]
async fn channel_identifiers_are_stable_opaque_scoped_and_duplicate_aware() {
    let first_feed = br#"#EXTM3U
#EXTINF:-1 tvg-id="news.one" group-title="News",News One
https://first-user:first-secret@media.fixture.invalid/live/primary
#EXTINF:-1 tvg-id="news.one" group-title="News",News One
https://first-user:first-secret@media.fixture.invalid/live/backup
#EXTINF:-1 group-title="Culture",Culture One
https://media.fixture.invalid/live/culture
"#;
    let reordered_with_new_locations = br#"#EXTM3U
#EXTINF:-1 group-title="  Culture  ",  Culture   One
https://changed-user:changed-secret@other.fixture.invalid/culture
#EXTINF:-1 tvg-id=" NEWS.ONE " group-title=" news ", NEWS ONE
https://other.fixture.invalid/news-a?token=changed-a
#EXTINF:-1 tvg-id="news.one" group-title="News",News One
https://other.fixture.invalid/news-b?token=changed-b
"#;

    let first = channel_ids_by_name(
        "https://source-user:source-secret@provider.fixture.invalid/a.m3u",
        first_feed,
    )
    .await;
    let refreshed = channel_ids_by_name(
        "https://source-user:source-secret@provider.fixture.invalid/a.m3u",
        reordered_with_new_locations,
    )
    .await;
    let another_configuration =
        channel_ids_by_name("https://provider.fixture.invalid/b.m3u", first_feed).await;

    assert_eq!(first["culture one"], refreshed["culture one"]);
    assert_eq!(first["news one"].len(), 2);
    assert_ne!(first["news one"][0], first["news one"][1]);

    let mut first_news = first["news one"].clone();
    first_news.sort();
    let mut refreshed_news = refreshed["news one"].clone();
    refreshed_news.sort();
    assert_eq!(first_news, refreshed_news);

    assert_ne!(first["culture one"], another_configuration["culture one"]);
    for id in first.values().flatten() {
        assert!(id.starts_with("ch1_"));
        for forbidden in [
            "news.one",
            "News One",
            "media.fixture.invalid",
            "first-user",
            "first-secret",
            "provider.fixture.invalid",
        ] {
            assert!(!id.contains(forbidden));
        }
    }
}

#[tokio::test]
async fn rejected_inputs_return_closed_failures_without_a_catalog() {
    let cases = [
        (
            "empty",
            ScriptedSource::from_bytes(Bytes::new()),
            SafeFailure::InvalidFormat {
                entry: None,
                reason: M3uFailureKind::MissingHeader,
            },
        ),
        (
            "header-only",
            ScriptedSource::from_bytes(Bytes::from_static(b"#EXTM3U\n")),
            SafeFailure::NoPlayableChannels,
        ),
        (
            "invalid-utf8",
            ScriptedSource::from_bytes(Bytes::from_static(b"#EXTM3U\n\xff\n")),
            SafeFailure::InvalidEncoding {
                kind: SourceKind::M3u,
            },
        ),
        (
            "unsupported-playback",
            ScriptedSource::from_bytes(Bytes::from_static(
                b"#EXTM3U\n#EXTINF:-1,Unsupported\nftp://fixture.invalid/live\n",
            )),
            SafeFailure::InvalidFormat {
                entry: Some(1),
                reason: M3uFailureKind::UnsupportedPlaybackSource,
            },
        ),
        (
            "unterminated-attribute",
            ScriptedSource::from_bytes(Bytes::from_static(
                b"#EXTM3U\n#EXTINF:-1 tvg-name=\"Never closed,Channel\nhttps://media.fixture.invalid/live\n",
            )),
            SafeFailure::InvalidFormat {
                entry: Some(1),
                reason: M3uFailureKind::UnterminatedQuote,
            },
        ),
        (
            "declared-oversize",
            ScriptedSource::from_chunks(
                vec![Ok(Bytes::from_static(VALID_M3U))],
                Some(128 * 1024 * 1024 + 1),
            ),
            SafeFailure::DecodedLimitExceeded {
                kind: SourceKind::M3u,
                limit_bytes: 128 * 1024 * 1024,
            },
        ),
        (
            "interrupted",
            ScriptedSource::from_chunks(
                vec![
                    Ok(Bytes::from_static(
                        b"#EXTM3U\n#EXTINF:-1,News One\nhttps://media.fixture.invalid/live\n",
                    )),
                    Err(SourceReadError::Interrupted),
                ],
                None,
            ),
            SafeFailure::SourceRead {
                kind: SourceKind::M3u,
                reason: SourceReadError::Interrupted,
            },
        ),
    ];

    for (name, source, expected_failure) in cases {
        let snapshots = MemorySnapshotStore::default();
        let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
            format!("https://provider.fixture.invalid/{name}.m3u?token=failure-canary"),
            None::<String>,
        ))
        .expect("the fixture configuration is valid");
        let core = SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots.clone()))
            .await
            .expect("bootstrap remains usable after a rejected source");

        assert_eq!(core.status().generation(), None, "case: {name}");
        assert_eq!(
            core.status().m3u(),
            &SourceState::Unavailable {
                failure: Some(expected_failure),
            },
            "case: {name}",
        );
        assert_eq!(snapshots.activation_count(), 0, "case: {name}");
    }
}

#[tokio::test]
async fn streamed_input_cannot_bypass_the_decoded_size_limit() {
    let one_mebibyte = Bytes::from(vec![b'x'; 1024 * 1024]);
    let chunks = (0..129)
        .map(|_| Ok(one_mebibyte.clone()))
        .collect::<Vec<_>>();
    let source = ScriptedSource::from_chunks(chunks, None);
    let snapshots = CountingSnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/oversize.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");

    let core = SparrowCore::bootstrap(
        Some(configuration),
        counting_adapters(source, snapshots.clone()),
    )
    .await
    .expect("bootstrap remains usable after an oversized source");

    assert!(matches!(
        core.status().m3u(),
        SourceState::Unavailable {
            failure: Some(SafeFailure::DecodedLimitExceeded {
                kind: SourceKind::M3u,
                limit_bytes: 134_217_728,
            }),
        }
    ));
    assert_eq!(snapshots.append_count(), 128);
    assert_eq!(snapshots.discard_count(), 1);
}

#[tokio::test]
async fn activation_failure_never_publishes_the_completed_candidate() {
    let source = ScriptedSource::from_bytes(VALID_M3U);
    let snapshots = MemorySnapshotStore::failing_activation(StoreError::Capacity);
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/activation-failure.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");

    let core = SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots.clone()))
        .await
        .expect("bootstrap remains usable after activation failure");

    assert_eq!(core.status().generation(), None);
    assert!(matches!(
        core.status().m3u(),
        SourceState::Unavailable {
            failure: Some(SafeFailure::Snapshot {
                kind: SourceKind::M3u,
                operation: SnapshotOperation::Activate,
                reason: StoreError::Capacity,
            }),
        }
    ));
    assert_eq!(snapshots.activation_count(), 0);
    assert_eq!(snapshots.discard_count(), 1);
}

#[test]
fn cancelling_bootstrap_discards_its_inactive_stage() {
    let source = ScriptedSource::from_bytes(VALID_M3U);
    let snapshots = PendingAppendSnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/cancelled.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");
    let mut bootstrap = Box::pin(SparrowCore::bootstrap(
        Some(configuration),
        pending_append_adapters(source, snapshots.clone()),
    ));
    let waker = futures_util::task::noop_waker();
    let mut context = Context::from_waker(&waker);

    assert!(matches!(
        bootstrap.as_mut().poll(&mut context),
        Poll::Pending
    ));
    assert!(snapshots.stage_is_active());

    drop(bootstrap);

    assert!(!snapshots.stage_is_active());
    assert_eq!(snapshots.discard_count(), 1);
}

#[test]
fn cancelling_activation_preparation_never_commits_the_candidate() {
    let source = ScriptedSource::from_bytes(VALID_M3U);
    let snapshots = PendingActivationSnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/cancelled-activation.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");
    let mut bootstrap = Box::pin(SparrowCore::bootstrap(
        Some(configuration),
        pending_activation_adapters(source, snapshots.clone()),
    ));
    let waker = futures_util::task::noop_waker();
    let mut context = Context::from_waker(&waker);

    assert!(matches!(
        bootstrap.as_mut().poll(&mut context),
        Poll::Pending
    ));
    assert!(snapshots.preparation_started());
    assert_eq!(snapshots.activation_count(), 0);

    drop(bootstrap);

    assert_eq!(snapshots.activation_count(), 0);
    assert_eq!(snapshots.discard_count(), 1);
}

async fn channel_ids_by_name(source_location: &str, m3u: &[u8]) -> BTreeMap<String, Vec<String>> {
    let source = ScriptedSource::from_bytes(m3u.to_vec());
    let snapshots = MemorySnapshotStore::default();
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        source_location,
        None::<String>,
    ))
    .expect("the fixture configuration is valid");
    let core = SparrowCore::bootstrap(Some(configuration), adapters(source, snapshots))
        .await
        .expect("bootstrap remains usable");
    let page = core
        .list_channels(first_channels(100))
        .expect("catalog is available");

    let mut by_name = BTreeMap::<String, Vec<String>>::new();
    for channel in page.items() {
        by_name
            .entry(channel.name().to_lowercase())
            .or_default()
            .push(channel.id().as_str().to_owned());
    }
    by_name
}

fn first_channels(limit: u16) -> ChannelQuery {
    ChannelQuery::all(PageRequest::first(
        PageLimit::new(limit).expect("valid page limit"),
    ))
}
