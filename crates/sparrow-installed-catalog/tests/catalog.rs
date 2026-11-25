use std::{
    future::{Future, poll_fn},
    sync::Arc,
    task::Poll,
    time::Duration,
};

use sparrow_core::{ChannelQuery, CoreEvent, PageLimit, PageRequest};
use sparrow_installed_catalog::{CatalogStartupError, InstalledCatalog, SourceConfigurationUpdate};
use tempfile::tempdir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

#[tokio::test]
async fn instance_lock_survives_bootstrap_and_shared_catalog_access() {
    let directory = tempdir().unwrap();
    let prepared = InstalledCatalog::prepare(directory.path()).unwrap();
    assert_eq!(
        InstalledCatalog::prepare(directory.path()).err(),
        Some(CatalogStartupError::AlreadyRunning)
    );

    let catalog = prepared.finish_open().await.unwrap();
    assert!(!catalog.core().status().configuration().is_configured());
    assert_eq!(
        InstalledCatalog::prepare(directory.path()).err(),
        Some(CatalogStartupError::AlreadyRunning)
    );
    let access = catalog.core().clone();
    drop(catalog);
    assert_eq!(
        InstalledCatalog::prepare(directory.path()).err(),
        Some(CatalogStartupError::AlreadyRunning)
    );
    drop(access);

    assert!(InstalledCatalog::prepare(directory.path()).is_ok());
}

#[test]
fn cancelled_startup_retains_the_instance_lock_while_snapshot_open_is_queued() {
    let directory = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
        });
        waiting.await.unwrap();

        let prepared = InstalledCatalog::prepare(directory.path()).unwrap();
        let mut opening = Box::pin(prepared.finish_open());
        assert!(poll_fn(|cx| Poll::Ready(opening.as_mut().poll(cx).is_pending())).await);
        drop(opening);

        let error = InstalledCatalog::prepare(directory.path()).err();
        release.send(()).unwrap();
        occupied.await.unwrap();
        assert_eq!(error, Some(CatalogStartupError::AlreadyRunning));
    });
}

#[test]
fn native_caller_can_restore_its_catalog_when_the_source_is_offline() {
    let directory = tempdir().unwrap();
    let start_runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    };
    let generation = start_runtime().block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source_location = format!("http://{}/catalog.m3u", listener.local_addr().unwrap());
        let reply = tokio::spawn(serve_catalog_fixture(listener));
        let catalog = InstalledCatalog::prepare(directory.path())
            .unwrap()
            .finish_open()
            .await
            .unwrap();
        let change =
            SourceConfigurationUpdate::parse(format!("  {source_location}  "), Some(" ".into()))
                .unwrap();
        let status = timeout(
            Duration::from_secs(5),
            catalog.replace_configuration(change),
        )
        .await
        .unwrap()
        .unwrap();
        reply.await.unwrap();
        status.generation().expect("the fixture produces a catalog")
    });

    start_runtime().block_on(async {
        let offline = InstalledCatalog::prepare(directory.path())
            .unwrap()
            .finish_open()
            .await
            .unwrap();
        let status = offline.core().status();
        assert!(status.configuration().is_configured());
        assert_eq!(status.generation(), Some(generation));
        let page = PageRequest::first(PageLimit::new(10).unwrap());
        let channels = offline
            .core()
            .list_channels(ChannelQuery::all(page))
            .unwrap();
        assert_eq!(channels.items().len(), 1);
        assert_eq!(channels.items()[0].name(), "Test News");
    });
}

#[tokio::test]
async fn started_replacement_publishes_the_catalog_when_its_caller_stops_waiting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_location = format!("http://{}/catalog.m3u", listener.local_addr().unwrap());
    let reply = tokio::spawn(serve_catalog_fixture(listener));
    let directory = tempdir().unwrap();
    let catalog = InstalledCatalog::prepare(directory.path())
        .unwrap()
        .finish_open()
        .await
        .unwrap();
    let mut events = catalog.core().subscribe();

    let change = SourceConfigurationUpdate::parse(source_location, None).unwrap();
    let mut replacement = Box::pin(catalog.replace_configuration(change));
    assert!(poll_fn(|cx| Poll::Ready(replacement.as_mut().poll(cx).is_pending())).await);
    drop(replacement);

    let generation = timeout(Duration::from_secs(5), async {
        loop {
            if let CoreEvent::CatalogPublished { generation, .. } = events
                .recv()
                .await
                .expect("catalog event stream stays open")
            {
                break generation;
            }
        }
    })
    .await
    .expect("the catalog publishes even when the caller stops waiting");
    reply.await.unwrap();
    assert_eq!(catalog.core().status().generation(), Some(generation));
}

#[test]
fn a_status_read_from_a_plain_thread_refreshes_the_saved_source() {
    let directory = tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let start_runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    };
    let source = format!("http://{}/catalog.m3u", listener.local_addr().unwrap());
    start_runtime().block_on(async {
        let unavailable = TcpListener::from_std(listener.try_clone().unwrap()).unwrap();
        let reply = tokio::spawn(serve_fixture_response(unavailable, "503 Unavailable", ""));
        let catalog = InstalledCatalog::prepare(directory.path())
            .unwrap()
            .finish_open()
            .await
            .unwrap();
        let change = SourceConfigurationUpdate::parse(source, None).unwrap();
        let status = timeout(
            Duration::from_secs(5),
            catalog.replace_configuration(change),
        )
        .await
        .unwrap()
        .unwrap();
        reply.await.unwrap();
        assert!(status.configuration().is_configured());
        assert!(status.generation().is_none());
    });

    start_runtime().block_on(async {
        let listener = TcpListener::from_std(listener).unwrap();
        let reply = tokio::spawn(serve_catalog_fixture(listener));
        let catalog = InstalledCatalog::prepare(directory.path())
            .unwrap()
            .finish_open()
            .await
            .unwrap();
        let mut events = catalog.core().subscribe();
        let ui_catalog = Arc::clone(&catalog);
        let initial = std::thread::spawn(move || ui_catalog.status())
            .join()
            .unwrap();
        assert!(initial.configuration().is_configured());
        assert!(initial.generation().is_none());
        let generation = timeout(Duration::from_secs(5), async {
            loop {
                if let CoreEvent::CatalogPublished { generation, .. } = events
                    .recv()
                    .await
                    .expect("catalog event stream stays open")
                {
                    break generation;
                }
            }
        })
        .await
        .expect("the status read starts a refresh on the catalog runtime");
        reply.await.unwrap();
        assert_eq!(catalog.core().status().generation(), Some(generation));
    });
}

async fn serve_catalog_fixture(listener: TcpListener) {
    const M3U: &str =
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",Test News\nhttp://127.0.0.1:9/live\n";
    serve_fixture_response(listener, "200 OK", M3U).await;
}

async fn serve_fixture_response(listener: TcpListener, status: &str, body: &str) {
    let (mut connection, _) = listener.accept().await.unwrap();
    let mut request = String::new();
    BufReader::new(&mut connection)
        .read_line(&mut request)
        .await
        .unwrap();
    assert!(request.starts_with("GET /catalog.m3u "));
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    connection.write_all(response.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn saved_source_locations_return_what_the_viewer_last_saved() {
    let directory = tempdir().unwrap();
    let catalog = InstalledCatalog::prepare(directory.path())
        .unwrap()
        .finish_open()
        .await
        .unwrap();
    assert!(catalog.saved_source_locations().await.unwrap().is_none());

    let origin = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let change = SourceConfigurationUpdate::parse(
        format!(" {origin}/catalog.m3u "),
        Some(format!("{origin}/guide.xml")),
    )
    .unwrap();
    timeout(
        Duration::from_secs(10),
        catalog.replace_configuration(change),
    )
    .await
    .unwrap()
    .unwrap();

    let (m3u, epg) = catalog
        .saved_source_locations()
        .await
        .unwrap()
        .expect("the configuration was saved")
        .into_locations();
    assert_eq!(m3u, format!("{origin}/catalog.m3u"));
    assert_eq!(epg, Some(format!("{origin}/guide.xml")));
}
