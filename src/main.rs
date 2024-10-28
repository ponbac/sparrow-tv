use futures::StreamExt;
use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{body::Body, extract::Path, http::Response, routing::get, Router};
use epg::Epg;
use playlist::Playlist;
use tokio::net::TcpListener;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

mod epg;
mod playlist;
mod routes;

trait FileFetch {
    fn is_stale(&self) -> bool;
}

#[derive(Debug)]
struct PlaylistFetch {
    playlist: Playlist,
    fetched: time::Instant,
}

#[derive(Debug)]
struct EpgFetch {
    epg: Epg,
    fetched: time::Instant,
}

impl FileFetch for PlaylistFetch {
    fn is_stale(&self) -> bool {
        self.fetched.elapsed() > time::Duration::hours(6)
    }
}

impl FileFetch for EpgFetch {
    fn is_stale(&self) -> bool {
        self.fetched.elapsed() > time::Duration::hours(6)
    }
}

#[derive(Debug, Clone)]
struct AppState {
    pub cached_playlist: Arc<RwLock<Option<PlaylistFetch>>>,
    pub cached_epg: Arc<RwLock<Option<EpgFetch>>>,
}

impl AppState {
    fn new() -> Self {
        #[cfg(debug_assertions)]
        {
            let playlist_file = std::fs::read_to_string("./examples/playlist.m3u").unwrap();
            let playlist: Playlist = playlist_file.parse().unwrap();
            let epg_file = std::fs::File::open("./examples/epg.xml").unwrap();
            let epg = Epg::from_reader(epg_file).unwrap();

            let now = time::Instant::now();
            let in_a_year = now + time::Duration::days(365);
            return Self {
                cached_playlist: Arc::new(RwLock::new(Some(PlaylistFetch {
                    playlist,
                    fetched: in_a_year,
                }))),
                cached_epg: Arc::new(RwLock::new(Some(EpgFetch {
                    epg,
                    fetched: in_a_year,
                }))),
            };
        }

        Self {
            cached_playlist: Arc::new(RwLock::new(None)),
            cached_epg: Arc::new(RwLock::new(None)),
        }
    }

    async fn fetch_playlist(&self) -> Result<Playlist, reqwest::Error> {
        {
            let cached_playlist = self.cached_playlist.read().unwrap();
            if let Some(playlist_fetch) = &*cached_playlist {
                if !playlist_fetch.is_stale() {
                    return Ok(playlist_fetch.playlist.clone());
                }
            }
        }

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
            .build()?;
        let response = client
            .get(std::env::var("M3U_PATH").unwrap())
            .send()
            .await?;
        if response.status() != 200 {
            tracing::error!("Received a non-200 response: {:?}", response);
        }
        let playlist_content = response.text().await?;
        let mut playlist: Playlist = playlist_content.parse().expect("Failed to parse playlist");
        playlist.exclude_groups(GROUPS_TO_EXCLUDE.to_vec());
        playlist.exclude_containing(SNIPPETS_TO_EXCLUDE.to_vec());
        playlist.exclude_all_extensions();
        tracing::info!(
            "Fetched playlist with {} groups:\n{}",
            playlist.filtered_groups().len(),
            playlist.filtered_groups().join("\n")
        );

        let mut cached_playlist = self.cached_playlist.write().unwrap();
        *cached_playlist = Some(PlaylistFetch {
            playlist: playlist.clone(),
            fetched: time::Instant::now(),
        });

        Ok(playlist)
    }

    async fn fetch_epg(&self) -> Result<Epg, Box<dyn std::error::Error>> {
        {
            let cached_epg = self.cached_epg.read().unwrap();
            if let Some(epg_fetch) = &*cached_epg {
                if !epg_fetch.is_stale() {
                    return Ok(epg_fetch.epg.clone());
                }
            }
        }

        let epg = Epg::from_url(&std::env::var("EPG_PATH").unwrap()).await?;

        let mut cached_epg = self.cached_epg.write().unwrap();
        *cached_epg = Some(EpgFetch {
            epg: epg.clone(),
            fetched: time::Instant::now(),
        });

        Ok(epg)
    }
}

#[tokio::main]
async fn main() {
    dotenvy::from_filename(".env.local").ok();

    let env_filter = EnvFilter::from("info,sparrow_tv=debug,tower_http=debug,axum=debug");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let app_state = AppState::new();
    app_state.fetch_playlist().await.unwrap();
    app_state.fetch_epg().await.unwrap();

    // thread that fetches the playlist if stale
    let app_state_clone = app_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let is_stale = {
                let playlist = app_state_clone.cached_playlist.read().unwrap();
                if let Some(playlist_fetch) = &*playlist {
                    playlist_fetch.is_stale()
                } else {
                    false
                }
            };
            if is_stale {
                tracing::info!("Playlist is stale, fetching new one");
                let _ = app_state_clone.fetch_playlist().await;
            }
        }
    });

    // thread that fetches the epg if stale
    let app_state_clone = app_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let is_stale = {
                let epg = app_state_clone.cached_epg.read().unwrap();
                if let Some(epg_fetch) = &*epg {
                    epg_fetch.is_stale()
                } else {
                    false
                }
            };
            if is_stale {
                tracing::info!("EPG is stale, fetching new one");
                let _ = app_state_clone.fetch_epg().await;
            }
        }
    });

    let serve_dir =
        ServeDir::new("./app/dist").not_found_service(ServeFile::new("./app/dist/index.html"));

    // Start server
    let cors_options = CorsLayer::very_permissive();
    let app: Router = Router::new()
        .route("/", get(routes::download_playlist))
        .route("/epg", get(routes::download_epg))
        .route("/search", get(routes::search))
        .route("/proxy/*stream_path", get(proxy_stream))
        .nest_service("/app", serve_dir.clone())
        .fallback_service(serve_dir)
        .with_state(app_state)
        .layer(cors_options)
        .layer(TraceLayer::new_for_http());

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);
    let socket_addr = format!("{}:{}", host, port)
        .parse::<SocketAddr>()
        .expect("Failed to parse address.");

    tracing::info!("listening on {}", socket_addr);
    let listener = TcpListener::bind(&socket_addr).await.unwrap();
    axum::serve(listener, app).await.unwrap()
}

pub async fn proxy_stream(
    Path(stream_path): Path<String>,
) -> Result<Response<Body>, (axum::http::StatusCode, String)> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create client: {}", e),
        ))?;

    let response = client.get(&stream_path).send().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Failed to fetch stream: {}", e),
        )
    })?;

    let status = response.status();
    let headers = response.headers().clone();

    // Convert the response body into a stream
    let stream = response
        .bytes_stream()
        .map(|result| result.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)));

    // Build the response with streaming body
    let mut builder = Response::builder().status(status);

    // Copy relevant headers
    for (name, value) in headers.iter() {
        if name != "transfer-encoding" {
            builder = builder.header(name, value);
        }
    }

    // Add CORS headers
    builder = builder
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, OPTIONS");

    let response = builder.body(Body::from_stream(stream)).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build response: {}", e),
        )
    })?;

    Ok(response)
}

pub const SNIPPETS_TO_EXCLUDE: &[&str] = &["PL", "FI"];

pub const GROUPS_TO_EXCLUDE: &[&str] = &[
    "For Adults",
    "Afganistan",
    "Pakistan",
    "Turkey",
    "India",
    "Romania",
    "Colombia",
    "Finland",
    "Bulgarien",
    "Iceland",
    "Arabic",
    "Albania",
    "Peru",
    "Chile",
    "Česká republika",
    "Ecuador",
    "France",
    "Latino",
    "Africa",
    "Germany",
    "Russia",
    "Spain",
    "Portugal",
    "Netherlands",
    "Belgium",
    "Thailand",
    "Slovenia",
    "Israel",
    "Iran",
    "Brazil",
    "Argentina",
    "Philippines",
    "Makedonien",
    "EX-Yu",
    "Poland",
    "Austria",
    "Paraguay",
    "Hungary",
    "Slovakien",
    "Mexico",
    "Dominican Republic",
    "Germany PPV Channels",
    "Greece",
    "Kurdistan",
    "Premiership Rugby UK",
    "Switzerland",
    "Venenzuela",
    "Uraguay",
    "Discovery+ Sport FI",
    "Italy",
    "Venezuela",
    "Music Collection",
    "SIMINN PPV (iceland)",
];
