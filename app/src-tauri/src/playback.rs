use std::{
    collections::VecDeque,
    fmt::{self, Debug, Formatter},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};

use bytes::Bytes;
use futures_util::{FutureExt as _, StreamExt as _};
use sparrow_core::{
    ChannelId, CoreError, PlaybackActivityLease, ResolvedPlaybackSource, SparrowCore,
};
use sparrow_source_http::{
    HttpPlaybackAccess, PlaybackAccessError, PlaybackByteStream, PlaybackReadError,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

#[path = "playback/mpv.rs"]
mod mpv;

#[cfg(test)]
use crate::screen_wake::noop_screen_wake;
use crate::{
    audio_preferences::{AudioPreferenceStore, PreferenceWrite},
    screen_wake::ScreenWake,
    selected_transport_stream::{
        AudioSelection, AudioTrack, AudioTrackId, PreferenceStatus, SelectedTransportStream,
        SelectionRequest, TransportStreamError,
    },
};
pub(crate) use mpv::MpvFailure;
#[cfg(test)]
use mpv::UnsupportedMpvPlayer;
use mpv::{MpvExit, MpvLaunchFuture, MpvProcess, NativeMpvPlayer, system_mpv_player};
pub(crate) use mpv::{MpvPlaybackControl, MpvVolume};
const COMMAND_CAPACITY: usize = 16;
const READ_CAPACITY: usize = 16;
const MAX_STOP_TOMBSTONES: usize = 64;
const MAX_SUSPEND_INTENTS: usize = 64;
const MAX_NATIVE_PULL_BYTES: usize = 64 * 1024;
const SESSION_ID_PREFIX: &str = "play1_";
const SESSION_ID_NONCE_HEX_BYTES: usize = 32;
const MAX_SESSION_ID_BYTES: usize = 64;
const STREAM_HANDLE_PREFIX: &str = "stream1_";
const STREAM_HANDLE_HEX_BYTES: usize = 16;

type AccessOpenFuture =
    Pin<Box<dyn Future<Output = Result<PlaybackByteStream, PlaybackAccessError>> + Send + 'static>>;
type TransportOpenFuture = Pin<
    Box<
        dyn Future<Output = Result<PreparedPlaybackTransport, TransportStreamError>>
            + Send
            + 'static,
    >,
>;
type SelectedOpenFuture =
    Pin<Box<dyn Future<Output = Result<OpenedPlayback, PendingOpenError>> + Send + 'static>>;
type DescriptorReply = oneshot::Sender<Result<StartedPlayback, PlaybackManagerError>>;
type UnitReply = oneshot::Sender<Result<(), PlaybackManagerError>>;
type MpvReply = oneshot::Sender<Result<MpvPlaybackSession, PlaybackManagerError>>;

trait NativePlaybackAccess: Send + Sync + 'static {
    fn open(&self, source: Arc<ResolvedPlaybackSource>) -> AccessOpenFuture;
}

impl NativePlaybackAccess for HttpPlaybackAccess {
    fn open(&self, source: Arc<ResolvedPlaybackSource>) -> AccessOpenFuture {
        let access = self.clone();
        Box::pin(async move {
            HttpPlaybackAccess::open(&access, source.as_ref())
                .await
                .map(|response| response.into_body())
        })
    }
}

trait PlaybackTransportSelector: Send + Sync + 'static {
    fn open(&self, body: PlaybackByteStream, request: SelectionRequest) -> TransportOpenFuture;
}

struct MpegTsPlaybackTransportSelector;

impl PlaybackTransportSelector for MpegTsPlaybackTransportSelector {
    fn open(&self, body: PlaybackByteStream, request: SelectionRequest) -> TransportOpenFuture {
        Box::pin(async move {
            let opened = SelectedTransportStream::open(body, request).await?;
            Ok(PreparedPlaybackTransport {
                body: Box::pin(opened.stream),
                tracks: opened.tracks,
                selection: opened.selection,
            })
        })
    }
}

#[cfg(test)]
struct PassthroughPlaybackTransportSelector;

#[cfg(test)]
impl PlaybackTransportSelector for PassthroughPlaybackTransportSelector {
    fn open(&self, body: PlaybackByteStream, _request: SelectionRequest) -> TransportOpenFuture {
        Box::pin(async move {
            Ok(PreparedPlaybackTransport {
                body,
                tracks: Vec::new(),
                selection: AudioSelection::None,
            })
        })
    }
}

/// A client-created identifier that correlates reordered playback invokes.
///
/// It is deliberately opaque in diagnostics and cannot be serialized from the
/// native playback layer.
#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct PlaybackSessionId(String);

impl PlaybackSessionId {
    pub(crate) fn parse(value: String) -> Result<Self, PlaybackManagerError> {
        let Some(suffix) = value.strip_prefix(SESSION_ID_PREFIX) else {
            return Err(PlaybackManagerError::Unavailable);
        };
        let Some((nonce, sequence)) = suffix.split_once('_') else {
            return Err(PlaybackManagerError::Unavailable);
        };
        if value.len() > MAX_SESSION_ID_BYTES
            || nonce.len() != SESSION_ID_NONCE_HEX_BYTES
            || sequence.is_empty()
            || !nonce.bytes().all(is_lower_hex)
            || !sequence.bytes().all(is_lower_hex)
        {
            return Err(PlaybackManagerError::Unavailable);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for PlaybackSessionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("PlaybackSessionId(<redacted>)")
    }
}

/// A Rust-generated handle for exactly one native provider stream.
#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct NativeStreamHandle(String);

impl NativeStreamHandle {
    pub(crate) fn parse(value: String) -> Result<Self, PlaybackManagerError> {
        let Some(suffix) = value.strip_prefix(STREAM_HANDLE_PREFIX) else {
            return Err(PlaybackManagerError::Unavailable);
        };
        if suffix.len() != STREAM_HANDLE_HEX_BYTES || !suffix.bytes().all(is_lower_hex) {
            return Err(PlaybackManagerError::Unavailable);
        }
        Ok(Self(value))
    }

    fn from_sequence(sequence: u64) -> Self {
        Self(format!("{STREAM_HANDLE_PREFIX}{sequence:016x}"))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for NativeStreamHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeStreamHandle(<redacted>)")
    }
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct StartedPlayback {
    session_id: PlaybackSessionId,
    stream_handle: NativeStreamHandle,
    tracks: Vec<AudioTrack>,
    selection: AudioSelection,
    preference_status: Option<PreferenceStatus>,
}

impl StartedPlayback {
    pub(crate) fn session_id(&self) -> &PlaybackSessionId {
        &self.session_id
    }

    pub(crate) fn stream_handle(&self) -> &NativeStreamHandle {
        &self.stream_handle
    }

    pub(crate) fn tracks(&self) -> &[AudioTrack] {
        &self.tracks
    }

    pub(crate) fn selection(&self) -> &AudioSelection {
        &self.selection
    }

    pub(crate) const fn preference_status(&self) -> Option<PreferenceStatus> {
        self.preference_status
    }
}

impl Debug for StartedPlayback {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartedPlayback")
            .field("session_id", &self.session_id)
            .field("stream_handle", &self.stream_handle)
            .field("tracks", &self.tracks)
            .field("selection", &self.selection)
            .field("preference_status", &self.preference_status)
            .finish()
    }
}

/// A safe acknowledgement that system mpv owns the selected Playback Session.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct MpvPlaybackSession {
    session_id: PlaybackSessionId,
}

impl MpvPlaybackSession {
    pub(crate) fn session_id(&self) -> &PlaybackSessionId {
        &self.session_id
    }
}

impl Debug for MpvPlaybackSession {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MpvPlaybackSession")
            .field("session_id", &self.session_id)
            .finish()
    }
}

/// Platform-selected Primary Playback Engine opened for an installed session.
pub(crate) enum InstalledPlaybackStart {
    NativeStream(StartedPlayback),
    #[cfg(target_os = "linux")]
    LinuxMpv(MpvPlaybackSession),
}

/// Safe failures from the native playback boundary. Provider locations and
/// library errors never enter this value.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PlaybackManagerError {
    #[error("the catalog could not resolve playback")]
    Core(#[source] CoreError),
    #[error("native playback failed before streaming")]
    Access(PlaybackAccessError),
    #[error("native playback failed while streaming")]
    Read(PlaybackReadError),
    #[error("native transport stream inspection failed")]
    TransportStream(TransportStreamError),
    #[error("system mpv playback failed")]
    Mpv(#[source] MpvFailure),
    #[error("the playback operation was cancelled")]
    Cancelled,
    #[error("native playback is unavailable")]
    Unavailable,
}

impl PlaybackManagerError {
    fn retryable(&self) -> bool {
        match self {
            Self::Access(error) => error.retryable(),
            Self::Read(_) => true,
            Self::TransportStream(error) => error.retryable(),
            Self::Mpv(error) => error.retryable(),
            Self::Core(_) | Self::Cancelled | Self::Unavailable => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackRestartIntent {
    Retry,
    Resume,
    SelectAudio(AudioTrackId),
}

/// Owns the only native provider connection and serializes all stream lifecycle
/// transitions behind bounded, cancellation-aware queues.
pub(crate) struct PlaybackManager {
    controls: mpsc::Sender<ControlCommand>,
    reads: mpsc::Sender<ReadCommand>,
    actor: JoinHandle<()>,
}

impl PlaybackManager {
    pub(crate) fn new_with_screen_wake(
        core: Arc<SparrowCore>,
        access: HttpPlaybackAccess,
        preferences: AudioPreferenceStore,
        private_root: PathBuf,
        screen_wake: Arc<dyn ScreenWake>,
    ) -> Self {
        Self::with_all_adapters(
            core,
            Arc::new(access),
            preferences,
            Arc::new(MpegTsPlaybackTransportSelector),
            screen_wake,
            system_mpv_player(private_root),
        )
    }

    #[cfg(test)]
    fn with_access(core: Arc<SparrowCore>, access: Arc<dyn NativePlaybackAccess>) -> Self {
        Self::with_access_and_screen_wake(core, access, noop_screen_wake())
    }

    #[cfg(test)]
    fn with_access_and_screen_wake(
        core: Arc<SparrowCore>,
        access: Arc<dyn NativePlaybackAccess>,
        screen_wake: Arc<dyn ScreenWake>,
    ) -> Self {
        Self::with_access_preferences_selector_and_screen_wake(
            core,
            access,
            AudioPreferenceStore::disabled(),
            Arc::new(PassthroughPlaybackTransportSelector),
            screen_wake,
        )
    }

    #[cfg(test)]
    fn with_access_preferences_and_selector(
        core: Arc<SparrowCore>,
        access: Arc<dyn NativePlaybackAccess>,
        preferences: AudioPreferenceStore,
        selector: Arc<dyn PlaybackTransportSelector>,
    ) -> Self {
        Self::with_access_preferences_selector_and_screen_wake(
            core,
            access,
            preferences,
            selector,
            noop_screen_wake(),
        )
    }

    #[cfg(test)]
    fn with_access_preferences_selector_and_screen_wake(
        core: Arc<SparrowCore>,
        access: Arc<dyn NativePlaybackAccess>,
        preferences: AudioPreferenceStore,
        selector: Arc<dyn PlaybackTransportSelector>,
        screen_wake: Arc<dyn ScreenWake>,
    ) -> Self {
        Self::with_all_adapters(
            core,
            access,
            preferences,
            selector,
            screen_wake,
            Arc::new(UnsupportedMpvPlayer),
        )
    }

    #[cfg(test)]
    fn with_adapters(
        core: Arc<SparrowCore>,
        access: Arc<dyn NativePlaybackAccess>,
        mpv: Arc<dyn NativeMpvPlayer>,
    ) -> Self {
        Self::with_all_adapters(
            core,
            access,
            AudioPreferenceStore::disabled(),
            Arc::new(PassthroughPlaybackTransportSelector),
            noop_screen_wake(),
            mpv,
        )
    }

    fn with_all_adapters(
        core: Arc<SparrowCore>,
        access: Arc<dyn NativePlaybackAccess>,
        preferences: AudioPreferenceStore,
        selector: Arc<dyn PlaybackTransportSelector>,
        screen_wake: Arc<dyn ScreenWake>,
        mpv: Arc<dyn NativeMpvPlayer>,
    ) -> Self {
        let (controls, control_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (reads, read_receiver) = mpsc::channel(READ_CAPACITY);
        let actor = tokio::spawn(
            PlaybackActor::new(
                PlaybackActorDependencies {
                    core,
                    access,
                    preferences,
                    selector,
                    screen_wake,
                    mpv,
                },
                control_receiver,
                read_receiver,
            )
            .run(),
        );
        Self {
            controls,
            reads,
            actor,
        }
    }

    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub(crate) async fn start(
        &self,
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
    ) -> Result<StartedPlayback, PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn read(
        &self,
        session_id: PlaybackSessionId,
        stream_handle: NativeStreamHandle,
    ) -> Result<Vec<u8>, PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.reads
            .send(ReadCommand {
                session_id,
                stream_handle,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    /// Confirms that the exact Rust-owned native transport generation is still
    /// live. Presentation adapters use this immediately before publishing an
    /// external player so a completed transport cleanup cannot be undone by a
    /// delayed presentation start.
    pub(crate) async fn validate_active_generation(
        &self,
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::ValidateActiveGeneration {
                session_id,
                expected_stream_handle,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn suspend(
        &self,
        session_id: PlaybackSessionId,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Suspend { session_id, reply })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    /// Cancels only the exact native transport generation while retaining its
    /// pinned session for a coordinated presentation release and restart.
    pub(crate) async fn suspend_generation(
        &self,
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::SuspendGeneration {
                session_id,
                expected_stream_handle,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub(crate) async fn reopen(
        &self,
        session_id: PlaybackSessionId,
    ) -> Result<StartedPlayback, PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Reopen { session_id, reply })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn restart(
        &self,
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
        intent: PlaybackRestartIntent,
    ) -> Result<StartedPlayback, PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Restart {
                session_id,
                expected_stream_handle,
                intent,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn stop(
        &self,
        session_id: PlaybackSessionId,
        stream_handle: Option<NativeStreamHandle>,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn set_activity(
        &self,
        session_id: PlaybackSessionId,
        active: bool,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn suspend_for_lifecycle(&self) -> Result<(), PlaybackManagerError> {
        self.report_lifecycle(PlaybackLifecycle::Suspended).await
    }

    pub(crate) async fn resume_for_lifecycle(&self) -> Result<(), PlaybackManagerError> {
        self.report_lifecycle(PlaybackLifecycle::Resumed).await
    }

    async fn report_lifecycle(
        &self,
        lifecycle: PlaybackLifecycle,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Lifecycle { lifecycle, reply })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    /// Resolves a Channel and starts system mpv as its Primary Playback Engine.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) async fn start_mpv_primary(
        &self,
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
    ) -> Result<MpvPlaybackSession, PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    /// Applies one control only when system mpv owns the correlated Playback Session.
    pub(crate) async fn control_mpv(
        &self,
        session_id: PlaybackSessionId,
        control: MpvPlaybackControl,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::ControlMpv {
                session_id,
                control,
                reply,
            })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    /// Reopens a suspended Linux mpv Primary Playback Engine at the live edge.
    pub(crate) async fn reopen_mpv(
        &self,
        session_id: PlaybackSessionId,
    ) -> Result<MpvPlaybackSession, PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::ReopenMpv { session_id, reply })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }

    pub(crate) async fn shutdown(&self) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Shutdown { reply })
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?;
        response
            .await
            .map_err(|_| PlaybackManagerError::Unavailable)?
    }
}

impl Drop for PlaybackManager {
    fn drop(&mut self) {
        self.actor.abort();
    }
}

enum ControlCommand {
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Start {
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
        reply: DescriptorReply,
    },
    Suspend {
        session_id: PlaybackSessionId,
        reply: UnitReply,
    },
    SuspendGeneration {
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
        reply: UnitReply,
    },
    ValidateActiveGeneration {
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
        reply: UnitReply,
    },
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Reopen {
        session_id: PlaybackSessionId,
        reply: DescriptorReply,
    },
    Restart {
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
        intent: PlaybackRestartIntent,
        reply: DescriptorReply,
    },
    Stop {
        session_id: PlaybackSessionId,
        stream_handle: Option<NativeStreamHandle>,
        reply: UnitReply,
    },
    SetActivity {
        session_id: PlaybackSessionId,
        active: bool,
        reply: UnitReply,
    },
    Lifecycle {
        lifecycle: PlaybackLifecycle,
        reply: UnitReply,
    },
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    StartMpvPrimary {
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
        reply: MpvReply,
    },
    ControlMpv {
        session_id: PlaybackSessionId,
        control: MpvPlaybackControl,
        reply: UnitReply,
    },
    ReopenMpv {
        session_id: PlaybackSessionId,
        reply: MpvReply,
    },
    Shutdown {
        reply: UnitReply,
    },
}

#[derive(Clone, Copy)]
enum PlaybackLifecycle {
    Suspended,
    Resumed,
}

struct ReadCommand {
    session_id: PlaybackSessionId,
    stream_handle: NativeStreamHandle,
    reply: oneshot::Sender<Result<Vec<u8>, PlaybackManagerError>>,
}

struct PlaybackActorDependencies {
    core: Arc<SparrowCore>,
    access: Arc<dyn NativePlaybackAccess>,
    preferences: AudioPreferenceStore,
    selector: Arc<dyn PlaybackTransportSelector>,
    screen_wake: Arc<dyn ScreenWake>,
    mpv: Arc<dyn NativeMpvPlayer>,
}

struct PlaybackActor {
    dependencies: PlaybackActorDependencies,
    controls: mpsc::Receiver<ControlCommand>,
    reads: mpsc::Receiver<ReadCommand>,
    tombstones: VecDeque<PlaybackSessionId>,
    suspend_intents: VecDeque<PlaybackSessionId>,
    next_handle: u64,
    foreground: bool,
    active_intent: Option<PlaybackSessionId>,
    resume_intent: Option<PlaybackSessionId>,
}

impl PlaybackActor {
    fn new(
        dependencies: PlaybackActorDependencies,
        controls: mpsc::Receiver<ControlCommand>,
        reads: mpsc::Receiver<ReadCommand>,
    ) -> Self {
        Self {
            dependencies,
            controls,
            reads,
            tombstones: VecDeque::with_capacity(MAX_STOP_TOMBSTONES),
            suspend_intents: VecDeque::with_capacity(MAX_SUSPEND_INTENTS),
            next_handle: 1,
            foreground: true,
            active_intent: None,
            resume_intent: None,
        }
    }

    async fn run(mut self) {
        let mut state = ActorState::Idle;
        loop {
            state = match state {
                ActorState::Idle => match self.next_command().await {
                    Some(ActorCommand::Control(command)) => self.control_idle(command),
                    Some(ActorCommand::Read(command)) => {
                        let _ = command.reply.send(Err(PlaybackManagerError::Cancelled));
                        ActorState::Idle
                    }
                    None => return,
                },
                ActorState::Opening(mut opening) => {
                    tokio::select! {
                        biased;
                        command = self.controls.recv() => match command {
                            Some(command) => self.control_opening(opening, command),
                            None => return,
                        },
                        () = opening.reply.as_mut().expect("opening reply exists").closed() => {
                            self.drop_abandoned_open(opening)
                        }
                        result = &mut opening.pending => self.finish_open(opening, result),
                        read = self.reads.recv() => match read {
                            Some(read) => {
                                let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
                                ActorState::Opening(opening)
                            }
                            None => return,
                        },
                    }
                }
                ActorState::Streaming(streaming) => {
                    tokio::select! {
                        biased;
                        command = self.controls.recv() => match command {
                            Some(command) => self.control_streaming(streaming, command),
                            None => return,
                        },
                        read = self.reads.recv() => match read {
                            Some(read) => self.begin_read(streaming, read),
                            None => return,
                        },
                    }
                }
                ActorState::Reading(mut reading) => {
                    tokio::select! {
                        biased;
                        command = self.controls.recv() => match command {
                            Some(command) => self.control_reading(reading, command),
                            None => return,
                        },
                        () = reading.reply.closed() => self.drop_abandoned_read(reading),
                        result = &mut reading.pending => self.finish_read(reading, result),
                        read = self.reads.recv() => match read {
                            Some(read) => {
                                let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
                                ActorState::Reading(reading)
                            }
                            None => return,
                        },
                    }
                }
                ActorState::Dormant(session) => match self.next_command().await {
                    Some(ActorCommand::Control(command)) => self.control_dormant(session, command),
                    Some(ActorCommand::Read(command)) => {
                        let _ = command.reply.send(Err(PlaybackManagerError::Cancelled));
                        ActorState::Dormant(session)
                    }
                    None => return,
                },
                ActorState::OpeningMpv(mut opening) => {
                    tokio::select! {
                        biased;
                        command = self.controls.recv() => match command {
                            Some(command) => self.control_opening_mpv(opening, command).await,
                            None => return,
                        },
                        () = opening.reply.as_mut().expect("mpv opening reply exists").closed() => {
                            self.drop_abandoned_mpv_open(opening)
                        }
                        result = &mut opening.pending => self.finish_mpv_open(opening, result).await,
                        read = self.reads.recv() => match read {
                            Some(read) => {
                                let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
                                ActorState::OpeningMpv(opening)
                            }
                            None => return,
                        },
                    }
                }
                ActorState::MpvPlaying(mut playing) => {
                    tokio::select! {
                        biased;
                        command = self.controls.recv() => match command {
                            Some(command) => self.control_mpv_playing(playing, command).await,
                            None => return,
                        },
                        exit = &mut playing.process.exited => {
                            self.finish_mpv_exit(playing, exit)
                        }
                        read = self.reads.recv() => match read {
                            Some(read) => {
                                let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
                                ActorState::MpvPlaying(playing)
                            }
                            None => return,
                        },
                    }
                }
                ActorState::Exited => return,
            };
        }
    }

    async fn next_command(&mut self) -> Option<ActorCommand> {
        tokio::select! {
            biased;
            command = self.controls.recv() => command.map(ActorCommand::Control),
            read = self.reads.recv() => read.map(ActorCommand::Read),
        }
    }

    fn control_idle(&mut self, command: ControlCommand) -> ActorState {
        match command {
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                if reply.is_closed() {
                    ActorState::Idle
                } else {
                    self.begin_start(session_id, channel_id, reply)
                }
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
            }
            ControlCommand::ValidateActiveGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
            }
            ControlCommand::Reopen { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
            }
            ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::SetActivity { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
            }
            ControlCommand::Lifecycle { lifecycle, reply } => {
                let result = self.update_lifecycle(None, lifecycle, false);
                let _ = reply.send(result);
                ActorState::Idle
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => ActorState::Idle,
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => self.begin_mpv_primary(session_id, channel_id, reply),
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::StaleSession)));
                ActorState::Idle
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::StaleSession)));
                ActorState::Idle
            }
            ControlCommand::Shutdown { reply } => {
                let _ = reply.send(Ok(()));
                ActorState::Exited
            }
        }
    }

    fn control_opening(
        &mut self,
        mut opening: OpeningState,
        command: ControlCommand,
    ) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::Opening(opening)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == opening.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = opening.session.id.clone();
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                self.retire(old_id);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Suspend { session_id, reply } if session_id == opening.session.id => {
                let activity = self.update_activity(&mut opening.session, session_id, false, false);
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                let _ = reply.send(activity);
                Self::dormant(opening.session, DormantReason::Suspended)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Opening(opening)
            }
            ControlCommand::SuspendGeneration {
                session_id,
                expected_stream_handle,
                reply,
            } if session_id == opening.session.id
                && opening.session.last_stream_handle.as_ref() == Some(&expected_stream_handle) =>
            {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                self.reply_with_dormant(reply, Ok(()), opening.session, DormantReason::Suspended)
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::ValidateActiveGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::Reopen { reply, .. } if reply.is_closed() => {
                ActorState::Opening(opening)
            }
            ControlCommand::Reopen { session_id, reply } if session_id == opening.session.id => {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                self.begin_reopen(opening.session, reply)
            }
            ControlCommand::Reopen { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::Restart { reply, .. } if reply.is_closed() => {
                ActorState::Opening(opening)
            }
            ControlCommand::Restart {
                session_id,
                expected_stream_handle,
                intent,
                reply,
            } if session_id == opening.session.id
                && opening.session.last_stream_handle.as_ref() == Some(&expected_stream_handle) =>
            {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                self.begin_restart(opening.session, intent, reply)
            }
            ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            } if session_id == opening.session.id
                && opening.session.last_stream_handle.as_ref() == stream_handle.as_ref() =>
            {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                self.reply_with_stopped(reply, opening.session)
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } if session_id != opening.session.id => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Opening(opening)
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => {
                ActorState::Opening(opening)
            }
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == opening.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = opening.session.id.clone();
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                self.retire(old_id);
                self.begin_mpv_primary(session_id, channel_id, reply)
            }
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Opening(opening)
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Opening(opening)
            }
            ControlCommand::Shutdown { reply } => {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                let _ = reply.send(Ok(()));
                ActorState::Exited
            }
            ControlCommand::Stop { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            } => {
                let result = self.update_activity(&mut opening.session, session_id, active, false);
                let _ = reply.send(result);
                ActorState::Opening(opening)
            }
            ControlCommand::Lifecycle {
                lifecycle: PlaybackLifecycle::Suspended,
                reply,
            } => {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                let result = self.update_lifecycle(
                    Some(&mut opening.session),
                    PlaybackLifecycle::Suspended,
                    false,
                );
                let _ = reply.send(result);
                Self::dormant(opening.session, DormantReason::Suspended)
            }
            ControlCommand::Lifecycle {
                lifecycle: PlaybackLifecycle::Resumed,
                reply,
            } => {
                let result = self.update_lifecycle(
                    Some(&mut opening.session),
                    PlaybackLifecycle::Resumed,
                    false,
                );
                let _ = reply.send(result);
                ActorState::Opening(opening)
            }
        }
    }

    fn control_streaming(
        &mut self,
        mut streaming: StreamingState,
        command: ControlCommand,
    ) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::Streaming(streaming)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == streaming.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Streaming(streaming)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = streaming.session.id.clone();
                drop(streaming.stream);
                drop(streaming.session);
                self.retire(old_id);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Suspend { session_id, reply } if session_id == streaming.session.id => {
                let activity =
                    self.update_activity(&mut streaming.session, session_id, false, false);
                drop(streaming.stream);
                let _ = reply.send(activity);
                Self::dormant(streaming.session, DormantReason::Suspended)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Streaming(streaming)
            }
            ControlCommand::SuspendGeneration {
                session_id,
                expected_stream_handle,
                reply,
            } if session_id == streaming.session.id
                && expected_stream_handle == streaming.stream.handle =>
            {
                drop(streaming.stream);
                self.reply_with_dormant(reply, Ok(()), streaming.session, DormantReason::Suspended)
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Streaming(streaming)
            }
            ControlCommand::ValidateActiveGeneration {
                session_id,
                expected_stream_handle,
                reply,
            } => {
                let result = if session_id == streaming.session.id
                    && expected_stream_handle == streaming.stream.handle
                {
                    Ok(())
                } else {
                    Err(PlaybackManagerError::Cancelled)
                };
                let _ = reply.send(result);
                ActorState::Streaming(streaming)
            }
            ControlCommand::Reopen { reply, .. } if reply.is_closed() => {
                ActorState::Streaming(streaming)
            }
            ControlCommand::Reopen { session_id, reply } if session_id == streaming.session.id => {
                drop(streaming.stream);
                self.begin_reopen(streaming.session, reply)
            }
            ControlCommand::Reopen { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Streaming(streaming)
            }
            ControlCommand::Restart { reply, .. } if reply.is_closed() => {
                ActorState::Streaming(streaming)
            }
            ControlCommand::Restart {
                session_id,
                expected_stream_handle,
                intent,
                reply,
            } if session_id == streaming.session.id
                && expected_stream_handle == streaming.stream.handle =>
            {
                drop(streaming.stream);
                self.begin_restart(streaming.session, intent, reply)
            }
            ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Streaming(streaming)
            }
            ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            } if session_id == streaming.session.id
                && stream_handle.as_ref() == Some(&streaming.stream.handle) =>
            {
                drop(streaming.stream);
                self.reply_with_stopped(reply, streaming.session)
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } if session_id != streaming.session.id => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Streaming(streaming)
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => {
                ActorState::Streaming(streaming)
            }
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == streaming.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Streaming(streaming)
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = streaming.session.id.clone();
                drop(streaming.stream);
                drop(streaming.session);
                self.retire(old_id);
                self.begin_mpv_primary(session_id, channel_id, reply)
            }
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Streaming(streaming)
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Streaming(streaming)
            }
            ControlCommand::Shutdown { reply } => {
                drop(streaming.stream);
                drop(streaming.session);
                let _ = reply.send(Ok(()));
                ActorState::Exited
            }
            ControlCommand::Stop { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Streaming(streaming)
            }
            ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            } => {
                let result =
                    self.update_activity(&mut streaming.session, session_id, active, false);
                let _ = reply.send(result);
                ActorState::Streaming(streaming)
            }
            ControlCommand::Lifecycle {
                lifecycle: PlaybackLifecycle::Suspended,
                reply,
            } => {
                drop(streaming.stream);
                let result = self.update_lifecycle(
                    Some(&mut streaming.session),
                    PlaybackLifecycle::Suspended,
                    false,
                );
                let _ = reply.send(result);
                Self::dormant(streaming.session, DormantReason::Suspended)
            }
            ControlCommand::Lifecycle {
                lifecycle: PlaybackLifecycle::Resumed,
                reply,
            } => {
                let result = self.update_lifecycle(
                    Some(&mut streaming.session),
                    PlaybackLifecycle::Resumed,
                    false,
                );
                let _ = reply.send(result);
                ActorState::Streaming(streaming)
            }
        }
    }

    fn control_reading(
        &mut self,
        mut reading: ReadingState,
        command: ControlCommand,
    ) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::Reading(reading)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == reading.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = reading.session.id.clone();
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                drop(reading.session);
                self.retire(old_id);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Suspend { session_id, reply } if session_id == reading.session.id => {
                let activity = self.update_activity(&mut reading.session, session_id, false, false);
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                let _ = reply.send(activity);
                Self::dormant(reading.session, DormantReason::Suspended)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Reading(reading)
            }
            ControlCommand::SuspendGeneration {
                session_id,
                expected_stream_handle,
                reply,
            } if session_id == reading.session.id
                && reading.session.last_stream_handle.as_ref() == Some(&expected_stream_handle) =>
            {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                self.reply_with_dormant(reply, Ok(()), reading.session, DormantReason::Suspended)
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::ValidateActiveGeneration {
                session_id,
                expected_stream_handle,
                reply,
            } => {
                let result = if session_id == reading.session.id
                    && reading.session.last_stream_handle.as_ref() == Some(&expected_stream_handle)
                {
                    Ok(())
                } else {
                    Err(PlaybackManagerError::Cancelled)
                };
                let _ = reply.send(result);
                ActorState::Reading(reading)
            }
            ControlCommand::Reopen { reply, .. } if reply.is_closed() => {
                ActorState::Reading(reading)
            }
            ControlCommand::Reopen { session_id, reply } if session_id == reading.session.id => {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                self.begin_reopen(reading.session, reply)
            }
            ControlCommand::Reopen { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::Restart { reply, .. } if reply.is_closed() => {
                ActorState::Reading(reading)
            }
            ControlCommand::Restart {
                session_id,
                expected_stream_handle,
                intent,
                reply,
            } if session_id == reading.session.id
                && reading.session.last_stream_handle.as_ref() == Some(&expected_stream_handle) =>
            {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                self.begin_restart(reading.session, intent, reply)
            }
            ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            } if session_id == reading.session.id
                && reading.session.last_stream_handle.as_ref() == stream_handle.as_ref() =>
            {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                self.reply_with_stopped(reply, reading.session)
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } if session_id != reading.session.id => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Reading(reading)
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => {
                ActorState::Reading(reading)
            }
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == reading.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = reading.session.id.clone();
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                drop(reading.session);
                self.retire(old_id);
                self.begin_mpv_primary(session_id, channel_id, reply)
            }
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Reading(reading)
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Reading(reading)
            }
            ControlCommand::Shutdown { reply } => {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                drop(reading.session);
                let _ = reply.send(Ok(()));
                ActorState::Exited
            }
            ControlCommand::Stop { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            } => {
                let result = self.update_activity(&mut reading.session, session_id, active, false);
                let _ = reply.send(result);
                ActorState::Reading(reading)
            }
            ControlCommand::Lifecycle {
                lifecycle: PlaybackLifecycle::Suspended,
                reply,
            } => {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                let result = self.update_lifecycle(
                    Some(&mut reading.session),
                    PlaybackLifecycle::Suspended,
                    false,
                );
                let _ = reply.send(result);
                Self::dormant(reading.session, DormantReason::Suspended)
            }
            ControlCommand::Lifecycle {
                lifecycle: PlaybackLifecycle::Resumed,
                reply,
            } => {
                let result = self.update_lifecycle(
                    Some(&mut reading.session),
                    PlaybackLifecycle::Resumed,
                    false,
                );
                let _ = reply.send(result);
                ActorState::Reading(reading)
            }
        }
    }

    fn control_dormant(&mut self, mut session: Session, command: ControlCommand) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::Dormant(session)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = session.id.clone();
                drop(session);
                self.retire(old_id);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Suspend { session_id, reply } if session_id == session.id => {
                self.reply_with_dormant(reply, Ok(()), session, DormantReason::Suspended)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Dormant(session)
            }
            ControlCommand::SuspendGeneration {
                session_id,
                expected_stream_handle,
                reply,
            } if session_id == session.id
                && session.last_stream_handle.as_ref() == Some(&expected_stream_handle) =>
            {
                self.reply_with_dormant(reply, Ok(()), session, DormantReason::Suspended)
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::ValidateActiveGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::Reopen { reply, .. } if reply.is_closed() => {
                ActorState::Dormant(session)
            }
            ControlCommand::Reopen { session_id, reply } if session_id == session.id => {
                self.begin_reopen(session, reply)
            }
            ControlCommand::Reopen { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::Restart { reply, .. } if reply.is_closed() => {
                ActorState::Dormant(session)
            }
            ControlCommand::Restart {
                session_id,
                expected_stream_handle,
                intent,
                reply,
            } if session_id == session.id
                && session.last_stream_handle.as_ref() == Some(&expected_stream_handle) =>
            {
                self.begin_restart(session, intent, reply)
            }
            ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            } if session_id == session.id
                && session.last_stream_handle.as_ref() == stream_handle.as_ref() =>
            {
                self.reply_with_stopped(reply, session)
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } if session_id != session.id => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Dormant(session)
            }
            ControlCommand::Stop { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => {
                ActorState::Dormant(session)
            }
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Dormant(session)
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = session.id.clone();
                drop(session);
                self.retire(old_id);
                self.begin_mpv_primary(session_id, channel_id, reply)
            }
            ControlCommand::ControlMpv {
                session_id, reply, ..
            } if session_id == session.id
                && matches!(session.dormant_reason, Some(DormantReason::Failed)) =>
            {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::Terminated)));
                ActorState::Dormant(session)
            }
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::StaleSession)));
                ActorState::Dormant(session)
            }
            ControlCommand::ReopenMpv { reply, .. } if reply.is_closed() => {
                ActorState::Dormant(session)
            }
            ControlCommand::ReopenMpv { session_id, reply }
                if session_id == session.id
                    && matches!(
                        session.dormant_reason,
                        Some(DormantReason::Failed | DormantReason::Suspended)
                    ) =>
            {
                self.begin_mpv_open(session, reply)
            }
            ControlCommand::ReopenMpv { session_id, reply } if session_id == session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::Dormant(session)
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::StaleSession)));
                ActorState::Dormant(session)
            }
            ControlCommand::Shutdown { reply } => {
                drop(session);
                let _ = reply.send(Ok(()));
                ActorState::Exited
            }
            ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            } => {
                let result = self.update_activity(&mut session, session_id, active, true);
                let _ = reply.send(result);
                ActorState::Dormant(session)
            }
            ControlCommand::Lifecycle { lifecycle, reply } => {
                let result = self.update_lifecycle(Some(&mut session), lifecycle, true);
                let _ = reply.send(result);
                ActorState::Dormant(session)
            }
        }
    }

    fn begin_mpv_primary(
        &mut self,
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
        reply: MpvReply,
    ) -> ActorState {
        if self.tombstones.contains(&session_id) {
            let _ = reply.send(Err(PlaybackManagerError::Cancelled));
            return ActorState::Idle;
        }

        let source = match self.dependencies.core.resolve_playback(&channel_id) {
            Ok(source) => Arc::new(source),
            Err(error) => {
                let _ = reply.send(Err(PlaybackManagerError::Core(error)));
                return ActorState::Idle;
            }
        };
        let session = Session {
            id: session_id,
            channel_id,
            source,
            current_track: None,
            last_stream_handle: None,
            resume_activity: None,
            dormant_reason: None,
        };
        if !self.foreground || self.take_suspend(&session.id) {
            return self.reply_with_dormant(
                reply,
                Err(PlaybackManagerError::Cancelled),
                session,
                DormantReason::Suspended,
            );
        }
        self.begin_mpv_open(session, reply)
    }

    fn begin_mpv_open(&mut self, session: Session, reply: MpvReply) -> ActorState {
        let pending = self.dependencies.mpv.launch(Arc::clone(&session.source));
        ActorState::OpeningMpv(MpvOpeningState {
            pending,
            session,
            reply: Some(reply),
        })
    }

    async fn finish_mpv_open(
        &mut self,
        mut opening: MpvOpeningState,
        result: Result<MpvProcess, MpvFailure>,
    ) -> ActorState {
        let reply = opening.reply.take().expect("mpv opening reply exists");
        match result {
            Ok(process) => {
                let started = MpvPlaybackSession {
                    session_id: opening.session.id.clone(),
                };
                match reply.send(Ok(started)) {
                    Ok(()) => ActorState::MpvPlaying(MpvPlayingState {
                        process,
                        session: opening.session,
                    }),
                    Err(_) => {
                        let _ = process.stop().await;
                        self.drop_session(opening.session)
                    }
                }
            }
            Err(error) => self.reply_with_dormant(
                reply,
                Err(PlaybackManagerError::Mpv(error)),
                opening.session,
                DormantReason::Failed,
            ),
        }
    }

    fn drop_abandoned_mpv_open(&mut self, mut opening: MpvOpeningState) -> ActorState {
        drop(opening.pending);
        drop(opening.reply.take());
        ActorState::Dormant(opening.session)
    }

    async fn control_opening_mpv(
        &mut self,
        mut opening: MpvOpeningState,
        command: ControlCommand,
    ) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if session_id == opening.session.id || self.tombstones.contains(&session_id) => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = opening.session.id.clone();
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("mpv opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                self.retire(old_id);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Suspend { session_id, reply } if session_id == opening.session.id => {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("mpv opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                self.reply_with_dormant(reply, Ok(()), opening.session, DormantReason::Suspended)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::ValidateActiveGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Reopen { reply, .. } | ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            } if session_id == opening.session.id
                && opening.session.last_stream_handle.as_ref() == stream_handle.as_ref() =>
            {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("mpv opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } if session_id != opening.session.id => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Stop { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => {
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == opening.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = opening.session.id.clone();
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("mpv opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                self.retire(old_id);
                self.begin_mpv_primary(session_id, channel_id, reply)
            }
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(
                    MpvFailure::ControlUnavailable,
                )));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Shutdown { reply } => {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("mpv opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                let _ = reply.send(Ok(()));
                ActorState::Exited
            }
            ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            } => {
                let result = self.update_activity(&mut opening.session, session_id, active, true);
                let _ = reply.send(result);
                ActorState::OpeningMpv(opening)
            }
            ControlCommand::Lifecycle { lifecycle, reply } => {
                let result = self.update_lifecycle(Some(&mut opening.session), lifecycle, true);
                let _ = reply.send(result);
                ActorState::OpeningMpv(opening)
            }
        }
    }

    async fn control_mpv_playing(
        &mut self,
        mut playing: MpvPlayingState,
        command: ControlCommand,
    ) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if session_id == playing.session.id || self.tombstones.contains(&session_id) => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = playing.session.id.clone();
                let result = playing.process.stop().await;
                drop(playing.session);
                self.retire(old_id);
                match result {
                    Ok(()) => self.begin_start(session_id, channel_id, reply),
                    Err(error) => {
                        let _ = reply.send(Err(PlaybackManagerError::Mpv(error)));
                        ActorState::Idle
                    }
                }
            }
            ControlCommand::Suspend { session_id, reply } if session_id == playing.session.id => {
                let result = playing.process.stop().await;
                match result {
                    Ok(()) => self.reply_with_dormant(
                        reply,
                        Ok(()),
                        playing.session,
                        DormantReason::Suspended,
                    ),
                    Err(error) => {
                        drop(playing.session);
                        self.retire(session_id);
                        let _ = reply.send(Err(PlaybackManagerError::Mpv(error)));
                        ActorState::Idle
                    }
                }
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::SuspendGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::ValidateActiveGeneration { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Reopen { reply, .. } | ControlCommand::Restart { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Stop {
                session_id,
                stream_handle,
                reply,
            } if session_id == playing.session.id
                && playing.session.last_stream_handle.as_ref() == stream_handle.as_ref() =>
            {
                let result = playing.process.stop().await;
                drop(playing.session);
                self.retire(session_id);
                let _ = reply.send(result.map_err(PlaybackManagerError::Mpv));
                ActorState::Idle
            }
            ControlCommand::Stop {
                session_id, reply, ..
            } if session_id != playing.session.id => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Stop { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::StartMpvPrimary { reply, .. } if reply.is_closed() => {
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::StartMpvPrimary {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) || session_id == playing.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::StartMpvPrimary {
                session_id,
                channel_id,
                reply,
            } => {
                let old_id = playing.session.id.clone();
                let result = playing.process.stop().await;
                drop(playing.session);
                self.retire(old_id);
                match result {
                    Ok(()) => self.begin_mpv_primary(session_id, channel_id, reply),
                    Err(error) => {
                        let _ = reply.send(Err(PlaybackManagerError::Mpv(error)));
                        ActorState::Idle
                    }
                }
            }
            ControlCommand::ControlMpv {
                session_id,
                control,
                reply,
            } if session_id == playing.session.id => {
                let result = playing
                    .process
                    .control(control)
                    .await
                    .map_err(PlaybackManagerError::Mpv);
                let _ = reply.send(result);
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::ControlMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::StaleSession)));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::ReopenMpv { session_id, reply } if session_id == playing.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::PrimaryActive)));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::ReopenMpv { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Mpv(MpvFailure::StaleSession)));
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Shutdown { reply } => {
                let result = playing.process.stop().await;
                drop(playing.session);
                let _ = reply.send(result.map_err(PlaybackManagerError::Mpv));
                ActorState::Exited
            }
            ControlCommand::SetActivity {
                session_id,
                active,
                reply,
            } => {
                let result = self.update_activity(&mut playing.session, session_id, active, true);
                let _ = reply.send(result);
                ActorState::MpvPlaying(playing)
            }
            ControlCommand::Lifecycle { lifecycle, reply } => {
                let result = self.update_lifecycle(Some(&mut playing.session), lifecycle, true);
                let _ = reply.send(result);
                ActorState::MpvPlaying(playing)
            }
        }
    }

    fn finish_mpv_exit(&mut self, playing: MpvPlayingState, exit: MpvExit) -> ActorState {
        match exit {
            MpvExit::Terminated => {
                drop(playing.process);
                self.failed_dormant(playing.session)
            }
        }
    }

    fn begin_start(
        &mut self,
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
        reply: DescriptorReply,
    ) -> ActorState {
        if self.tombstones.contains(&session_id) {
            let _ = reply.send(Err(PlaybackManagerError::Cancelled));
            return ActorState::Idle;
        }

        let activity = self.dependencies.core.begin_playback_activity();
        let source = match self.dependencies.core.resolve_playback(&channel_id) {
            Ok(source) => Arc::new(source),
            Err(error) => {
                drop(activity);
                let _ = reply.send(Err(PlaybackManagerError::Core(error)));
                return ActorState::Idle;
            }
        };
        let session = Session {
            id: session_id,
            channel_id: channel_id.clone(),
            source,
            current_track: None,
            last_stream_handle: None,
            resume_activity: None,
            dormant_reason: None,
        };
        if !self.foreground || self.take_suspend(&session.id) {
            drop(activity);
            return self.reply_with_dormant(
                reply,
                Err(PlaybackManagerError::Cancelled),
                session,
                DormantReason::Suspended,
            );
        }
        let request = SelectionRequest::Initial {
            saved: self.dependencies.preferences.preference(&channel_id),
        };
        self.begin_open(session, activity, request, reply)
    }

    fn begin_reopen(&mut self, mut session: Session, reply: DescriptorReply) -> ActorState {
        if !self.foreground {
            return self.reply_with_dormant(
                reply,
                Err(PlaybackManagerError::Cancelled),
                session,
                DormantReason::Suspended,
            );
        }
        let activity = session
            .resume_activity
            .take()
            .unwrap_or_else(|| self.dependencies.core.begin_playback_activity());
        let request = SelectionRequest::Continue {
            current: session.current_track.clone(),
            saved: self
                .dependencies
                .preferences
                .preference(&session.channel_id),
        };
        self.begin_open(session, activity, request, reply)
    }

    fn begin_restart(
        &mut self,
        mut session: Session,
        intent: PlaybackRestartIntent,
        reply: DescriptorReply,
    ) -> ActorState {
        if !self.foreground {
            return self.reply_with_dormant(
                reply,
                Err(PlaybackManagerError::Cancelled),
                session,
                DormantReason::Suspended,
            );
        }
        let request = match intent {
            PlaybackRestartIntent::Retry | PlaybackRestartIntent::Resume => {
                SelectionRequest::Continue {
                    current: session.current_track.clone(),
                    saved: self
                        .dependencies
                        .preferences
                        .preference(&session.channel_id),
                }
            }
            PlaybackRestartIntent::SelectAudio(track_id) => SelectionRequest::Requested(track_id),
        };
        let activity = session
            .resume_activity
            .take()
            .unwrap_or_else(|| self.dependencies.core.begin_playback_activity());
        self.begin_open(session, activity, request, reply)
    }

    fn begin_open(
        &mut self,
        mut session: Session,
        activity: PlaybackActivityLease,
        request: SelectionRequest,
        reply: DescriptorReply,
    ) -> ActorState {
        session.dormant_reason = None;
        let stream_handle = match self.allocate_handle() {
            Some(stream_handle) => stream_handle,
            None => {
                drop(activity);
                return self.reply_with_dormant(
                    reply,
                    Err(PlaybackManagerError::Unavailable),
                    session,
                    DormantReason::Failed,
                );
            }
        };
        let access = self.dependencies.access.open(Arc::clone(&session.source));
        let selector = Arc::clone(&self.dependencies.selector);
        let preferences = self.dependencies.preferences.clone();
        let channel_id = session.channel_id.clone();
        let pending_request = request.clone();
        let future = Box::pin(async move {
            let body = access.await.map_err(PendingOpenError::Access)?;
            let opened = selector
                .open(body, pending_request.clone())
                .await
                .map_err(PendingOpenError::TransportStream)?;
            let preference_status = match pending_request {
                SelectionRequest::Requested(requested)
                    if opened.selection.track_id() == Some(&requested) =>
                {
                    Some(match preferences.remember(channel_id, requested).await {
                        PreferenceWrite::Saved => PreferenceStatus::Saved,
                        PreferenceWrite::NotSaved => PreferenceStatus::NotSaved,
                        PreferenceWrite::Unchanged => PreferenceStatus::Unchanged,
                    })
                }
                SelectionRequest::Requested(_) => Some(PreferenceStatus::NotSaved),
                SelectionRequest::Initial { .. } | SelectionRequest::Continue { .. } => None,
            };
            Ok(OpenedPlayback {
                opened,
                preference_status,
            })
        });
        let pending = PendingOpen::new(future, activity);
        ActorState::Opening(OpeningState {
            pending: Box::pin(pending),
            session,
            stream_handle,
            reply: Some(reply),
        })
    }

    fn finish_open(
        &mut self,
        mut opening: OpeningState,
        result: Result<(OpenedPlayback, PlaybackActivityLease), PendingOpenError>,
    ) -> ActorState {
        let reply = opening.reply.take().expect("opening reply exists");
        match result {
            Ok((opened, activity)) => {
                opening.session.current_track = opened.opened.selection.track_id().cloned();
                opening.session.last_stream_handle = Some(opening.stream_handle.clone());
                let descriptor = StartedPlayback {
                    session_id: opening.session.id.clone(),
                    stream_handle: opening.stream_handle.clone(),
                    tracks: opened.opened.tracks,
                    selection: opened.opened.selection,
                    preference_status: opened.preference_status,
                };
                let stream = StreamInstance {
                    body: opened.opened.body,
                    remainder: Bytes::new(),
                    _activity: activity,
                    handle: opening.stream_handle,
                };
                match reply.send(Ok(descriptor)) {
                    Ok(()) => ActorState::Streaming(StreamingState {
                        stream,
                        session: opening.session,
                    }),
                    Err(_) => {
                        drop(stream);
                        self.drop_session(opening.session)
                    }
                }
            }
            Err(error) => {
                let error = match error {
                    PendingOpenError::Access(error) => PlaybackManagerError::Access(error),
                    PendingOpenError::TransportStream(error) => {
                        PlaybackManagerError::TransportStream(error)
                    }
                };
                if error.retryable() {
                    self.reply_with_dormant(
                        reply,
                        Err(error),
                        opening.session,
                        DormantReason::Failed,
                    )
                } else {
                    let _ = reply.send(Err(error));
                    self.drop_session(opening.session)
                }
            }
        }
    }

    fn begin_read(&mut self, streaming: StreamingState, read: ReadCommand) -> ActorState {
        if read.reply.is_closed() {
            return ActorState::Streaming(streaming);
        }
        if read.session_id != streaming.session.id || read.stream_handle != streaming.stream.handle
        {
            let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
            return ActorState::Streaming(streaming);
        }
        ActorState::Reading(ReadingState {
            pending: Box::pin(PendingRead::new(streaming.stream)),
            session: streaming.session,
            reply: read.reply,
        })
    }

    fn finish_read(&mut self, reading: ReadingState, result: ReadCompletion) -> ActorState {
        match result {
            ReadCompletion::Chunk(bytes, stream) => match reading.reply.send(Ok(bytes)) {
                Ok(()) => ActorState::Streaming(StreamingState {
                    stream,
                    session: reading.session,
                }),
                Err(_) => {
                    drop(stream);
                    self.drop_session(reading.session)
                }
            },
            ReadCompletion::Eof => self.reply_with_dormant(
                reading.reply,
                Ok(Vec::new()),
                reading.session,
                DormantReason::Failed,
            ),
            ReadCompletion::Failed(error) => self.reply_with_dormant(
                reading.reply,
                Err(PlaybackManagerError::Read(error)),
                reading.session,
                DormantReason::Failed,
            ),
        }
    }

    fn drop_abandoned_open(&mut self, mut opening: OpeningState) -> ActorState {
        let id = opening.session.id.clone();
        drop(opening.pending);
        drop(opening.reply.take());
        drop(opening.session);
        self.retire(id);
        ActorState::Idle
    }

    fn drop_abandoned_read(&mut self, reading: ReadingState) -> ActorState {
        let id = reading.session.id.clone();
        drop(reading.pending);
        drop(reading.reply);
        drop(reading.session);
        self.retire(id);
        ActorState::Idle
    }

    fn reply_with_dormant<T>(
        &mut self,
        reply: oneshot::Sender<Result<T, PlaybackManagerError>>,
        result: Result<T, PlaybackManagerError>,
        mut session: Session,
        reason: DormantReason,
    ) -> ActorState {
        session.resume_activity = None;
        self.clear_activity_for(&session.id);
        session.dormant_reason = Some(reason);
        match reply.send(result) {
            Ok(()) => ActorState::Dormant(session),
            Err(_) => self.drop_session(session),
        }
    }

    fn reply_with_stopped(&mut self, reply: UnitReply, session: Session) -> ActorState {
        let id = session.id.clone();
        drop(session);
        self.retire(id);
        let _ = reply.send(Ok(()));
        ActorState::Idle
    }

    fn failed_dormant(&mut self, mut session: Session) -> ActorState {
        session.resume_activity = None;
        self.clear_activity_for(&session.id);
        session.dormant_reason = Some(DormantReason::Failed);
        ActorState::Dormant(session)
    }

    fn dormant(mut session: Session, reason: DormantReason) -> ActorState {
        session.dormant_reason = Some(reason);
        ActorState::Dormant(session)
    }

    fn drop_session(&mut self, session: Session) -> ActorState {
        let id = session.id.clone();
        drop(session);
        self.retire(id);
        ActorState::Idle
    }

    fn allocate_handle(&mut self) -> Option<NativeStreamHandle> {
        let sequence = self.next_handle;
        self.next_handle = sequence.checked_add(1)?;
        Some(NativeStreamHandle::from_sequence(sequence))
    }

    fn remember_suspend(&mut self, session_id: PlaybackSessionId) {
        if self.tombstones.contains(&session_id) || self.suspend_intents.contains(&session_id) {
            return;
        }
        if self.suspend_intents.len() == MAX_SUSPEND_INTENTS {
            self.suspend_intents.pop_front();
        }
        self.suspend_intents.push_back(session_id);
    }

    fn take_suspend(&mut self, session_id: &PlaybackSessionId) -> bool {
        let Some(index) = self
            .suspend_intents
            .iter()
            .position(|candidate| candidate == session_id)
        else {
            return false;
        };
        self.suspend_intents.remove(index);
        true
    }

    fn update_activity(
        &mut self,
        session: &mut Session,
        session_id: PlaybackSessionId,
        active: bool,
        preacquire_activity: bool,
    ) -> Result<(), PlaybackManagerError> {
        if session.id != session_id || (active && !self.foreground) {
            return Err(PlaybackManagerError::Cancelled);
        }
        if active {
            self.active_intent = Some(session.id.clone());
            self.resume_intent = None;
            if preacquire_activity && session.resume_activity.is_none() {
                session.resume_activity = Some(self.dependencies.core.begin_playback_activity());
            }
        } else {
            session.resume_activity = None;
            self.clear_activity_for(&session.id);
        }
        self.dependencies
            .screen_wake
            .set_active(active)
            .map_err(|()| PlaybackManagerError::Unavailable)
    }

    fn update_lifecycle(
        &mut self,
        session: Option<&mut Session>,
        lifecycle: PlaybackLifecycle,
        preacquire_resume_activity: bool,
    ) -> Result<(), PlaybackManagerError> {
        match lifecycle {
            PlaybackLifecycle::Suspended => {
                if !self.foreground {
                    if let Some(session) = session {
                        session.resume_activity = None;
                    }
                    return self
                        .dependencies
                        .screen_wake
                        .set_active(false)
                        .map_err(|()| PlaybackManagerError::Unavailable);
                }
                self.foreground = false;
                self.resume_intent = self.active_intent.take();
                if let Some(session) = session {
                    session.resume_activity = None;
                }
                self.dependencies
                    .screen_wake
                    .set_active(false)
                    .map_err(|()| PlaybackManagerError::Unavailable)
            }
            PlaybackLifecycle::Resumed => {
                let was_foreground = self.foreground;
                self.foreground = true;
                let Some(session) = session else {
                    self.resume_intent = None;
                    return self
                        .dependencies
                        .screen_wake
                        .set_active(false)
                        .map_err(|()| PlaybackManagerError::Unavailable);
                };
                if was_foreground || self.resume_intent.as_ref() != Some(&session.id) {
                    return Ok(());
                }
                self.active_intent = Some(session.id.clone());
                if preacquire_resume_activity && session.resume_activity.is_none() {
                    session.resume_activity =
                        Some(self.dependencies.core.begin_playback_activity());
                }
                self.dependencies
                    .screen_wake
                    .set_active(true)
                    .map_err(|()| PlaybackManagerError::Unavailable)
            }
        }
    }

    fn clear_activity_for(&mut self, session_id: &PlaybackSessionId) {
        let mut owned = false;
        if self.active_intent.as_ref() == Some(session_id) {
            self.active_intent = None;
            owned = true;
        }
        if self.resume_intent.as_ref() == Some(session_id) {
            self.resume_intent = None;
            owned = true;
        }
        if owned {
            let _ = self.dependencies.screen_wake.set_active(false);
        }
    }

    fn retire(&mut self, session_id: PlaybackSessionId) {
        if let Some(index) = self
            .suspend_intents
            .iter()
            .position(|candidate| candidate == &session_id)
        {
            self.suspend_intents.remove(index);
        }
        if self.tombstones.contains(&session_id) {
            self.clear_activity_for(&session_id);
            return;
        }
        if self.tombstones.len() == MAX_STOP_TOMBSTONES {
            self.tombstones.pop_front();
        }
        self.clear_activity_for(&session_id);
        self.tombstones.push_back(session_id);
    }
}

enum ActorCommand {
    Control(ControlCommand),
    Read(ReadCommand),
}

enum ActorState {
    Idle,
    Opening(OpeningState),
    Streaming(StreamingState),
    Reading(ReadingState),
    Dormant(Session),
    OpeningMpv(MpvOpeningState),
    MpvPlaying(MpvPlayingState),
    Exited,
}

struct OpeningState {
    // The upstream future is dropped before the pinned Session source.
    pending: Pin<Box<PendingOpen>>,
    session: Session,
    stream_handle: NativeStreamHandle,
    reply: Option<DescriptorReply>,
}

struct StreamingState {
    // Provider resources always drop before their pinned Session source.
    stream: StreamInstance,
    session: Session,
}

struct ReadingState {
    // The provider body is owned here so cancelling this state cancels `next`.
    pending: Pin<Box<PendingRead>>,
    session: Session,
    reply: oneshot::Sender<Result<Vec<u8>, PlaybackManagerError>>,
}

struct MpvOpeningState {
    pending: MpvLaunchFuture,
    session: Session,
    reply: Option<MpvReply>,
}

struct MpvPlayingState {
    process: MpvProcess,
    session: Session,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DormantReason {
    Failed,
    Suspended,
}

/// Pins one resolved source independently of its replaceable transport.
struct Session {
    id: PlaybackSessionId,
    channel_id: ChannelId,
    source: Arc<ResolvedPlaybackSource>,
    current_track: Option<AudioTrackId>,
    last_stream_handle: Option<NativeStreamHandle>,
    resume_activity: Option<PlaybackActivityLease>,
    dormant_reason: Option<DormantReason>,
}

struct StreamInstance {
    // Field order ensures provider resources drop before the activity lease.
    body: PlaybackByteStream,
    remainder: Bytes,
    _activity: PlaybackActivityLease,
    handle: NativeStreamHandle,
}

struct PendingOpen {
    // Field order ensures a cancelled request drops before its activity lease.
    future: SelectedOpenFuture,
    activity: Option<PlaybackActivityLease>,
}

impl PendingOpen {
    fn new(future: SelectedOpenFuture, activity: PlaybackActivityLease) -> Self {
        Self {
            future,
            activity: Some(activity),
        }
    }
}

impl Future for PendingOpen {
    type Output = Result<(OpenedPlayback, PlaybackActivityLease), PendingOpenError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let result = ready!(self.future.as_mut().poll(context));
        match result {
            Ok(body) => Poll::Ready(Ok((
                body,
                self.activity.take().expect("pending open owns activity"),
            ))),
            Err(error) => {
                drop(self.activity.take());
                Poll::Ready(Err(error))
            }
        }
    }
}

struct OpenedPlayback {
    opened: PreparedPlaybackTransport,
    preference_status: Option<PreferenceStatus>,
}

struct PreparedPlaybackTransport {
    body: PlaybackByteStream,
    tracks: Vec<AudioTrack>,
    selection: AudioSelection,
}

enum PendingOpenError {
    Access(PlaybackAccessError),
    TransportStream(TransportStreamError),
}

struct PendingRead {
    stream: Option<StreamInstance>,
}

impl PendingRead {
    fn new(stream: StreamInstance) -> Self {
        Self {
            stream: Some(stream),
        }
    }
}

impl Future for PendingRead {
    type Output = ReadCompletion;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let stream = self.stream.as_mut().expect("pending read owns stream");
        if !stream.remainder.is_empty() {
            let take = stream.remainder.len().min(MAX_NATIVE_PULL_BYTES);
            let bytes = stream.remainder.split_to(take).to_vec();
            let stream = self.stream.take().expect("pending read owns stream");
            return Poll::Ready(ReadCompletion::Chunk(bytes, stream));
        }

        match ready!(stream.body.next().poll_unpin(context)) {
            Some(Ok(bytes)) if bytes.is_empty() => {
                context.waker().wake_by_ref();
                Poll::Pending
            }
            Some(Ok(mut bytes)) => {
                let take = bytes.len().min(MAX_NATIVE_PULL_BYTES);
                let chunk = bytes.split_to(take).to_vec();
                stream.remainder = bytes;
                let stream = self.stream.take().expect("pending read owns stream");
                Poll::Ready(ReadCompletion::Chunk(chunk, stream))
            }
            Some(Err(error)) => {
                drop(self.stream.take());
                Poll::Ready(ReadCompletion::Failed(error))
            }
            None => {
                drop(self.stream.take());
                Poll::Ready(ReadCompletion::Eof)
            }
        }
    }
}

enum ReadCompletion {
    Chunk(Vec<u8>, StreamInstance),
    Eof,
    Failed(PlaybackReadError),
}

#[cfg(test)]
#[path = "playback/tests.rs"]
mod tests;
