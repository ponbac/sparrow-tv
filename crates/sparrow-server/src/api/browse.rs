use axum::{
    Json,
    extract::{Path, Query, State, rejection::PathRejection, rejection::QueryRejection},
};

use super::{
    ApiError, AppState,
    dto::{ChannelDetailsDto, ChannelGroupDto, ChannelSummaryDto, PageDto},
    query::{ChannelsQuery, PageQuery, channel_id, extract},
};

pub(crate) async fn groups(
    State(state): State<AppState>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<PageDto<ChannelGroupDto>>, ApiError> {
    let request = extract(query)?.page_request()?;
    let page = state.core().list_groups(request).map_err(ApiError::from)?;
    Ok(Json(PageDto::groups(&page)))
}

pub(crate) async fn channels(
    State(state): State<AppState>,
    query: Result<Query<ChannelsQuery>, QueryRejection>,
) -> Result<Json<PageDto<ChannelSummaryDto>>, ApiError> {
    let query = extract(query)?.into_core()?;
    let page = state.core().list_channels(query).map_err(ApiError::from)?;
    Ok(Json(PageDto::channels(&page)))
}

pub(crate) async fn channel(
    State(state): State<AppState>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<ChannelDetailsDto>, ApiError> {
    let id = channel_id(path)?;
    let channel = state.core().channel(&id).map_err(ApiError::from)?;
    Ok(Json(ChannelDetailsDto::from(&channel)))
}
