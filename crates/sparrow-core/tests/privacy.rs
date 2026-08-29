mod support;

use static_assertions::assert_not_impl_any;

use sparrow_core::{
    ChannelId, ChannelQuery, CoreEvent, PageLimit, PageRequest, RefreshTrigger, ScheduleQuery,
    SearchRequest, SearchTerm, SourceConfigurationInput, SourceKind, SourceResponse, SparrowCore,
};
use support::{MemorySnapshotStore, ScriptedSource, adapters};

assert_not_impl_any!(SourceConfigurationInput: std::fmt::Debug, std::fmt::Display);
assert_not_impl_any!(SourceResponse: std::fmt::Debug, std::fmt::Display);
assert_not_impl_any!(ChannelId: std::fmt::Display);
assert_not_impl_any!(SearchTerm: std::fmt::Display);

const PRIVATE_MARKERS: [&str; 11] = [
    "configuration-user",
    "configuration-secret",
    "private-provider.fixture.invalid",
    "playback-user",
    "playback-secret",
    "private-media.fixture.invalid",
    "payload-canary",
    "fingerprint-canary",
    "guide-user",
    "guide-secret",
    "private-guide.fixture.invalid",
];

#[tokio::test]
async fn public_diagnostics_and_read_models_do_not_expose_private_source_data() {
    let valid_m3u = br#"#EXTM3U
#EXTINF:-1 tvg-id="safe.one" group-title="News",Safe Channel
https://playback-user:playback-secret@private-media.fixture.invalid/live?token=payload-canary
"#;
    let valid_epg = br#"<tv>
<channel id="safe.one"><display-name>Safe Channel</display-name></channel>
<programme start="20260829120000 +0000" stop="20260829130000 +0000" channel="safe.one">
<title>Safe Programme</title>
</programme>
</tv>"#;
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://configuration-user:configuration-secret@private-provider.fixture.invalid/fingerprint-canary.m3u",
        Some("https://guide-user:guide-secret@private-guide.fixture.invalid/schedules.xml"),
    ))
    .expect("the fixture configuration is valid");
    assert_private_markers_absent(&format!("{configuration:?}"));

    let source = ScriptedSource::from_bytes(valid_m3u.to_vec()).with_epg_bytes(valid_epg.to_vec());
    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), MemorySnapshotStore::default()),
    )
    .await
    .expect("bootstrap remains usable");
    let page = core
        .list_channels(ChannelQuery::all(PageRequest::first(
            PageLimit::new(10).expect("valid page limit"),
        )))
        .expect("catalog is available");

    assert_private_markers_absent(&format!("{:?}", core.status()));
    assert_private_markers_absent(
        &source
            .request_debug()
            .expect("the deterministic source observed a request"),
    );
    assert_private_markers_absent(
        &source
            .request_debug_for(SourceKind::Epg)
            .expect("the deterministic source observed an EPG request"),
    );
    assert_private_markers_absent(&format!("{page:?}"));
    assert_private_markers_absent(&format!("{:?}", page.items()[0].id()));
    assert_private_markers_absent(page.items()[0].id().as_str());
    let schedule = core
        .schedule(ScheduleQuery::new(
            page.items()[0].id().clone(),
            PageRequest::first(PageLimit::new(10).expect("valid page limit")),
        ))
        .expect("the Programme schedule is available");
    assert_private_markers_absent(&format!("{schedule:?}"));
    let search = core
        .search(SearchRequest::new(
            SearchTerm::parse("Safe").expect("the fixture search term is valid"),
            PageRequest::first(PageLimit::new(10).expect("valid page limit")),
            PageRequest::first(PageLimit::new(10).expect("valid page limit")),
        ))
        .expect("the catalog is searchable");
    assert_private_markers_absent(&format!("{search:?}"));
    assert_private_markers_absent(&format!(
        "{:?}",
        SearchTerm::parse("configuration-secret")
            .expect("the private canary is a syntactically valid search term")
    ));

    let mut events = core.subscribe();
    let refresh = core.refresh(RefreshTrigger::Manual).await;
    assert_private_markers_absent(&format!("{refresh:?}"));
    loop {
        let event = events.recv().await.expect("the refresh event feed is open");
        assert_private_markers_absent(&format!("{event:?}"));
        if matches!(event, CoreEvent::RefreshCompleted { .. }) {
            break;
        }
    }

    let malformed = b"#EXTM3U\n#EXTINF:-1,payload-canary\nhttps://playback-user:playback-secret@private-media.fixture.invalid/live\n#EXTINF:-1,broken";
    let failing_configuration =
        SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
            "https://configuration-user:configuration-secret@private-provider.fixture.invalid/fingerprint-canary.m3u",
            None::<String>,
        ))
        .expect("the fixture configuration is valid");
    let failing_core = SparrowCore::bootstrap(
        Some(failing_configuration),
        adapters(
            ScriptedSource::from_bytes(malformed.to_vec()),
            MemorySnapshotStore::default(),
        ),
    )
    .await
    .expect("bootstrap remains usable after malformed input");
    let error = failing_core
        .list_channels(ChannelQuery::all(PageRequest::first(
            PageLimit::new(10).expect("valid page limit"),
        )))
        .expect_err("catalog is unavailable");

    assert_private_markers_absent(&format!("{error}"));
    assert_private_markers_absent(&format!("{error:?}"));
    assert_private_markers_absent(&format!("{:?}", failing_core.status()));
}

fn assert_private_markers_absent(output: &str) {
    for marker in PRIVATE_MARKERS {
        assert!(!output.contains(marker), "private marker leaked: {marker}");
    }
}
