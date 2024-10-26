use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    http::{Response, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, FixedOffset};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};

use crate::{playlist::PlaylistEntry, AppState};

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

pub async fn download_epg(
    Query(DownloadQuery { pw }): Query<DownloadQuery>,
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    if pw != std::env::var("PASSWORD").unwrap() {
        return Err((StatusCode::UNAUTHORIZED, "Unauthorized"));
    }

    let mut epg = app_state.fetch_epg().await.map_err(|e| {
        tracing::error!("Failed to fetch EPG: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch EPG")
    })?;
    let playlist = app_state.fetch_playlist().await.map_err(|e| {
        tracing::error!("Failed to fetch playlist: {:?}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to fetch playlist",
        )
    })?;

    let channels_to_keep: Vec<String> = playlist
        .filtered_entries
        .par_iter()
        .map(|e| e.tvg_id.clone())
        .collect();
    epg.filter_channels(&channels_to_keep);

    let xml = epg.to_xml().unwrap();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/xml")
        .body(xml)
        .unwrap())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(rename = "q")]
    search_query: String,
    include_hidden: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeResult {
    channel_name: String,
    channel_group: Option<String>,
    programme_title: String,
    programme_desc: String,
    start: DateTime<FixedOffset>,
    stop: DateTime<FixedOffset>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResult {
    channel_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    programmes: Vec<ProgrammeResult>,
    channels: Vec<ChannelResult>,
}

pub async fn search(
    Query(SearchQuery {
        search_query,
        include_hidden,
    }): Query<SearchQuery>,
    State(app_state): State<AppState>,
) -> Result<Json<SearchResult>, (StatusCode, &'static str)> {
    let epg = app_state.fetch_epg().await.map_err(|e| {
        tracing::error!("Failed to fetch EPG: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch EPG")
    })?;
    let playlist = app_state.fetch_playlist().await.map_err(|e| {
        tracing::error!("Failed to fetch playlist: {:?}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to fetch playlist",
        )
    })?;
    let playlist_entries = if let Some(true) = include_hidden {
        playlist.entries.clone()
    } else {
        playlist.filtered_entries.clone()
    };
    let playlist_channels: HashMap<String, PlaylistEntry> = playlist_entries
        .clone()
        .into_iter()
        .map(|e| (e.tvg_id.clone(), e))
        .collect();

    let channel_map = epg.channel_map();
    let programmes = epg.search(&search_query);

    let programme_results: Vec<ProgrammeResult> = programmes
        .into_iter()
        .map(|p| {
            let channel = channel_map.get(&p.channel);
            ProgrammeResult {
                programme_title: p.title,
                programme_desc: p.desc,
                start: p.start,
                stop: p.stop,
                channel_name: if let Some(channel) = channel {
                    channel.display_name.clone()
                } else {
                    "Unknown channel".to_string()
                },
                channel_group: channel.and_then(|c| {
                    playlist_channels
                        .get(&c.id)
                        .map(|pc| pc.group_title.clone())
                }),
            }
        })
        .filter(|p| {
            if let Some(true) = include_hidden {
                true
            } else {
                p.channel_group.is_some()
            }
        })
        .collect();

    let lower_search_query = search_query.to_lowercase();
    let channels: Vec<ChannelResult> = playlist_entries
        .par_iter()
        .filter(|e| e.name.to_lowercase().contains(&lower_search_query))
        .map(|e| ChannelResult {
            channel_name: format!("{} ({})", e.name, e.group_title),
        })
        .collect();

    Ok(Json(SearchResult {
        programmes: programme_results,
        channels,
    }))
}
