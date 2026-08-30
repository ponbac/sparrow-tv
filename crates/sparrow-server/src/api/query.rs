use axum::extract::{Query, rejection::QueryRejection};
use serde::Deserialize;
use sparrow_core::{ChannelGroupFilter, ChannelQuery, PageCursor, PageLimit, PageRequest};

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
    pub(crate) fn extract(query: Result<Query<Self>, QueryRejection>) -> Result<Self, ApiError> {
        query
            .map(|Query(query)| query)
            .map_err(|_| ApiError::invalid("query", "invalid-format"))
    }

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
    pub(crate) fn extract(query: Result<Query<Self>, QueryRejection>) -> Result<Self, ApiError> {
        query
            .map(|Query(query)| query)
            .map_err(|_| ApiError::invalid("query", "invalid-format"))
    }

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
