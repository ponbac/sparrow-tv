use std::{sync::Arc, time::Duration};

use sparrow_core::CatalogAccess;
use sparrow_installed_catalog::{InstalledCatalog, SourceConfigurationUpdate};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

pub struct Fixture {
    catalog: Arc<InstalledCatalog>,
    _directory: TempDir,
}

impl Fixture {
    pub fn core(&self) -> &CatalogAccess {
        self.catalog.core()
    }
}

pub async fn open(m3u: String, epg: Option<String>) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let change = SourceConfigurationUpdate::parse(
        format!("{origin}/catalog.m3u"),
        epg.as_ref().map(|_| format!("{origin}/guide.xml")),
    )
    .unwrap();
    let replies = tokio::spawn(async move {
        for _ in 0..1 + usize::from(epg.is_some()) {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream)
                .read_line(&mut request)
                .await
                .unwrap();
            let body = match request.split_ascii_whitespace().nth(1) {
                Some("/catalog.m3u") => &m3u,
                Some("/guide.xml") => epg.as_ref().expect("requested fixture guide is present"),
                _ => panic!("unknown fixture request"),
            };
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(body.as_bytes()).await.unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let catalog = InstalledCatalog::prepare(directory.path())
        .unwrap()
        .finish_open()
        .await
        .unwrap();
    let status = timeout(
        Duration::from_secs(5),
        catalog.replace_configuration(change),
    )
    .await
    .expect("synthetic Source Configuration finishes")
    .unwrap();
    assert!(status.generation().is_some(), "fixture publishes a catalog");
    timeout(Duration::from_secs(5), replies)
        .await
        .expect("fixture requests arrive")
        .expect("fixture replies complete");
    Fixture {
        catalog,
        _directory: directory,
    }
}
