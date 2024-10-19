use axum::{
    extract::{Query, State},
    http::{Response, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    pw: String,
}

pub async fn download_playlist(
    Query(DownloadQuery { pw }): Query<DownloadQuery>,
    State(app_state): State<AppState>,
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
    #[serde(rename = "q")]
    search_query: String,
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
    Query(SearchQuery { search_query }): Query<SearchQuery>,
    State(app_state): State<AppState>,
) -> Result<Json<SearchResult>, (StatusCode, &'static str)> {
    let epg = app_state.fetch_epg().await.map_err(|e| {
        tracing::error!("Failed to fetch EPG: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch EPG")
    })?;
    let channel_map = epg.channel_map();
    let programmes = epg.search(&search_query);

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
        .filter(|e| e.name.to_lowercase().contains(&search_query.to_lowercase()))
        .map(|e| ChannelResult {
            channel_name: e.name.clone(),
        })
        .collect();

    Ok(Json(SearchResult {
        programmes: programme_results,
        channels,
    }))
}
