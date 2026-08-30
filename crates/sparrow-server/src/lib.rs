//! Hosted, same-origin HTTP composition for Sparrow.

mod api;
mod auth;
mod config;
mod memory_snapshot_store;
mod static_app;

use std::{path::PathBuf, sync::Arc};

use axum::{Router, middleware, response::Redirect, routing::get};
use serde::Serialize;
use sparrow_core::{CoreAdapters, SparrowCore, SystemClock};
use sparrow_source_http::{HttpPlaybackAccess, HttpSourceAccess};
use thiserror::Error;

use crate::{
    api::AppState, auth::DeploymentCredential, config::HostedConfig,
    memory_snapshot_store::MemorySnapshotStore,
};

const BIND_ADDRESS: &str = "0.0.0.0:33733";

/// Builds the complete hosted router around one shared catalog core.
///
/// `/health` and the root redirect are public. Authentication is applied once
/// around both same-origin interfaces: `/app` and `/api/v1`.
pub fn router(
    core: Arc<SparrowCore>,
    password: impl AsRef<[u8]>,
    app_root: impl Into<PathBuf>,
) -> Result<Router, RouterBuildError> {
    let credential = DeploymentCredential::new(password.as_ref())?;
    let playback = HttpPlaybackAccess::new().map_err(|_| RouterBuildError::PlaybackAdapter)?;
    Ok(authenticated_router(
        core,
        playback,
        credential,
        app_root.into(),
    ))
}

fn authenticated_router(
    core: Arc<SparrowCore>,
    playback: HttpPlaybackAccess,
    credential: DeploymentCredential,
    app_root: PathBuf,
) -> Router {
    let protected = Router::new()
        .nest("/api/v1", api::router())
        .nest_service("/app", static_app::service(app_root))
        .layer(middleware::from_fn_with_state(
            credential,
            auth::require_authentication,
        ))
        .with_state(AppState::new(core, playback));

    Router::new()
        .route("/health", get(health))
        .route("/", get(|| async { Redirect::permanent("/app/") }))
        .merge(protected)
}

/// Loads deployment configuration, bootstraps the production adapters, and
/// serves the hosted composition on `0.0.0.0:33733`.
pub async fn run() -> Result<(), StartupError> {
    let config = HostedConfig::load()?;
    let credential = DeploymentCredential::new(config.password.expose())
        .map_err(|_| StartupError::Configuration)?;
    let HostedConfig {
        password,
        source: configuration,
        app_root,
    } = config;
    drop(password);
    let source = Arc::new(HttpSourceAccess::new().map_err(|_| StartupError::SourceAdapter)?);
    let playback = HttpPlaybackAccess::new().map_err(|_| StartupError::PlaybackAdapter)?;
    let snapshots = Arc::new(MemorySnapshotStore::default());
    let adapters = CoreAdapters::new(source, snapshots, Arc::new(SystemClock));
    let core = Arc::new(
        SparrowCore::bootstrap(Some(configuration), adapters)
            .await
            .map_err(|_| StartupError::Core)?,
    );
    let app = authenticated_router(core, playback, credential, app_root);
    let listener = tokio::net::TcpListener::bind(BIND_ADDRESS)
        .await
        .map_err(|_| StartupError::Bind)?;
    axum::serve(listener, app)
        .await
        .map_err(|_| StartupError::Serve)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> axum::Json<Health> {
    axum::Json(Health { status: "ok" })
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RouterBuildError {
    #[error("the deployment password is required")]
    MissingPassword,
    #[error("the deployment password exceeds the supported size")]
    PasswordTooLong,
    #[error("the hosted playback adapter could not be initialized")]
    PlaybackAdapter,
}

/// Startup errors deliberately discard environment, provider, filesystem, and
/// socket diagnostics because those values may contain deployment secrets.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StartupError {
    #[error("deployment configuration is unavailable")]
    Configuration,
    #[error("the source adapter could not be initialized")]
    SourceAdapter,
    #[error("the playback adapter could not be initialized")]
    PlaybackAdapter,
    #[error("the catalog core could not be initialized")]
    Core,
    #[error("the hosted listener could not bind")]
    Bind,
    #[error("the hosted listener stopped unexpectedly")]
    Serve,
}

#[cfg(test)]
mod tests;
