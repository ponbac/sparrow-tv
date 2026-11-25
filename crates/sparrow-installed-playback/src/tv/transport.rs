use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::{
    NativeStreamHandle, PlaybackManager, PlaybackManagerError, PlaybackSessionId, StartedPlayback,
    android::AndroidPlaybackIdentity,
};

use super::TvPlaybackAudio;
pub(super) use crate::playback::NativePlaybackOpen as Open;

pub(super) type Reply<T> = BoxFuture<'static, Result<T, PlaybackManagerError>>;
pub(super) type Opening = Reply<Opened>;

pub(super) struct Opened {
    pub identity: AndroidPlaybackIdentity,
    pub audio: TvPlaybackAudio,
}

impl From<StartedPlayback> for Opened {
    fn from(opened: StartedPlayback) -> Self {
        Self {
            identity: AndroidPlaybackIdentity::new(
                opened.session_id().clone(),
                opened.stream_handle().clone(),
            ),
            audio: TvPlaybackAudio {
                tracks: opened.tracks().to_vec(),
                selection: opened.selection().clone(),
                preference_status: opened.preference_status(),
            },
        }
    }
}

/// Private transport seam for driving the TV state machine without a provider.
pub(super) trait Transport: Send + Sync + 'static {
    fn admit_open(self: Arc<Self>, session: PlaybackSessionId, request: Open) -> Reply<Opening>;
    fn suspend(
        self: Arc<Self>,
        session: PlaybackSessionId,
        exact: Option<NativeStreamHandle>,
    ) -> Reply<()>;
    fn stop(
        self: Arc<Self>,
        session: PlaybackSessionId,
        last_handle: Option<NativeStreamHandle>,
    ) -> Reply<()>;
    fn validate(
        self: Arc<Self>,
        session: PlaybackSessionId,
        handle: NativeStreamHandle,
    ) -> Reply<()>;
    fn lifecycle(self: Arc<Self>, visible: bool) -> Reply<()>;
    fn wake(self: Arc<Self>, session: PlaybackSessionId) -> Reply<()>;
}

impl Transport for PlaybackManager {
    fn admit_open(self: Arc<Self>, session: PlaybackSessionId, request: Open) -> Reply<Opening> {
        Box::pin(async move {
            let response = self.admit_native_open(session, request).await?;
            let opening: Opening = Box::pin(async move { response.await.map(Opened::from) });
            Ok(opening)
        })
    }

    fn suspend(
        self: Arc<Self>,
        session: PlaybackSessionId,
        exact: Option<NativeStreamHandle>,
    ) -> Reply<()> {
        Box::pin(async move {
            match exact {
                Some(handle) => self.suspend_generation(session, handle).await,
                None => PlaybackManager::suspend(&self, session).await,
            }
        })
    }

    fn stop(
        self: Arc<Self>,
        session: PlaybackSessionId,
        last_handle: Option<NativeStreamHandle>,
    ) -> Reply<()> {
        Box::pin(async move { PlaybackManager::stop(&self, session, last_handle).await })
    }

    fn validate(
        self: Arc<Self>,
        session: PlaybackSessionId,
        handle: NativeStreamHandle,
    ) -> Reply<()> {
        Box::pin(async move { self.validate_active_generation(session, handle).await })
    }

    fn lifecycle(self: Arc<Self>, visible: bool) -> Reply<()> {
        Box::pin(async move {
            if visible {
                self.resume_for_lifecycle().await
            } else {
                self.suspend_for_lifecycle().await
            }
        })
    }

    fn wake(self: Arc<Self>, session: PlaybackSessionId) -> Reply<()> {
        Box::pin(async move { self.set_activity(session, true).await })
    }
}
