//! Shared TV Playback Session ownership. The host supplies presentation
//! operations, Channel targets, layout bounds and visibility.

mod owner;
mod transport;

use std::{future::Future, io::Read, pin::Pin, sync::Arc};

use sparrow_core::{CatalogAccess, ChannelFamily, ChannelId, ChannelQuality, ChannelSummary};
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
    /// Samples the currently bound visible surface. Callers may abandon the
    /// result; the host can finish sampling afterwards and must then drop it.
    fn capture_frame(&self, key: TvPresentationKey) -> TvPresentationFuture<Arc<TvRecoveryFrame>>;
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

    pub fn from_family(family: &ChannelFamily, channel: &ChannelSummary) -> Self {
        let quality = channel.variant().map(|variant| variant.quality());
        let lower = quality.and_then(|quality| {
            family
                .variants()
                .iter()
                .filter_map(|channel| {
                    let candidate = channel.variant()?.quality();
                    (candidate < quality).then(|| TvQualityVariant {
                        channel: channel.id().clone(),
                        quality: candidate,
                    })
                })
                .max_by_key(|candidate| candidate.quality)
        });
        Self {
            number: channel.number(),
            channel: channel.id().clone(),
            quality,
            lower,
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
    StopConfirmed(oneshot::Sender<Result<(), TvPlaybackFailure>>),
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

/// A downsampled BGRA still for recovery. Keep pixels out of diagnostics.
#[derive(PartialEq, Eq)]
pub struct TvRecoveryFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for TvRecoveryFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TvRecoveryFrame")
    }
}

/// Acknowledges one handoff after the still has painted beneath the native picture.
#[derive(Clone, Debug, Default)]
pub struct TvStillHandoff(Arc<tokio::sync::Notify>);

impl TvStillHandoff {
    pub fn painted(&self) {
        self.0.notify_one();
    }
}

impl PartialEq for TvStillHandoff {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TvPlaybackSnapshot {
    /// First visible frame of the selection, retained across transport rebinds.
    pub first_picture: Option<std::time::Instant>,
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
    pub still_handoff: Option<TvStillHandoff>,
    pub recovery_frame: Option<Arc<TvRecoveryFrame>>,
    pub recovery_attempts: u8,
    pub quality_fallback: Option<(ChannelQuality, ChannelQuality)>,
}

impl Default for TvPlaybackSnapshot {
    fn default() -> Self {
        Self {
            phase: TvPlaybackPhase::Idle,
            first_picture: None,
            selected: None,
            pending_zap: None,
            zap_revision: 0,
            last_played: None,
            controls: TvPlaybackControls::default(),
            audio: None,
            transport_token: None,
            status: None,
            visible_host: None,
            recovery_frame: None,
            still_handoff: None,
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
    pending_viewports: Arc<owner::PendingViewports>,
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
        let owner = owner::Owner::new(
            Some(catalog),
            playback,
            presentation,
            u128::from_be_bytes(nonce),
        );
        let snapshot = owner.published.subscribe();
        let pending_viewports = Arc::clone(&owner.pending_viewports);
        runtime.spawn(owner.run(inbox));
        Ok(Self {
            commands,
            snapshot,
            pending_viewports,
        })
    }

    /// Frame samples replace the pending bounds for their host while the owner is busy.
    pub fn set_viewports(
        &self,
        host: TvHostId,
        docked: Option<AndroidPlaybackViewport>,
        fullscreen: Option<AndroidPlaybackViewport>,
    ) {
        self.pending_viewports.set(host, docked, fullscreen);
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

    /// Stops the current session and confirms its transport and picture were released.
    pub async fn stop(&self) -> Result<(), TvPlaybackFailure> {
        let (stopped, confirmation) = oneshot::channel();
        self.commands
            .send(TvPlaybackCommand::StopConfirmed(stopped))
            .await
            .map_err(|_| TvPlaybackFailure::CleanupUnconfirmed)?;
        confirmation
            .await
            .map_err(|_| TvPlaybackFailure::CleanupUnconfirmed)?
    }
}
