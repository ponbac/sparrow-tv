mod support;

use sparrow_core::{
    ChannelId, ChannelQuery, CoreError, PageLimit, PageRequest, RefreshOutcome, RefreshTrigger,
    ResolvedPlaybackSource, SourceConfigurationInput, SourceKind, SparrowCore,
};
use static_assertions::assert_not_impl_any;
use support::{MemorySnapshotStore, ScriptedSource, adapters};

assert_not_impl_any!(ResolvedPlaybackSource: std::fmt::Display, serde::Serialize);

const INITIAL_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="stable" group-title="News",Stable Channel
https://old-user:old-secret@private-media.fixture.invalid/live?token=old-playback-canary
"#;

const UPDATED_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="stable" group-title="News",Stable Channel
https://new-user:new-secret@replacement-media.fixture.invalid/live?token=new-playback-canary
"#;

const SOURCE_LOCATION: &str =
    "https://source-user:source-secret@private-provider.fixture.invalid/channels.m3u";

#[tokio::test]
async fn opaque_channel_id_resolves_to_a_private_playback_source() {
    let (core, _) = playback_core().await;
    let channel_id = only_channel_id(&core);

    let resolved = core
        .resolve_playback(&channel_id)
        .expect("the catalog Channel has a Playback Source");

    assert_eq!(
        resolved.location_for_adapter().as_str(),
        "https://old-user:old-secret@private-media.fixture.invalid/live?token=old-playback-canary"
    );
    assert_eq!(
        format!("{resolved:?}"),
        "ResolvedPlaybackSource(<redacted>)"
    );
    assert_private_markers_absent(&format!("{resolved:?}"));
}

#[tokio::test]
async fn unknown_channel_id_returns_the_safe_typed_catalog_failure() {
    let (core, _) = playback_core().await;
    let unknown = ChannelId::parse(format!("ch1_{}", "f".repeat(64)))
        .expect("the unknown fixture ID is canonical");

    let error = core
        .resolve_playback(&unknown)
        .expect_err("the unknown Channel cannot resolve playback");

    assert!(matches!(
        &error,
        CoreError::ChannelNotFound { id } if id == &unknown
    ));
    assert_eq!(error.to_string(), "the Channel was not found");
    assert_private_markers_absent(&format!("{error:?} {error}"));
}

#[tokio::test]
async fn resolved_source_stays_pinned_when_refresh_changes_the_catalog_location() {
    let (core, source) = playback_core().await;
    let channel_id = only_channel_id(&core);
    let initial_generation = core
        .status()
        .generation()
        .expect("the initial catalog is published");
    let pinned = core
        .resolve_playback(&channel_id)
        .expect("the initial Playback Source resolves");

    source.replace_bytes(SourceKind::M3u, UPDATED_M3U);
    let report = core.refresh(RefreshTrigger::Manual).await;
    assert!(matches!(report.m3u(), RefreshOutcome::Updated { .. }));
    assert_ne!(core.status().generation(), Some(initial_generation));

    let current = core
        .resolve_playback(&channel_id)
        .expect("the recognizable Channel retains its identifier");
    assert_eq!(
        pinned.location_for_adapter().as_str(),
        "https://old-user:old-secret@private-media.fixture.invalid/live?token=old-playback-canary"
    );
    assert_eq!(
        current.location_for_adapter().as_str(),
        "https://new-user:new-secret@replacement-media.fixture.invalid/live?token=new-playback-canary"
    );
    assert_private_markers_absent(&format!("{pinned:?} {current:?} {report:?}"));
}

async fn playback_core() -> (SparrowCore, ScriptedSource) {
    let source = ScriptedSource::from_bytes(INITIAL_M3U);
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        SOURCE_LOCATION,
        None::<String>,
    ))
    .expect("the fixture Source Configuration is valid");
    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), MemorySnapshotStore::default()),
    )
    .await
    .expect("the playback fixture core bootstraps");
    (core, source)
}

fn only_channel_id(core: &SparrowCore) -> ChannelId {
    core.list_channels(ChannelQuery::all(PageRequest::first(
        PageLimit::new(1).expect("the fixture page limit is valid"),
    )))
    .expect("the fixture catalog is queryable")
    .items()[0]
        .id()
        .clone()
}

fn assert_private_markers_absent(output: &str) {
    for marker in [
        "source-user",
        "source-secret",
        "private-provider.fixture.invalid",
        "old-user",
        "old-secret",
        "private-media.fixture.invalid",
        "old-playback-canary",
        "new-user",
        "new-secret",
        "replacement-media.fixture.invalid",
        "new-playback-canary",
    ] {
        assert!(!output.contains(marker), "private marker leaked: {marker}");
    }
}
