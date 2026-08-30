mod browse;
mod dto;
mod error;
mod query;

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use sparrow_core::SparrowCore;

pub(crate) use dto::{CapabilitiesDto, CatalogStatusDto};
pub(crate) use error::{ApiError, ErrorEnvelope};

#[derive(Clone)]
pub(crate) struct AppState {
    core: Arc<SparrowCore>,
}

impl AppState {
    pub(crate) fn new(core: Arc<SparrowCore>) -> Self {
        Self { core }
    }

    pub(crate) fn core(&self) -> &SparrowCore {
        &self.core
    }
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/capabilities", get(capabilities))
        .route("/status", get(status))
        .route("/groups", get(browse::groups))
        .route("/channels", get(browse::channels))
        .route("/channels/{channel_id}", get(browse::channel))
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
