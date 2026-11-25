//! Android TV Playback Session ownership. The host posts Media3 operations and
//! supplies Channel targets, layout bounds and Activity visibility.

mod owner;
mod transport;

use std::{future::Future, io::Read, pin::Pin, sync::Arc};

use sparrow_core::{CatalogAccess, ChannelId, ChannelQuality};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    AudioSelection, AudioTrack, AudioTrackId, PlaybackManager, PreferenceStatus,
    android::{
        AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity,
        AndroidPlaybackStatus, AndroidPlaybackViewport,
    },
};

pub type TvHostId = u64;
pub type TvPresentationFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, AndroidPlaybackError>> + Send + 'static>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TvPresentationKey {
    pub host: TvHostId,
    pub identity: AndroidPlaybackIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TvPresentationStop {
    RetainPicture,
    ClearPicture,
}

/// Host operations post only when polled, finish on acknowledgement and return
/// a safe failure if they cannot confirm what happened. A timed-out start must
/// roll back late binding. Clearing an exact key returns success when the host
/// never bound it, already retired it, or replaced it with a different key.
pub trait TvPresentation: Send + Sync + 'static {
    fn start(
        &self,
        key: TvPresentationKey,
        viewport: AndroidPlaybackViewport,
        controls: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()>;
    fn stop(&self, key: TvPresentationKey, stop: TvPresentationStop) -> TvPresentationFuture<()>;
    fn status(&self, key: TvPresentationKey) -> TvPresentationFuture<AndroidPlaybackStatus>;
    fn set_controls(
        &self,
        key: TvPresentationKey,
        controls: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()>;
    /// Hides the picture when no viewport is available, keeping the exact binding.
    fn set_viewport(
        &self,
        key: TvPresentationKey,
        viewport: Option<AndroidPlaybackViewport>,
    ) -> TvPresentationFuture<()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TvQualityVariant {
    pub channel: ChannelId,
    pub quality: ChannelQuality,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TvTuneTarget {
    pub channel: ChannelId,
    pub number: u32,
    pub quality: Option<ChannelQuality>,
    /// The nearest lower variant of the same Channel Number, if any.
    pub lower: Option<TvQualityVariant>,
}

impl TvTuneTarget {
    pub fn new(channel: ChannelId, number: u32) -> Self {
        Self {
            channel,
            number,
            quality: None,
            lower: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TvPresentationEvent {
    FirstFrame,
    Ended,
    StreamInterrupted,
    PlayerUnavailable,
    AudioFocusLost,
}

/// Lets the UI correlate Audio Track changes to the on-screen stream generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TvTransportToken(u64);

pub enum TvPlaybackCommand {
    HostVisibility {
        host: TvHostId,
        started: bool,
        applied: oneshot::Sender<Result<(), TvPlaybackFailure>>,
    },
    HostStarted(TvHostId),
    HostStopped(TvHostId),
    HostFinished(TvHostId),
    HostDestroyed {
        host: TvHostId,
        finishing: bool,
    },
    SetViewports {
        host: TvHostId,
        docked: Option<AndroidPlaybackViewport>,
        fullscreen: Option<AndroidPlaybackViewport>,
    },
    Tune {
        target: TvTuneTarget,
        repeat: bool,
    },
    Zap {
        target: TvTuneTarget,
        revision: u64,
    },
    Pause,
    Resume,
    Restart,
    Stop,
    SelectAudio {
        token: TvTransportToken,
        track: AudioTrackId,
    },
    SetVolume(f32),
    ToggleMute,
    Fullscreen,
    ExitFullscreen,
    PresentationEvent {
        key: TvPresentationKey,
        event: TvPresentationEvent,
    },
    StopForSourceChange {
        released: oneshot::Sender<Result<(), TvPlaybackFailure>>,
        resume: oneshot::Receiver<()>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TvPlaybackFailure {
    ChannelUnavailable,
    SourceRejected,
    SourceTimeout,
    SourceUnavailable,
    InvalidSource,
    UnsupportedMedia,
    StreamInterrupted,
    Ended,
    PresentationUnavailable,
    PlayerUnavailable,
    CleanupUnconfirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TvPauseCause {
    User,
    Visibility,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TvPlaybackPhase {
    Idle,
    Starting,
    Playing,
    Pausing,
    Paused(TvPauseCause),
    ReplacingAudio,
    Recovering {
        attempt: u8,
        failure: TvPlaybackFailure,
    },
    Stopping,
    Failed {
        failure: TvPlaybackFailure,
        can_restart: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TvPlaybackControls {
    pub volume: f32,
    pub muted: bool,
    pub fullscreen: bool,
}

impl Default for TvPlaybackControls {
    fn default() -> Self {
        Self {
            volume: 1.,
            muted: false,
            fullscreen: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TvPlaybackAudio {
    pub tracks: Vec<AudioTrack>,
    pub selection: AudioSelection,
    pub preference_status: Option<PreferenceStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TvPlaybackSnapshot {
    pub phase: TvPlaybackPhase,
    pub selected: Option<TvTuneTarget>,
    pub pending_zap: Option<TvTuneTarget>,
    /// Advances when the owner cancels a zap sequence, even if no target was queued yet.
    pub zap_revision: u64,
    pub last_played: Option<TvTuneTarget>,
    pub controls: TvPlaybackControls,
    pub audio: Option<TvPlaybackAudio>,
    pub transport_token: Option<TvTransportToken>,
    pub status: Option<AndroidPlaybackStatus>,
    pub visible_host: Option<TvHostId>,
    pub recovery_attempts: u8,
    pub quality_fallback: Option<(ChannelQuality, ChannelQuality)>,
}

impl Default for TvPlaybackSnapshot {
    fn default() -> Self {
        Self {
            phase: TvPlaybackPhase::Idle,
            selected: None,
            pending_zap: None,
            zap_revision: 0,
            last_played: None,
            controls: TvPlaybackControls::default(),
            audio: None,
            transport_token: None,
            status: None,
            visible_host: None,
            recovery_attempts: 0,
            quality_fallback: None,
        }
    }
}

/// Holds new tune and zap intents until the durable Source Configuration save ends.
pub struct TvSourceChange {
    _resume: oneshot::Sender<()>,
}

#[derive(Clone)]
pub struct TvPlaybackRunner {
    commands: mpsc::Sender<TvPlaybackCommand>,
    snapshot: watch::Receiver<TvPlaybackSnapshot>,
}

impl TvPlaybackRunner {
    /// Starts one command and timer owner on the caller's Tokio runtime.
    pub fn spawn(
        catalog: CatalogAccess,
        playback: Arc<PlaybackManager>,
        presentation: Arc<dyn TvPresentation>,
    ) -> Result<Self, TvPlaybackFailure> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| TvPlaybackFailure::PlayerUnavailable)?;
        let mut nonce = [0; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut nonce))
            .map_err(|_| TvPlaybackFailure::PlayerUnavailable)?;
        let (commands, inbox) = mpsc::channel(32);
        let (published, snapshot) = watch::channel(TvPlaybackSnapshot::default());
        runtime.spawn(
            owner::Owner::new(
                Some(catalog),
                playback,
                presentation,
                u128::from_be_bytes(nonce),
            )
            .run(inbox, published),
        );
        Ok(Self { commands, snapshot })
    }

    pub fn commands(&self) -> mpsc::Sender<TvPlaybackCommand> {
        self.commands.clone()
    }

    /// Observes Playback Session changes. Diagnostic-only samples remain
    /// available in the latest value without a notification.
    pub fn subscribe(&self) -> watch::Receiver<TvPlaybackSnapshot> {
        self.snapshot.clone()
    }

    pub async fn host_visibility(
        &self,
        host: TvHostId,
        started: bool,
    ) -> Result<(), TvPlaybackFailure> {
        let (applied, acknowledgement) = oneshot::channel();
        self.commands
            .send(TvPlaybackCommand::HostVisibility {
                host,
                started,
                applied,
            })
            .await
            .map_err(|_| TvPlaybackFailure::CleanupUnconfirmed)?;
        acknowledgement
            .await
            .map_err(|_| TvPlaybackFailure::CleanupUnconfirmed)?
    }

    pub async fn stop_for_source_change(&self) -> Result<TvSourceChange, TvPlaybackFailure> {
        let (released, confirmation) = oneshot::channel();
        let (guard, resume) = oneshot::channel();
        self.commands
            .send(TvPlaybackCommand::StopForSourceChange { released, resume })
            .await
            .map_err(|_| TvPlaybackFailure::CleanupUnconfirmed)?;
        confirmation
            .await
            .map_err(|_| TvPlaybackFailure::CleanupUnconfirmed)??;
        Ok(TvSourceChange { _resume: guard })
    }
}
