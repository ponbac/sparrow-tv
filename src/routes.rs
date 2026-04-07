use std::collections::{HashMap, HashSet};

use axum::{
    extract::{Query, State},
    http::{Response, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, FixedOffset};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};

use crate::{
    epg::{Channel, Epg, Icon},
    playlist::PlaylistEntry,
    AppState,
};

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
        (StatusCode::SERVICE_UNAVAILABLE, "Failed to fetch playlist")
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

    let playlist = app_state.fetch_playlist().await.map_err(|e| {
        tracing::error!("Failed to fetch playlist: {:?}", e);
        (StatusCode::SERVICE_UNAVAILABLE, "Failed to fetch playlist")
    })?;
    let mut epg = app_state.fetch_epg().await.unwrap_or_else(|error| {
        tracing::warn!(?error, "Failed to fetch EPG, serving playlist-only EPG response");
        epg_from_playlist_entries(&playlist.filtered_entries)
    });

    let channels_to_keep: Vec<String> = playlist
        .filtered_entries
        .par_iter()
        .map(|e| e.tvg_id.clone())
        .collect();
    epg.filter_channels(&channels_to_keep);

    let xml = epg.to_xml().map_err(|e| {
        tracing::error!("Failed to render EPG XML: {:?}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to render EPG XML",
        )
    })?;

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
    channel_url: Option<String>,
    programme_title: String,
    programme_desc: String,
    start: DateTime<FixedOffset>,
    stop: DateTime<FixedOffset>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResult {
    channel_name: String,
    url: String,
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
    let playlist = app_state.fetch_playlist().await.map_err(|e| {
        tracing::error!("Failed to fetch playlist: {:?}", e);
        (StatusCode::SERVICE_UNAVAILABLE, "Failed to fetch playlist")
    })?;
    let epg = app_state.fetch_epg().await.unwrap_or_else(|error| {
        tracing::warn!(?error, "Failed to fetch EPG, serving channels-only search response");
        Epg::empty()
    });
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
                channel_url: channel
                    .and_then(|c| playlist_channels.get(&c.id).map(|pc| pc.url.clone())),
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
            url: e.url.clone(),
        })
        .collect();

    Ok(Json(SearchResult {
        programmes: programme_results,
        channels,
    }))
}

fn epg_from_playlist_entries(entries: &[PlaylistEntry]) -> Epg {
    let mut seen = HashSet::new();
    let channels = entries
        .iter()
        .filter_map(|entry| {
            if entry.tvg_id.is_empty() || !seen.insert(entry.tvg_id.clone()) {
                return None;
            }

            Some(Channel {
                id: entry.tvg_id.clone(),
                display_name: if entry.tvg_name.is_empty() {
                    entry.name.clone()
                } else {
                    entry.tvg_name.clone()
                },
                icon: (!entry.tvg_logo.is_empty()).then(|| Icon {
                    src: entry.tvg_logo.clone(),
                }),
            })
        })
        .collect();

    Epg {
        channels,
        programmes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epg_from_playlist_entries_deduplicates_channels() {
        let entries = vec![
            PlaylistEntry {
                duration: -1,
                tvg_id: "svt1.se".to_string(),
                tvg_name: "SVT1".to_string(),
                tvg_logo: "https://example.com/svt1.png".to_string(),
                group_title: "Sweden".to_string(),
                name: "SVT1 FHD SE".to_string(),
                url: "http://example.com/1".to_string(),
            },
            PlaylistEntry {
                duration: -1,
                tvg_id: "svt1.se".to_string(),
                tvg_name: "SVT1".to_string(),
                tvg_logo: "https://example.com/svt1.png".to_string(),
                group_title: "Sweden".to_string(),
                name: "SVT1 Backup".to_string(),
                url: "http://example.com/2".to_string(),
            },
        ];

        let epg = epg_from_playlist_entries(&entries);
        assert_eq!(epg.channels.len(), 1);
        assert_eq!(epg.channels[0].id, "svt1.se");
        assert_eq!(epg.channels[0].display_name, "SVT1");
        assert!(epg.programmes.is_empty());
    }
}
