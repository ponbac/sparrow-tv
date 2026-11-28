use tokio::{io::AsyncBufReadExt, net::TcpListener};

use super::*;

const TOKEN: &str = "socket-contract-test-token";

#[test]
fn socket_contract_with_a_catalog_and_playback_runner() {
    let root = tempfile::tempdir().unwrap();
    let playback = Arc::new(DesktopPlayback::default());
    let catalog = Arc::new(Catalog::new(Some(playback.clone())));
    catalog.start(root.path().join("data"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", http.local_addr().unwrap());
        // A private source marker must never reach any Agent Control response.
        let playlist = format!(
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"c1\" group-title=\"Coast\",Coral Sound\n{base}/private-playback-source.ts\n\
             #EXTINF:-1 tvg-id=\"c2\" group-title=\"Coast\",Coral Sound Plus\n{base}/second.ts\n"
        );
        let fixture = tokio::spawn(serve_catalog(http, playlist));
        let seed = catalog.seed_fixture(format!("{base}/catalog"), format!("{base}/guide")).unwrap();
        assert!(timeout(Duration::from_secs(5), seed).await.unwrap().unwrap());
        let runner = catalog.runner().unwrap();
        let mut updates = runner.subscribe();
        let (ui, mut intents) = mpsc::channel(MAX_CONNECTIONS);
        let socket = root.path().join("agent.sock");
        let config = Config { socket: socket.clone(), token: TOKEN.into() };
        assert!(config.valid(Some(root.path())));
        let (shutdown, closing) = oneshot::channel();
        let server = tokio::spawn(serve(config, Dispatcher::new(catalog.clone(), playback, ui), closing));
        timeout(Duration::from_secs(2), async {
            while UnixStream::connect(&socket).await.is_err() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();

        assert_eq!(std::fs::metadata(&socket).unwrap().mode() & 0o777, 0o600);
        assert_eq!(rpc(&socket, "incorrect-token!", json!({"_tag": "ping"})).await,
            json!({"ok": false, "error": {"_tag": "unauthenticated"}}));
        assert_eq!(rpc(&socket, TOKEN, json!({"_tag": "ping", "extra": true})).await,
            json!({"ok": false, "error": {"_tag": "invalid-request"}}));
        let mut oversized = UnixStream::connect(&socket).await.unwrap();
        oversized.write_all(&vec![b'x'; MAX_REQUEST_BYTES]).await.unwrap();
        assert_eq!(response(oversized).await,
            json!({"ok": false, "error": {"_tag": "invalid-request"}}));
        assert_eq!(rpc(&socket, TOKEN, json!({"_tag": "ping"})).await,
            json!({"ok": true, "result": {"_tag": "pong", "ready": false}}));
        assert_eq!(rpc(&socket, TOKEN, json!({"_tag": "search", "term": "Coral"})).await,
            json!({"ok": true, "result": {"_tag": "channels", "channels": [
                {"name": "Coral Sound", "group": "Coast"},
                {"name": "Coral Sound Plus", "group": "Coast"}
            ]}}));
        assert_eq!(rpc(&socket, TOKEN, json!({"_tag": "tune", "term": "Coral"})).await,
            json!({"ok": false, "error": {"_tag": "ambiguous-channel",
                "names": ["Coral Sound", "Coral Sound Plus"]}}));

        let tune = start_rpc(&socket, TOKEN, json!({"_tag": "tune", "term": "  CORAL  SOUND  "})).await;
        accept(&mut intents).await.commit(|command| runner.commands().try_send(command).is_ok());
        assert_eq!(response(tune).await,
            json!({"ok": true, "result": {"_tag": "tuned", "name": "Coral Sound"}}));
        // With no shown window, the actual runner records the selected Channel
        // without contacting its Playback Source.
        timeout(Duration::from_secs(2), updates.wait_for(|state| state.selected.is_some()))
            .await.unwrap().unwrap();
        let snapshot = rpc(&socket, TOKEN, json!({"_tag": "snapshot"})).await;
        assert_eq!(snapshot, json!({"ok": true, "result": {
            "_tag": "snapshot", "channelName": "Coral Sound", "diagnostics": {
                "engine": "embedded-libmpv", "version": 2, "phase": "paused",
                "recoveryCount": 0, "media": null,
                "controls": {"volumePercent": 100.0, "fullscreen": false, "muted": false}
            }
        }}));
        assert!(!snapshot.to_string().contains("private-playback-source"));
        assert!(!snapshot.to_string().contains(&base));

        let tune = start_rpc(&socket, TOKEN, json!({"_tag": "tune", "term": "Coral Sound Plus"})).await;
        let superseded = accept(&mut intents).await;
        let stop = start_rpc(&socket, TOKEN, json!({"_tag": "stop"})).await;
        accept(&mut intents).await.commit(|command| runner.commands().try_send(command).is_ok());
        superseded.commit(|_| panic!("stop must cancel a tune that has not committed"));
        assert_eq!(response(stop).await, json!({"ok": true, "result": {"_tag": "stopped"}}));
        assert_eq!(response(tune).await, json!({"ok": false, "error": {"_tag": "timeout"}}));
        assert_eq!(rpc(&socket, TOKEN, json!({"_tag": "snapshot"})).await,
            json!({"ok": true, "result": {"_tag": "snapshot", "channelName": null, "diagnostics": null}}));

        shutdown.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert!(!socket.exists());
        fixture.abort();
        let _ = fixture.await;
    });
}

async fn accept(intents: &mut mpsc::Receiver<Intent>) -> Intent {
    timeout(Duration::from_secs(2), intents.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn serve_catalog(http: TcpListener, playlist: String) {
    loop {
        let (stream, _) = http.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut request = String::new();
        stream.read_line(&mut request).await.unwrap();
        let body = if request.starts_with("GET /catalog ") {
            &playlist
        } else {
            "<?xml version=\"1.0\"?><tv><channel id=\"c1\"><display-name>Coral Sound</display-name></channel></tv>"
        };
        loop {
            let mut line = String::new();
            if stream.read_line(&mut line).await.unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }
}

async fn start_rpc(socket: &Path, token: &str, request: Value) -> UnixStream {
    let mut stream = UnixStream::connect(socket).await.unwrap();
    let line = format!("{}\n", json!({ "token": token, "request": request }));
    stream.write_all(line.as_bytes()).await.unwrap();
    stream
}

async fn response(stream: UnixStream) -> Value {
    let mut line = String::new();
    timeout(
        Duration::from_secs(2),
        BufReader::new(stream).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn rpc(socket: &Path, token: &str, request: Value) -> Value {
    response(start_rpc(socket, token, request).await).await
}
