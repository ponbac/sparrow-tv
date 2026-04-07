use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use http::{HeaderName, HeaderValue};
use reqwest::Client;
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::{fs, net::TcpListener, sync::Mutex};
use tower::ServiceBuilder;

use axum::{body::Body, extract::Path, http::Response, routing::get, Router};
use epg::Epg;
use playlist::Playlist;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

mod epg;
mod playlist;
mod routes;

const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REFRESH_RETRY_BACKOFF: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const APP_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36";

trait FileFetch {
    fn is_stale(&self) -> bool;
}

#[derive(Debug)]
struct PlaylistFetch {
    playlist: Playlist,
    fetched: Instant,
}

#[derive(Debug)]
struct EpgFetch {
    epg: Epg,
    fetched: Instant,
}

impl FileFetch for PlaylistFetch {
    fn is_stale(&self) -> bool {
        self.fetched.elapsed() > CACHE_TTL
    }
}

impl FileFetch for EpgFetch {
    fn is_stale(&self) -> bool {
        self.fetched.elapsed() > CACHE_TTL
    }
}

#[derive(Debug, Clone)]
struct AppState {
    pub cached_playlist: Arc<RwLock<Option<PlaylistFetch>>>,
    pub cached_epg: Arc<RwLock<Option<EpgFetch>>>,
    playlist_last_attempt: Arc<RwLock<Option<Instant>>>,
    epg_last_attempt: Arc<RwLock<Option<Instant>>>,
    playlist_refresh_lock: Arc<Mutex<()>>,
    epg_refresh_lock: Arc<Mutex<()>>,
    client: Client,
}

impl AppState {
    fn new() -> Self {
        let client = Client::builder()
            .user_agent(APP_USER_AGENT)
            .timeout(FETCH_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("failed to build HTTP client");

        #[cfg(debug_assertions)]
        let (cached_playlist, cached_epg) = {
            let playlist_file = std::fs::read_to_string("./examples/playlist.m3u").unwrap();
            let playlist: Playlist = playlist_file.parse().unwrap();
            let epg_file = std::fs::File::open("./examples/epg.xml").unwrap();
            let epg = Epg::from_reader(epg_file).unwrap();

            let now = Instant::now();
            let in_a_year = now + Duration::from_secs(365 * 24 * 60 * 60);
            (
                Arc::new(RwLock::new(Some(PlaylistFetch {
                    playlist,
                    fetched: in_a_year,
                }))),
                Arc::new(RwLock::new(Some(EpgFetch {
                    epg,
                    fetched: in_a_year,
                }))),
            )
        };

        #[cfg(not(debug_assertions))]
        let (cached_playlist, cached_epg) =
            (Arc::new(RwLock::new(None)), Arc::new(RwLock::new(None)));

        Self {
            cached_playlist,
            cached_epg,
            playlist_last_attempt: Arc::new(RwLock::new(None)),
            epg_last_attempt: Arc::new(RwLock::new(None)),
            playlist_refresh_lock: Arc::new(Mutex::new(())),
            epg_refresh_lock: Arc::new(Mutex::new(())),
            client,
        }
    }

    fn cached_playlist_snapshot(&self) -> Option<Playlist> {
        self.cached_playlist
            .read()
            .unwrap()
            .as_ref()
            .map(|fetch| fetch.playlist.clone())
    }

    fn cached_epg_snapshot(&self) -> Option<Epg> {
        self.cached_epg
            .read()
            .unwrap()
            .as_ref()
            .map(|fetch| fetch.epg.clone())
    }

    fn fresh_playlist(&self) -> Option<Playlist> {
        self.cached_playlist
            .read()
            .unwrap()
            .as_ref()
            .and_then(|fetch| {
                if fetch.is_stale() {
                    None
                } else {
                    Some(fetch.playlist.clone())
                }
            })
    }

    fn fresh_epg(&self) -> Option<Epg> {
        self.cached_epg.read().unwrap().as_ref().and_then(|fetch| {
            if fetch.is_stale() {
                None
            } else {
                Some(fetch.epg.clone())
            }
        })
    }

    fn playlist_needs_refresh(&self) -> bool {
        match self.cached_playlist.read().unwrap().as_ref() {
            Some(fetch) => fetch.is_stale(),
            None => true,
        }
    }

    fn epg_needs_refresh(&self) -> bool {
        match self.cached_epg.read().unwrap().as_ref() {
            Some(fetch) => fetch.is_stale(),
            None => true,
        }
    }

    fn playlist_attempt_in_backoff(&self) -> bool {
        Self::is_attempt_in_backoff(*self.playlist_last_attempt.read().unwrap())
    }

    fn epg_attempt_in_backoff(&self) -> bool {
        Self::is_attempt_in_backoff(*self.epg_last_attempt.read().unwrap())
    }

    fn is_attempt_in_backoff(last_attempt: Option<Instant>) -> bool {
        last_attempt.is_some_and(|attempt| attempt.elapsed() < REFRESH_RETRY_BACKOFF)
    }

    fn mark_playlist_attempt(&self) {
        *self.playlist_last_attempt.write().unwrap() = Some(Instant::now());
    }

    fn mark_epg_attempt(&self) {
        *self.epg_last_attempt.write().unwrap() = Some(Instant::now());
    }

    async fn fetch_playlist(&self) -> Result<Playlist> {
        if let Some(playlist) = self.fresh_playlist() {
            return Ok(playlist);
        }

        if self.playlist_attempt_in_backoff() {
            if let Some(playlist) = self.cached_playlist_snapshot() {
                return Ok(playlist);
            }
            return Err(anyhow!(
                "playlist refresh is backing off after a recent failed attempt"
            ));
        }

        let _guard = self.playlist_refresh_lock.lock().await;

        if let Some(playlist) = self.fresh_playlist() {
            return Ok(playlist);
        }

        if self.playlist_attempt_in_backoff() {
            if let Some(playlist) = self.cached_playlist_snapshot() {
                return Ok(playlist);
            }
            return Err(anyhow!(
                "playlist refresh is backing off after a recent failed attempt"
            ));
        }

        self.mark_playlist_attempt();
        match self.fetch_playlist_uncached().await {
            Ok(playlist) => {
                let mut cached_playlist = self.cached_playlist.write().unwrap();
                *cached_playlist = Some(PlaylistFetch {
                    playlist: playlist.clone(),
                    fetched: Instant::now(),
                });
                Ok(playlist)
            }
            Err(error) => {
                if let Some(playlist) = self.cached_playlist_snapshot() {
                    tracing::warn!(
                        error = ?error,
                        "Failed to refresh playlist, serving stale cached playlist"
                    );
                    Ok(playlist)
                } else {
                    Err(error)
                }
            }
        }
    }

    async fn fetch_playlist_uncached(&self) -> Result<Playlist> {
        let playlist_content = self.read_source_text("M3U_PATH").await?;
        let playlist_content = strip_utf8_bom(&playlist_content);
        if !playlist_content.trim_start().starts_with("#EXTM3U") {
            return Err(anyhow!(
                "M3U_PATH did not return an M3U playlist: {}",
                body_snippet(playlist_content)
            ));
        }

        let mut playlist: Playlist = playlist_content
            .parse()
            .context("failed to parse playlist response")?;
        playlist.exclude_groups(GROUPS_TO_EXCLUDE.to_vec());
        playlist.exclude_containing(SNIPPETS_TO_EXCLUDE.to_vec());
        playlist.exclude_all_extensions();
        tracing::info!(
            "Fetched playlist with {} groups:\n{}",
            playlist.filtered_groups().len(),
            playlist.filtered_groups().join("\n")
        );
        Ok(playlist)
    }

    async fn fetch_epg(&self) -> Result<Epg> {
        if let Some(epg) = self.fresh_epg() {
            return Ok(epg);
        }

        if self.epg_attempt_in_backoff() {
            if let Some(epg) = self.cached_epg_snapshot() {
                return Ok(epg);
            }
            return Err(anyhow!(
                "EPG refresh is backing off after a recent failed attempt"
            ));
        }

        let _guard = self.epg_refresh_lock.lock().await;

        if let Some(epg) = self.fresh_epg() {
            return Ok(epg);
        }

        if self.epg_attempt_in_backoff() {
            if let Some(epg) = self.cached_epg_snapshot() {
                return Ok(epg);
            }
            return Err(anyhow!(
                "EPG refresh is backing off after a recent failed attempt"
            ));
        }

        self.mark_epg_attempt();
        match self.fetch_epg_uncached().await {
            Ok(epg) => {
                let mut cached_epg = self.cached_epg.write().unwrap();
                *cached_epg = Some(EpgFetch {
                    epg: epg.clone(),
                    fetched: Instant::now(),
                });
                Ok(epg)
            }
            Err(error) => {
                if let Some(epg) = self.cached_epg_snapshot() {
                    tracing::warn!(error = ?error, "Failed to refresh EPG, serving stale cached EPG");
                    Ok(epg)
                } else {
                    Err(error)
                }
            }
        }
    }

    async fn fetch_epg_uncached(&self) -> Result<Epg> {
        let epg_content = self.read_source_text("EPG_PATH").await?;
        Epg::from_reader(epg_content.as_bytes())
            .map_err(|error| anyhow!("failed to parse EPG response: {error}"))
    }

    async fn read_source_text(&self, env_name: &str) -> Result<String> {
        let source = std::env::var(env_name).with_context(|| format!("{env_name} is not set"))?;
        if source.starts_with("http://") || source.starts_with("https://") {
            let response = self
                .client
                .get(&source)
                .send()
                .await
                .with_context(|| format!("failed to fetch {env_name} from {source}"))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .with_context(|| format!("failed to read {env_name} response body"))?;
            if !status.is_success() {
                return Err(anyhow!(
                    "{env_name} returned {status}: {}",
                    body_snippet(&body)
                ));
            }
            return Ok(body);
        }

        let file_path = source.strip_prefix("file://").unwrap_or(&source);
        fs::read_to_string(file_path)
            .await
            .with_context(|| format!("failed to read {env_name} from {file_path}"))
    }
}

#[tokio::main]
async fn main() {
    dotenvy::from_filename(".env.local").ok();

    let env_filter = EnvFilter::from("info,sparrow_tv=debug,tower_http=debug,axum=debug");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let app_state = AppState::new();
    if let Err(error) = app_state.fetch_playlist().await {
        tracing::warn!(error = ?error, "Initial playlist warmup failed");
    }
    if let Err(error) = app_state.fetch_epg().await {
        tracing::warn!(error = ?error, "Initial EPG warmup failed");
    }

    let app_state_clone = app_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if app_state_clone.playlist_needs_refresh()
                && !app_state_clone.playlist_attempt_in_backoff()
            {
                tracing::info!("Playlist is stale, fetching a refresh");
                if let Err(error) = app_state_clone.fetch_playlist().await {
                    tracing::warn!(error = ?error, "Playlist refresh failed");
                }
            }
        }
    });

    let app_state_clone = app_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if app_state_clone.epg_needs_refresh() && !app_state_clone.epg_attempt_in_backoff() {
                tracing::info!("EPG is stale, fetching a refresh");
                if let Err(error) = app_state_clone.fetch_epg().await {
                    tracing::warn!(error = ?error, "EPG refresh failed");
                }
            }
        }
    });

    let serve_index = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_lowercase(b"cache-control").expect("Invalid header name"),
            HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
        ))
        .service(ServeFile::new("./app/dist/index.html"));
    let serve_dir = ServeDir::new("./app/dist").not_found_service(serve_index);

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

async fn proxy_stream(
    Path(stream_path): Path<String>,
    axum::extract::State(app_state): axum::extract::State<AppState>,
) -> Result<Response<Body>, (axum::http::StatusCode, String)> {
    let response = app_state
        .client
        .get(&stream_path)
        .send()
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("Failed to fetch stream: {}", e),
            )
        })?;

    let status = response.status();
    let headers = response.headers().clone();
    let stream = response
        .bytes_stream()
        .map(|result| result.map_err(io::Error::other));

    let mut builder = Response::builder().status(status);
    for (name, value) in headers.iter() {
        if name != "transfer-encoding" {
            builder = builder.header(name, value);
        }
    }

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

fn strip_utf8_bom(input: &str) -> &str {
    input.strip_prefix('\u{feff}').unwrap_or(input)
}

fn body_snippet(input: &str) -> String {
    let collapsed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let snippet: String = collapsed.chars().take(160).collect();
    if collapsed.chars().count() > 160 {
        format!("{snippet}...")
    } else {
        snippet
    }
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
