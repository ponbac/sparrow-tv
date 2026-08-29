mod support;

use static_assertions::assert_not_impl_any;

use sparrow_core::{ChannelId, PageLimit, SourceConfigurationInput, SourceResponse, SparrowCore};
use support::{MemorySnapshotStore, ScriptedSource, adapters};

assert_not_impl_any!(SourceConfigurationInput: std::fmt::Debug, std::fmt::Display);
assert_not_impl_any!(SourceResponse: std::fmt::Debug, std::fmt::Display);
assert_not_impl_any!(ChannelId: std::fmt::Display);

const PRIVATE_MARKERS: [&str; 8] = [
    "configuration-user",
    "configuration-secret",
    "private-provider.fixture.invalid",
    "playback-user",
    "playback-secret",
    "private-media.fixture.invalid",
    "payload-canary",
    "fingerprint-canary",
];

#[tokio::test]
async fn public_diagnostics_and_read_models_do_not_expose_private_source_data() {
    let valid_m3u = br#"#EXTM3U
#EXTINF:-1 tvg-id="safe.one" group-title="News",Safe Channel
https://playback-user:playback-secret@private-media.fixture.invalid/live?token=payload-canary
"#;
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://configuration-user:configuration-secret@private-provider.fixture.invalid/fingerprint-canary.m3u",
        None::<String>,
    ))
    .expect("the fixture configuration is valid");
    assert_private_markers_absent(&format!("{configuration:?}"));

    let source = ScriptedSource::from_bytes(valid_m3u.to_vec());
    let core = SparrowCore::bootstrap(
        Some(configuration),
        adapters(source.clone(), MemorySnapshotStore::default()),
    )
    .await
    .expect("bootstrap remains usable");
    let page = core
        .list_channels(PageLimit::new(10).expect("valid page limit"))
        .expect("catalog is available");

    assert_private_markers_absent(&format!("{:?}", core.status()));
    assert_private_markers_absent(
        &source
            .request_debug()
            .expect("the deterministic source observed a request"),
    );
    assert_private_markers_absent(&format!("{page:?}"));
    assert_private_markers_absent(&format!("{:?}", page.items()[0].id()));
    assert_private_markers_absent(page.items()[0].id().as_str());

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
        .list_channels(PageLimit::new(10).expect("valid page limit"))
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
