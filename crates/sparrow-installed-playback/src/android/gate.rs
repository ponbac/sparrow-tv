use std::{future::Future, sync::Arc};

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::{PlaybackManager, PlaybackManagerError, PlaybackRestartIntent, StartedPlayback};

use super::{AndroidPlaybackError, AndroidPlaybackIdentity};

/// Serializes changes to the Rust transport and its Android presentation.
///
/// Callbacks are invoked under an async permit. Presentation callbacks must
/// settle only after the host acknowledges their work. Keep the executor alive
/// through presentation shutdown and drain admitted work before reusing the
/// gate on another executor.
#[derive(Default)]
pub struct AndroidPresentationGate {
    operation: Arc<Mutex<()>>,
}

impl AndroidPresentationGate {
    /// Rechecks the exact stream generation before admitting a delayed start.
    pub async fn start_if_active<Start>(
        &self,
        playback: &PlaybackManager,
        identity: AndroidPlaybackIdentity,
        start_presentation: impl FnOnce(AndroidPlaybackIdentity) -> Start + Send + 'static,
    ) -> Result<(), PlaybackManagerError>
    where
        Start: Future<Output = Result<(), AndroidPlaybackError>> + Send + 'static,
    {
        self.validate_then_start(
            identity,
            |identity| {
                playback.validate_active_generation(
                    identity.session_id().clone(),
                    identity.stream_handle().clone(),
                )
            },
            start_presentation,
        )
        .await
    }

    pub(crate) async fn validate_then_start<Validate, Start>(
        &self,
        identity: AndroidPlaybackIdentity,
        validate_transport: impl FnOnce(AndroidPlaybackIdentity) -> Validate,
        start_presentation: impl FnOnce(AndroidPlaybackIdentity) -> Start + Send + 'static,
    ) -> Result<(), PlaybackManagerError>
    where
        Validate: Future<Output = Result<(), PlaybackManagerError>>,
        Start: Future<Output = Result<(), AndroidPlaybackError>> + Send + 'static,
    {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        validate_transport(identity.clone()).await?;
        run_presentation(permit, move || start_presentation(identity)).await
    }

    pub async fn presentation_call<Output, Operation>(
        &self,
        operation: impl FnOnce() -> Operation + Send + 'static,
    ) -> Result<Output, PlaybackManagerError>
    where
        Output: Send + 'static,
        Operation: Future<Output = Result<Output, AndroidPlaybackError>> + Send + 'static,
    {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        run_presentation(permit, operation).await
    }

    pub async fn transport_transition<Output, Transition>(
        &self,
        transition: impl FnOnce() -> Transition,
    ) -> Result<Output, PlaybackManagerError>
    where
        Transition: Future<Output = Result<Output, PlaybackManagerError>>,
    {
        let _permit = Arc::clone(&self.operation).lock_owned().await;
        transition().await
    }

    pub async fn stop_after_transport<Transport, Presentation>(
        &self,
        stop_transport: impl FnOnce() -> Transport + Send + 'static,
        stop_presentation: impl FnOnce() -> Presentation + Send + 'static,
    ) -> Result<(), PlaybackManagerError>
    where
        Transport: Future<Output = Result<(), PlaybackManagerError>> + Send + 'static,
        Presentation: Future<Output = Result<(), AndroidPlaybackError>> + Send + 'static,
    {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        run_cleanup(permit, stop_transport, stop_presentation)
            .await
            .map(drop)
    }

    /// Stops and replaces the same generation after its presentation release settles.
    pub async fn restart_generation<Stop>(
        &self,
        playback: &Arc<PlaybackManager>,
        identity: AndroidPlaybackIdentity,
        intent: PlaybackRestartIntent,
        stop_presentation: impl FnOnce(AndroidPlaybackIdentity) -> Stop + Send + 'static,
    ) -> Result<StartedPlayback, PlaybackManagerError>
    where
        Stop: Future<Output = Result<(), AndroidPlaybackError>> + Send + 'static,
    {
        let transport = Arc::clone(playback);
        let stop_identity = identity.clone();
        let replace_identity = identity.clone();
        self.stop_before_transport_replace(
            move || async move {
                transport
                    .suspend_generation(
                        stop_identity.session_id().clone(),
                        stop_identity.stream_handle().clone(),
                    )
                    .await
            },
            move || stop_presentation(identity),
            || {
                playback.restart(
                    replace_identity.session_id().clone(),
                    replace_identity.stream_handle().clone(),
                    intent,
                )
            },
        )
        .await
    }

    async fn stop_before_transport_replace<Output, Transport, Presentation, Replace>(
        &self,
        stop_transport: impl FnOnce() -> Transport + Send + 'static,
        stop_presentation: impl FnOnce() -> Presentation + Send + 'static,
        replace_transport: impl FnOnce() -> Replace,
    ) -> Result<Output, PlaybackManagerError>
    where
        Transport: Future<Output = Result<(), PlaybackManagerError>> + Send + 'static,
        Presentation: Future<Output = Result<(), AndroidPlaybackError>> + Send + 'static,
        Replace: Future<Output = Result<Output, PlaybackManagerError>>,
    {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        let _permit = run_cleanup(permit, stop_transport, stop_presentation).await?;
        replace_transport().await
    }
}

/// Once a presentation operation starts, a cancelled caller must not release
/// the permit ahead of its acknowledgement.
async fn run_presentation<Output, Operation>(
    permit: OwnedMutexGuard<()>,
    operation: impl FnOnce() -> Operation + Send + 'static,
) -> Result<Output, PlaybackManagerError>
where
    Output: Send + 'static,
    Operation: Future<Output = Result<Output, AndroidPlaybackError>> + Send + 'static,
{
    tokio::spawn(async move {
        let _permit = permit;
        operation()
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)
    })
    .await
    .map_err(|_| PlaybackManagerError::Unavailable)?
}

/// Keeps the whole cleanup alive after the requester has been cancelled.
/// Acquiring the permit before spawning limits detached work to one operation.
async fn run_cleanup<Transport, Presentation>(
    permit: OwnedMutexGuard<()>,
    stop_transport: impl FnOnce() -> Transport + Send + 'static,
    stop_presentation: impl FnOnce() -> Presentation + Send + 'static,
) -> Result<OwnedMutexGuard<()>, PlaybackManagerError>
where
    Transport: Future<Output = Result<(), PlaybackManagerError>> + Send + 'static,
    Presentation: Future<Output = Result<(), AndroidPlaybackError>> + Send + 'static,
{
    tokio::spawn(async move {
        let transport_result = stop_transport().await;
        let presentation_result = stop_presentation().await;

        // Always attempt presentation cleanup. The transport failure wins
        // when both steps return an error.
        transport_result?;
        presentation_result
            .map(|()| permit)
            .map_err(|_| PlaybackManagerError::Unavailable)
    })
    .await
    .map_err(|_| PlaybackManagerError::Unavailable)?
}

#[cfg(test)]
mod tests;
