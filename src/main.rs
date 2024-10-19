use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{routing::get, Router};
use epg::Epg;
use playlist::Playlist;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

mod epg;
mod playlist;

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
            playlist.groups().len(),
            playlist.groups().join("\n")
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

    // Start server
    let cors_options = CorsLayer::very_permissive();
    let app: Router = Router::new()
        .route("/", get(routes::download_playlist))
        .route("/search", get(routes::search))
        .with_state(app_state)
        .layer(cors_options)
        .layer(TraceLayer::new_for_http());

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
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

mod routes {
    use axum::{
        extract::{Query, State},
        http::{Response, StatusCode},
        Json,
    };
    use chrono::{DateTime, FixedOffset};
    use serde::{Deserialize, Serialize};

    use crate::epg;

    #[derive(Debug, Deserialize)]
    pub struct DownloadQuery {
        pw: String,
    }

    pub async fn download_playlist(
        Query(DownloadQuery { pw }): Query<DownloadQuery>,
        State(app_state): State<super::AppState>,
    ) -> Result<Response<String>, (StatusCode, &'static str)> {
        if pw != std::env::var("PASSWORD").unwrap() {
            return Err((StatusCode::UNAUTHORIZED, "Unauthorized"));
        }

        let playlist = app_state.fetch_playlist().await.map_err(|e| {
            tracing::error!("Failed to fetch playlist: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch playlist",
            )
        })?;
        let m3u = playlist.to_m3u();

        // return m3u file
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "audio/x-mpegurl")
            .body(m3u)
            .unwrap())
    }

    #[derive(Debug, Deserialize)]
    pub struct SearchQuery {
        search: String,
    }

    #[derive(Debug, Serialize)]
    pub struct ProgrammeResult {
        channel_name: String,
        programme_title: String,
        programme_desc: String,
        start: DateTime<FixedOffset>,
        stop: DateTime<FixedOffset>,
    }

    #[derive(Debug, Serialize)]
    pub struct ChannelResult {
        channel_name: String,
    }

    #[derive(Debug, Serialize)]
    pub struct SearchResult {
        programmes: Vec<ProgrammeResult>,
        channels: Vec<ChannelResult>,
    }

    pub async fn search(
        Query(SearchQuery { search }): Query<SearchQuery>,
        State(app_state): State<super::AppState>,
    ) -> Result<Json<SearchResult>, (StatusCode, &'static str)> {
        let epg = app_state.fetch_epg().await.map_err(|e| {
            tracing::error!("Failed to fetch EPG: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch EPG")
        })?;
        let channel_map = epg.channel_map();
        let programmes = epg.search(&search);

        let programme_results: Vec<ProgrammeResult> = programmes
            .into_iter()
            .map(|p| ProgrammeResult {
                programme_title: p.title,
                programme_desc: p.desc,
                start: p.start,
                stop: p.stop,
                channel_name: channel_map.get(&p.channel).unwrap().display_name.clone(),
            })
            .collect();

        let playlist = app_state.fetch_playlist().await.map_err(|e| {
            tracing::error!("Failed to fetch playlist: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch playlist",
            )
        })?;
        let channels: Vec<ChannelResult> = playlist
            .entries
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&search))
            .map(|e| ChannelResult {
                channel_name: e.name.clone(),
            })
            .collect();

        Ok(Json(SearchResult {
            programmes: programme_results,
            channels,
        }))
    }
}

const SNIPPETS_TO_EXCLUDE: &[&str] = &["NO", "DK", "PL", "FI"];

const GROUPS_TO_EXCLUDE: &[&str] = &[
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
];
