use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::{Response, StatusCode},
    response::IntoResponse,
    Json,
};
use axum_streams::StreamBodyAs;
use chrono::{DateTime, FixedOffset};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use tokio::stream;
use tokio_stream::Stream;
use tokio_stream::{iter, StreamExt};

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

pub async fn download_epg(
    Query(DownloadQuery { pw }): Query<DownloadQuery>,
    State(app_state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // for (key, value) in headers.iter() {
    //     tracing::info!("Header: {}: {:?}", key, value);
    // }

    if pw != std::env::var("PASSWORD").unwrap() {
        // return Ok(Err((StatusCode::UNAUTHORIZED, "Unauthorized")));
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

    // let start = time::Instant::now();
    // let channels_to_keep: Vec<String> = playlist.entries.iter().map(|e| e.tvg_id.clone()).collect();
    // epg.filter_channels(&channels_to_keep);
    // tracing::info!("Filtered channels in {:?}", start.elapsed());

    tracing::info!("Found {} channels", epg.channels.len());
    tracing::info!("Found {} programmes", epg.programmes.len());

    let xml = epg.to_xml().unwrap();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/xml")
        .body(xml)
        .unwrap())
    // let start = time::Instant::now();
    // let xml = epg.to_xml().unwrap();
    // tracing::info!("Generated XML in {:?}", start.elapsed());
    // Ok(StreamBodyAs::text(stream_xml(xml)).into_response())
}

fn stream_xml(xml: String) -> impl Stream<Item = String> {
    let chunked_xml = xml
        .chars()
        .collect::<Vec<_>>()
        .chunks(64)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>();
    iter(chunked_xml)
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(rename = "q")]
    search_query: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeResult {
    channel_name: String,
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
            channel_name: format!("{} ({})", e.name, e.group_title),
        })
        .collect();

    Ok(Json(SearchResult {
        programmes: programme_results,
        channels,
    }))
}
