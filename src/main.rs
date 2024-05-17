use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
};

use axum::{routing::get, Router};
use parse::Playlist;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

mod parse;

#[derive(Debug)]
struct PlaylistFetch {
    playlist: Playlist,
    fetched: time::Instant,
}

#[derive(Debug, Clone)]
struct AppState {
    cached_playlist: Arc<RwLock<Option<PlaylistFetch>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            cached_playlist: Arc::new(RwLock::new(None)),
        }
    }

    async fn fetch_playlist(&self) -> Result<Playlist, reqwest::Error> {
        {
            let cached_playlist = self.cached_playlist.read().unwrap();
            if let Some(playlist_fetch) = &*cached_playlist {
                if playlist_fetch.fetched.elapsed() < time::Duration::hours(12) {
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
        // let playlist_content =
        //     std::fs::read_to_string("playlist_full.m3u").expect("Failed to read playlist file");
        let mut playlist: Playlist = playlist_content.parse().expect("Failed to parse playlist");
        playlist.exclude_groups(GROUPS_TO_EXCLUDE.to_vec());
        playlist.exclude_containing(SNIPPETS_TO_EXCLUDE.to_vec());
        playlist.exclude_all_extensions();

        let mut cached_playlist = self.cached_playlist.write().unwrap();
        *cached_playlist = Some(PlaylistFetch {
            playlist: playlist.clone(),
            fetched: time::Instant::now(),
        });

        Ok(playlist)
    }
}

#[tokio::main]
async fn main() {
    dotenvy::from_filename(".env.local").ok();

    let env_filter = EnvFilter::from("info,sparrow_tv=debug,tower_http=debug,axum=debug");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let app_state = AppState::new();
    app_state.fetch_playlist().await.unwrap();

    // Start server
    let cors_options = CorsLayer::very_permissive();
    let app: Router = Router::new()
        .route("/", get(routes::download_playlist))
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
        debug_handler,
        extract::{Query, State},
        http::{Response, StatusCode},
    };
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    pub struct DownloadQuery {
        pw: String,
    }

    #[debug_handler]
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
