use axum::{
    Json,
    extract::{Path, Query, State, rejection::PathRejection, rejection::QueryRejection},
};

use super::{
    ApiError, AppState,
    dto::{ChannelSummaryDto, GuideWindowChannelDto, PageDto, ProgrammeDto, SearchResultsDto},
    query::{
        GuideWindowHttpQuery, PageQuery, SearchPageQuery, SearchQuery, extract, schedule_query,
    },
};

pub(crate) async fn guide_window(
    State(state): State<AppState>,
    query: Result<Query<GuideWindowHttpQuery>, QueryRejection>,
) -> Result<Json<PageDto<GuideWindowChannelDto>>, ApiError> {
    let query = extract(query)?.into_core()?;
    let page = state.core().guide_window(query).map_err(ApiError::from)?;
    Ok(Json(PageDto::guide_window(&page)))
}

pub(crate) async fn schedule(
    State(state): State<AppState>,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<PageDto<ProgrammeDto>>, ApiError> {
    let query = schedule_query(path, query)?;
    let page = state.core().schedule(query).map_err(ApiError::from)?;
    Ok(Json(PageDto::programmes(&page)))
}

pub(crate) async fn search(
    State(state): State<AppState>,
    query: Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Json<SearchResultsDto>, ApiError> {
    let request = extract(query)?.into_core()?;
    let results = state.search(request).await?;
    Ok(Json(SearchResultsDto::from(&results)))
}

pub(crate) async fn search_channels(
    State(state): State<AppState>,
    query: Result<Query<SearchPageQuery>, QueryRejection>,
) -> Result<Json<PageDto<ChannelSummaryDto>>, ApiError> {
    let (term, page) = extract(query)?.into_core()?;
    let page = state.search_channels(term, page).await?;
    Ok(Json(PageDto::channels(&page)))
}

pub(crate) async fn search_programmes(
    State(state): State<AppState>,
    query: Result<Query<SearchPageQuery>, QueryRejection>,
) -> Result<Json<PageDto<ProgrammeDto>>, ApiError> {
    let (term, page) = extract(query)?.into_core()?;
    let page = state.search_programmes(term, page).await?;
    Ok(Json(PageDto::programmes(&page)))
}
