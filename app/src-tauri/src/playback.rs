use std::{
    collections::VecDeque,
    fmt::{self, Debug, Formatter},
    future::Future,
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

const COMMAND_CAPACITY: usize = 16;
const READ_CAPACITY: usize = 16;
const MAX_STOP_TOMBSTONES: usize = 64;
const MAX_NATIVE_PULL_BYTES: usize = 64 * 1024;
const SESSION_ID_PREFIX: &str = "play1_";
const SESSION_ID_NONCE_HEX_BYTES: usize = 32;
const MAX_SESSION_ID_BYTES: usize = 64;
const STREAM_HANDLE_PREFIX: &str = "stream1_";
const STREAM_HANDLE_HEX_BYTES: usize = 16;

type AccessOpenFuture =
    Pin<Box<dyn Future<Output = Result<PlaybackByteStream, PlaybackAccessError>> + Send + 'static>>;

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

/// A client-created identifier that correlates reordered start and stop invokes.
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

/// A Rust-generated handle for one native provider stream.
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
}

impl StartedPlayback {
    pub(crate) fn session_id(&self) -> &PlaybackSessionId {
        &self.session_id
    }

    pub(crate) fn stream_handle(&self) -> &NativeStreamHandle {
        &self.stream_handle
    }
}

impl Debug for StartedPlayback {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartedPlayback")
            .field("session_id", &self.session_id)
            .field("stream_handle", &self.stream_handle)
            .finish()
    }
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
    #[error("the playback operation was cancelled")]
    Cancelled,
    #[error("native playback is unavailable")]
    Unavailable,
}

/// Owns the only native provider connection and serializes all stream lifecycle
/// transitions behind bounded, cancellation-aware queues.
pub(crate) struct PlaybackManager {
    controls: mpsc::Sender<ControlCommand>,
    reads: mpsc::Sender<ReadCommand>,
    actor: JoinHandle<()>,
}

impl PlaybackManager {
    pub(crate) fn new(core: Arc<SparrowCore>, access: HttpPlaybackAccess) -> Self {
        Self::with_access(core, Arc::new(access))
    }

    fn with_access(core: Arc<SparrowCore>, access: Arc<dyn NativePlaybackAccess>) -> Self {
        let (controls, control_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (reads, read_receiver) = mpsc::channel(READ_CAPACITY);
        let actor =
            tokio::spawn(PlaybackActor::new(core, access, control_receiver, read_receiver).run());
        Self {
            controls,
            reads,
            actor,
        }
    }

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

    pub(crate) async fn stop(
        &self,
        session_id: PlaybackSessionId,
    ) -> Result<(), PlaybackManagerError> {
        let (reply, response) = oneshot::channel();
        self.controls
            .send(ControlCommand::Stop { session_id, reply })
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
    Start {
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
        reply: oneshot::Sender<Result<StartedPlayback, PlaybackManagerError>>,
    },
    Stop {
        session_id: PlaybackSessionId,
        reply: oneshot::Sender<Result<(), PlaybackManagerError>>,
    },
}

struct ReadCommand {
    session_id: PlaybackSessionId,
    stream_handle: NativeStreamHandle,
    reply: oneshot::Sender<Result<Vec<u8>, PlaybackManagerError>>,
}

struct PlaybackActor {
    core: Arc<SparrowCore>,
    access: Arc<dyn NativePlaybackAccess>,
    controls: mpsc::Receiver<ControlCommand>,
    reads: mpsc::Receiver<ReadCommand>,
    tombstones: VecDeque<PlaybackSessionId>,
    next_handle: u64,
}

impl PlaybackActor {
    fn new(
        core: Arc<SparrowCore>,
        access: Arc<dyn NativePlaybackAccess>,
        controls: mpsc::Receiver<ControlCommand>,
        reads: mpsc::Receiver<ReadCommand>,
    ) -> Self {
        Self {
            core,
            access,
            controls,
            reads,
            tombstones: VecDeque::with_capacity(MAX_STOP_TOMBSTONES),
            next_handle: 1,
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
                            self.retire(opening.session_id.clone());
                            drop(opening);
                            ActorState::Idle
                        }
                        result = &mut opening.pending => {
                            self.finish_open(opening, result)
                        }
                        read = self.reads.recv() => match read {
                            Some(read) => {
                                let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
                                ActorState::Opening(opening)
                            }
                            None => return,
                        },
                    }
                }
                ActorState::Active(active) => {
                    tokio::select! {
                        biased;
                        command = self.controls.recv() => match command {
                            Some(command) => self.control_active(active, command),
                            None => return,
                        },
                        read = self.reads.recv() => match read {
                            Some(read) => self.begin_read(active, read),
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
                        () = reading.reply.closed() => {
                            self.abandon_read(reading)
                        }
                        result = &mut reading.pending => {
                            self.finish_read(reading, result)
                        }
                        read = self.reads.recv() => match read {
                            Some(read) => {
                                let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
                                ActorState::Reading(reading)
                            }
                            None => return,
                        },
                    }
                }
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
            ControlCommand::Stop { session_id, reply } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Idle
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
            } if self.tombstones.contains(&session_id) => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::Start {
                session_id,
                channel_id: _,
                reply,
            } if session_id == opening.session_id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Opening(opening)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                self.retire(opening.session_id.clone());
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Stop { session_id, reply } if session_id == opening.session_id => {
                self.retire(session_id);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::Stop { session_id, reply } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Opening(opening)
            }
        }
    }

    fn control_active(&mut self, active: ActiveState, command: ControlCommand) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => ActorState::Active(active),
            ControlCommand::Start {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Active(active)
            }
            ControlCommand::Start {
                session_id,
                channel_id: _,
                reply,
            } if session_id == active.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Active(active)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                self.retire(active.session.id.clone());
                drop(active);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Stop { session_id, reply } if session_id == active.session.id => {
                self.retire(session_id);
                drop(active);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::Stop { session_id, reply } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Active(active)
            }
        }
    }

    fn control_reading(&mut self, reading: ReadingState, command: ControlCommand) -> ActorState {
        match command {
            ControlCommand::Start { reply, .. } if reply.is_closed() => {
                ActorState::Reading(reading)
            }
            ControlCommand::Start {
                session_id, reply, ..
            } if self.tombstones.contains(&session_id) => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::Start {
                session_id,
                channel_id: _,
                reply,
            } if session_id == reading.session.id => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Reading(reading)
            }
            ControlCommand::Start {
                session_id,
                channel_id,
                reply,
            } => {
                self.retire(reading.session.id.clone());
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                drop(reading.pending);
                drop(reading.session);
                self.begin_start(session_id, channel_id, reply)
            }
            ControlCommand::Stop { session_id, reply } if session_id == reading.session.id => {
                self.retire(session_id);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                drop(reading.pending);
                drop(reading.session);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::Stop { session_id, reply } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Reading(reading)
            }
        }
    }

    fn begin_start(
        &mut self,
        session_id: PlaybackSessionId,
        channel_id: ChannelId,
        reply: oneshot::Sender<Result<StartedPlayback, PlaybackManagerError>>,
    ) -> ActorState {
        if self.tombstones.contains(&session_id) {
            let _ = reply.send(Err(PlaybackManagerError::Cancelled));
            return ActorState::Idle;
        }

        let activity = self.core.begin_playback_activity();
        let source = match self.core.resolve_playback(&channel_id) {
            Ok(source) => Arc::new(source),
            Err(error) => {
                drop(activity);
                let _ = reply.send(Err(PlaybackManagerError::Core(error)));
                return ActorState::Idle;
            }
        };
        let stream_handle = match self.allocate_handle() {
            Some(stream_handle) => stream_handle,
            None => {
                drop(source);
                drop(activity);
                let _ = reply.send(Err(PlaybackManagerError::Unavailable));
                return ActorState::Idle;
            }
        };
        let pending = PendingOpen::new(self.access.open(Arc::clone(&source)), activity);
        ActorState::Opening(OpeningState {
            pending: Box::pin(pending),
            session_id,
            stream_handle,
            source,
            reply: Some(reply),
        })
    }

    fn finish_open(
        &mut self,
        mut opening: OpeningState,
        result: Result<(PlaybackByteStream, PlaybackActivityLease), PlaybackAccessError>,
    ) -> ActorState {
        let reply = opening.reply.take().expect("opening reply exists");
        match result {
            Ok((body, activity)) => {
                let descriptor = StartedPlayback {
                    session_id: opening.session_id.clone(),
                    stream_handle: opening.stream_handle.clone(),
                };
                let session = Session {
                    id: opening.session_id,
                    _source: opening.source,
                };
                let stream = StreamStatus::Open(StreamInstance {
                    body,
                    remainder: Bytes::new(),
                    _activity: activity,
                    handle: opening.stream_handle,
                });
                match reply.send(Ok(descriptor)) {
                    Ok(()) => ActorState::Active(ActiveState { session, stream }),
                    Err(_) => {
                        self.retire(session.id.clone());
                        drop(stream);
                        drop(session);
                        ActorState::Idle
                    }
                }
            }
            Err(error) => {
                let _ = reply.send(Err(PlaybackManagerError::Access(error)));
                ActorState::Idle
            }
        }
    }

    fn begin_read(&mut self, active: ActiveState, read: ReadCommand) -> ActorState {
        if read.reply.is_closed() {
            return ActorState::Active(active);
        }
        if read.session_id != active.session.id || read.stream_handle != *active.stream.handle() {
            let _ = read.reply.send(Err(PlaybackManagerError::Cancelled));
            return ActorState::Active(active);
        }
        match active.stream {
            StreamStatus::Open(stream) => ActorState::Reading(ReadingState {
                pending: Box::pin(PendingRead::new(stream)),
                session: active.session,
                reply: read.reply,
            }),
            StreamStatus::Ended(handle) => {
                let _ = read.reply.send(Ok(Vec::new()));
                ActorState::Active(ActiveState {
                    session: active.session,
                    stream: StreamStatus::Ended(handle),
                })
            }
            StreamStatus::Failed(handle, error) => {
                let _ = read.reply.send(Err(PlaybackManagerError::Read(error)));
                ActorState::Active(ActiveState {
                    session: active.session,
                    stream: StreamStatus::Failed(handle, error),
                })
            }
        }
    }

    fn finish_read(&mut self, reading: ReadingState, result: ReadCompletion) -> ActorState {
        match result {
            ReadCompletion::Chunk(bytes, stream) => match reading.reply.send(Ok(bytes)) {
                Ok(()) => ActorState::Active(ActiveState {
                    session: reading.session,
                    stream: StreamStatus::Open(stream),
                }),
                Err(_) => {
                    drop(stream);
                    self.drop_abandoned_session(reading.session)
                }
            },
            ReadCompletion::Eof(handle) => match reading.reply.send(Ok(Vec::new())) {
                Ok(()) => ActorState::Active(ActiveState {
                    session: reading.session,
                    stream: StreamStatus::Ended(handle),
                }),
                Err(_) => self.drop_abandoned_session(reading.session),
            },
            ReadCompletion::Failed(handle, error) => {
                match reading.reply.send(Err(PlaybackManagerError::Read(error))) {
                    Ok(()) => ActorState::Active(ActiveState {
                        session: reading.session,
                        stream: StreamStatus::Failed(handle, error),
                    }),
                    Err(_) => self.drop_abandoned_session(reading.session),
                }
            }
        }
    }

    fn abandon_read(&mut self, reading: ReadingState) -> ActorState {
        drop(reading.pending);
        self.drop_abandoned_session(reading.session)
    }

    fn drop_abandoned_session(&mut self, session: Session) -> ActorState {
        self.retire(session.id.clone());
        drop(session);
        ActorState::Idle
    }

    fn allocate_handle(&mut self) -> Option<NativeStreamHandle> {
        let sequence = self.next_handle;
        self.next_handle = sequence.checked_add(1)?;
        Some(NativeStreamHandle::from_sequence(sequence))
    }

    fn retire(&mut self, session_id: PlaybackSessionId) {
        if self.tombstones.contains(&session_id) {
            return;
        }
        if self.tombstones.len() == MAX_STOP_TOMBSTONES {
            self.tombstones.pop_front();
        }
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
    Active(ActiveState),
    Reading(ReadingState),
}

struct OpeningState {
    // The upstream future is dropped before the pinned source on cancellation.
    pending: Pin<Box<PendingOpen>>,
    session_id: PlaybackSessionId,
    stream_handle: NativeStreamHandle,
    source: Arc<ResolvedPlaybackSource>,
    reply: Option<oneshot::Sender<Result<StartedPlayback, PlaybackManagerError>>>,
}

struct ActiveState {
    // Provider resources always drop before their pinned Session source.
    stream: StreamStatus,
    session: Session,
}

struct ReadingState {
    // The provider body is owned here so cancelling this state cancels `next`.
    pending: Pin<Box<PendingRead>>,
    session: Session,
    reply: oneshot::Sender<Result<Vec<u8>, PlaybackManagerError>>,
}

/// Pins one resolved source independently of its replaceable transport. #28 can
/// replace `StreamStatus` without resolving the catalog again.
struct Session {
    id: PlaybackSessionId,
    _source: Arc<ResolvedPlaybackSource>,
}

enum StreamStatus {
    Open(StreamInstance),
    Ended(NativeStreamHandle),
    Failed(NativeStreamHandle, PlaybackReadError),
}

impl StreamStatus {
    fn handle(&self) -> &NativeStreamHandle {
        match self {
            Self::Open(stream) => &stream.handle,
            Self::Ended(handle) | Self::Failed(handle, _) => handle,
        }
    }
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
    future: AccessOpenFuture,
    activity: Option<PlaybackActivityLease>,
}

impl PendingOpen {
    fn new(future: AccessOpenFuture, activity: PlaybackActivityLease) -> Self {
        Self {
            future,
            activity: Some(activity),
        }
    }
}

impl Future for PendingOpen {
    type Output = Result<(PlaybackByteStream, PlaybackActivityLease), PlaybackAccessError>;

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
                let stream = self.stream.take().expect("pending read owns stream");
                let handle = stream.handle.clone();
                drop(stream);
                Poll::Ready(ReadCompletion::Failed(handle, error))
            }
            None => {
                let stream = self.stream.take().expect("pending read owns stream");
                let handle = stream.handle.clone();
                drop(stream);
                Poll::Ready(ReadCompletion::Eof(handle))
            }
        }
    }
}

enum ReadCompletion {
    Chunk(Vec<u8>, StreamInstance),
    Eof(NativeStreamHandle),
    Failed(NativeStreamHandle, PlaybackReadError),
}

#[cfg(test)]
mod tests;
