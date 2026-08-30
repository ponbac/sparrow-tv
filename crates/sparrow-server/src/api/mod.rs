mod bounded_blocking;
mod browse;
mod dto;
mod error;
mod programmes;
mod query;

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use sparrow_core::{
    ChannelSummary, Page, PageRequest, ProgrammeSummary, SearchRequest, SearchResults, SearchTerm,
    SparrowCore,
};

use self::bounded_blocking::BoundedBlocking;

pub(crate) use dto::{CapabilitiesDto, CatalogStatusDto};
pub(crate) use error::{ApiError, ErrorEnvelope};

#[derive(Clone)]
pub(crate) struct AppState {
    core: Arc<SparrowCore>,
    searches: BoundedBlocking,
}

impl AppState {
    pub(crate) fn new(core: Arc<SparrowCore>) -> Self {
        Self {
            core,
            searches: BoundedBlocking::serial(),
        }
    }

    pub(crate) fn core(&self) -> &SparrowCore {
        &self.core
    }

    pub(crate) async fn search(&self, request: SearchRequest) -> Result<SearchResults, ApiError> {
        let core = Arc::clone(&self.core);
        self.searches
            .run(move |cancellation| {
                core.search_with_cancellation(request, || cancellation.is_cancelled())
            })
            .await
            .map_err(|_| ApiError::service_unavailable())?
            .map_err(ApiError::from)
    }

    pub(crate) async fn search_channels(
        &self,
        term: SearchTerm,
        page: PageRequest,
    ) -> Result<Page<ChannelSummary>, ApiError> {
        let core = Arc::clone(&self.core);
        self.searches
            .run(move |cancellation| {
                core.search_channels_with_cancellation(term, page, || cancellation.is_cancelled())
            })
            .await
            .map_err(|_| ApiError::service_unavailable())?
            .map_err(ApiError::from)
    }

    pub(crate) async fn search_programmes(
        &self,
        term: SearchTerm,
        page: PageRequest,
    ) -> Result<Page<ProgrammeSummary>, ApiError> {
        let core = Arc::clone(&self.core);
        self.searches
            .run(move |cancellation| {
                core.search_programmes_with_cancellation(term, page, || cancellation.is_cancelled())
            })
            .await
            .map_err(|_| ApiError::service_unavailable())?
            .map_err(ApiError::from)
    }
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/capabilities", get(capabilities))
        .route("/status", get(status))
        .route("/groups", get(browse::groups))
        .route("/channels", get(browse::channels))
        .route("/channels/{channel_id}", get(browse::channel))
        .route("/channels/{channel_id}/schedule", get(programmes::schedule))
        .route("/search", get(programmes::search))
        .route("/search/channels", get(programmes::search_channels))
        .route("/search/programmes", get(programmes::search_programmes))
        .fallback(api_not_found)
}

async fn capabilities() -> Json<CapabilitiesDto> {
    Json(CapabilitiesDto::hosted())
}

async fn status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<CatalogStatusDto> {
    Json(CatalogStatusDto::from(state.core().status()))
}

async fn api_not_found() -> ApiError {
    ApiError::invalid("route", "invalid-format")
}
