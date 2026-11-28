#![cfg(all(target_os = "linux", feature = "embedded"))]

use std::{sync::Arc, time::Duration};

use sparrow_core::{ChannelQuery, PageLimit, PageRequest};
use sparrow_installed_catalog::{InstalledCatalog, SourceConfigurationUpdate};
use sparrow_installed_playback::{
    PlaybackManager,
    android::AndroidPlaybackViewport,
    noop_screen_wake,
    tv::{TvPlaybackCommand, TvPlaybackPhase, TvPlaybackRunner, TvTuneTarget},
};
use sparrow_mpv_player::{Backend, MpvPresentation};
use sparrow_source_http::HttpPlaybackAccess;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::{sleep, timeout},
};

const DEADLINE: Duration = Duration::from_secs(10);
const CLIP: &[u8] = include_bytes!("fixtures/blue.mpegts");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plays_an_opaque_source_retains_its_picture_on_pause_and_clears_it_on_stop() {
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", http.local_addr().unwrap());
    let playlist = format!("#EXTM3U\n#EXTINF:-1,Fixture\n{origin}/private-playback-source.ts\n");
    let server = tokio::spawn(serve_fixture(http, playlist));
    let root = tempfile::tempdir().unwrap();
    let catalog = InstalledCatalog::prepare(root.path())
        .unwrap()
        .finish_open()
        .await
        .unwrap();
    catalog
        .replace_configuration(
            SourceConfigurationUpdate::parse(format!("{origin}/catalog.m3u"), None).unwrap(),
        )
        .await
        .unwrap();
    let core = catalog.core().clone();
    let channel = timeout(DEADLINE, async {
        loop {
            let query = ChannelQuery::all(PageRequest::first(PageLimit::new(1).unwrap()));
            if let Ok(page) = core.list_channels(query)
                && let Some(channel) = page.items().first()
            {
                break channel.id().clone();
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture catalog should be ready");

    let manager = Arc::new(PlaybackManager::new(
        core.clone(),
        HttpPlaybackAccess::new().unwrap(),
        catalog.private_root().to_path_buf(),
        noop_screen_wake(),
    ));
    let player = Arc::new(MpvPresentation::new(&manager, Backend::Software).unwrap());
    let runner = TvPlaybackRunner::spawn(core, manager, player.clone()).unwrap();
    player.connect(&runner);
    let mut state = runner.subscribe();
    let viewport = AndroidPlaybackViewport::parse(0, 0, 64, 48, false).unwrap();
    runner.set_viewports(1, Some(viewport), None);
    runner.host_visibility(1, true).await.unwrap();
    let commands = runner.commands();
    commands
        .send(TvPlaybackCommand::Tune {
            target: TvTuneTarget::new(channel, 1),
            repeat: false,
        })
        .await
        .unwrap();
    let playing = timeout(
        DEADLINE,
        state.wait_for(|state| {
            matches!(
                state.phase,
                TvPlaybackPhase::Playing | TvPlaybackPhase::Failed { .. }
            )
        }),
    )
    .await
    .is_ok();
    let phase = state.borrow().phase;
    assert!(playing && phase == TvPlaybackPhase::Playing, "{phase:?}");
    let frame = player.latest_frame().expect("visible picture");
    assert_eq!((frame.width, frame.height), (64, 48));
    let [blue, _, red, alpha] = frame.bgra.as_chunks::<4>().0[0];
    assert!(blue > 200 && red < 30 && alpha == 255);
    assert!(player.diagnostics().await.unwrap().opaque_playlist);

    commands.send(TvPlaybackCommand::Pause).await.unwrap();
    timeout(
        DEADLINE,
        state.wait_for(|state| matches!(state.phase, TvPlaybackPhase::Paused(_))),
    )
    .await
    .expect("pause should release the transport")
    .unwrap();
    assert_eq!(player.latest_frame().unwrap().key, frame.key);
    assert!(player.diagnostics().await.is_err());

    timeout(DEADLINE, runner.stop()).await.unwrap().unwrap();
    assert!(player.latest_frame().is_none());
    server.abort();
}

async fn serve_fixture(http: TcpListener, playlist: String) {
    while let Ok((stream, _)) = http.accept().await {
        let mut stream = BufReader::new(stream);
        let mut buffer = String::new();
        stream.read_line(&mut buffer).await.unwrap();
        let body = if buffer.starts_with("GET /catalog.m3u ") {
            playlist.as_bytes()
        } else {
            assert!(buffer.starts_with("GET /private-playback-source.ts "));
            CLIP
        };
        loop {
            buffer.clear();
            if stream.read_line(&mut buffer).await.unwrap() == 0 || buffer == "\r\n" {
                break;
            }
        }
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
    }
}
