use axum::extract::{
    Path, Query,
    rejection::{PathRejection, QueryRejection},
};
use serde::Deserialize;
use sparrow_core::{
    ChannelGroupFilter, ChannelId, ChannelQuery, PageCursor, PageLimit, PageRequest, ScheduleQuery,
    SearchRequest, SearchTerm,
};

use super::ApiError;

const DEFAULT_PAGE_LIMIT: u16 = 50;
const MAX_CURSOR_BYTES: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    cursor: Option<String>,
    limit: Option<String>,
}

impl PageQuery {
    pub(crate) fn page_request(self) -> Result<PageRequest, ApiError> {
        page_request(self.cursor, self.limit)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelsQuery {
    cursor: Option<String>,
    group: Option<String>,
    limit: Option<String>,
}

impl ChannelsQuery {
    pub(crate) fn into_core(self) -> Result<ChannelQuery, ApiError> {
        let page = page_request(self.cursor, self.limit)?;
        match self.group {
            Some(group) => {
                let group = ChannelGroupFilter::parse(group).map_err(ApiError::from)?;
                Ok(ChannelQuery::in_group(group, page))
            }
            None => Ok(ChannelQuery::all(page)),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchQuery {
    term: String,
    channel_limit: String,
    channel_cursor: Option<String>,
    programme_limit: String,
    programme_cursor: Option<String>,
}

impl SearchQuery {
    pub(crate) fn into_core(self) -> Result<SearchRequest, ApiError> {
        let term = SearchTerm::parse(self.term).map_err(ApiError::from)?;
        let channels = page_request(self.channel_cursor, Some(self.channel_limit))?;
        let programmes = page_request(self.programme_cursor, Some(self.programme_limit))?;
        Ok(SearchRequest::new(term, channels, programmes))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchPageQuery {
    term: String,
    limit: String,
    cursor: Option<String>,
}

impl SearchPageQuery {
    pub(crate) fn into_core(self) -> Result<(SearchTerm, PageRequest), ApiError> {
        let term = SearchTerm::parse(self.term).map_err(ApiError::from)?;
        let page = page_request(self.cursor, Some(self.limit))?;
        Ok((term, page))
    }
}

pub(crate) fn schedule_query(
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<ScheduleQuery, ApiError> {
    Ok(ScheduleQuery::new(
        channel_id(path)?,
        extract(query)?.page_request()?,
    ))
}

pub(crate) fn extract<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    query
        .map(|Query(query)| query)
        .map_err(|_| ApiError::invalid("query", "invalid-format"))
}

pub(crate) fn channel_id(path: Result<Path<String>, PathRejection>) -> Result<ChannelId, ApiError> {
    let Path(value) = path.map_err(|_| ApiError::invalid("channel-id", "invalid-format"))?;
    ChannelId::parse(value).map_err(ApiError::from)
}

fn page_request(cursor: Option<String>, limit: Option<String>) -> Result<PageRequest, ApiError> {
    let limit = match limit {
        Some(value) => {
            let parsed = value
                .parse::<u16>()
                .map_err(|_| ApiError::invalid("page-limit", "invalid-format"))?;
            PageLimit::new(parsed).map_err(ApiError::from)?
        }
        None => PageLimit::new(DEFAULT_PAGE_LIMIT).expect("the default page limit is valid"),
    };

    let cursor = cursor
        .map(|value| {
            if value.len() > MAX_CURSOR_BYTES {
                return Err(ApiError::invalid("page-cursor", "too-long"));
            }
            PageCursor::parse(value).map_err(ApiError::from)
        })
        .transpose()?;

    Ok(PageRequest::new(cursor, limit))
}
