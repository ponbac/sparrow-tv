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
const MAX_SUSPEND_INTENTS: usize = 64;
const MAX_NATIVE_PULL_BYTES: usize = 64 * 1024;
const SESSION_ID_PREFIX: &str = "play1_";
const SESSION_ID_NONCE_HEX_BYTES: usize = 32;
const MAX_SESSION_ID_BYTES: usize = 64;
const STREAM_HANDLE_PREFIX: &str = "stream1_";
const STREAM_HANDLE_HEX_BYTES: usize = 16;

type AccessOpenFuture =
    Pin<Box<dyn Future<Output = Result<PlaybackByteStream, PlaybackAccessError>> + Send + 'static>>;
type DescriptorReply = oneshot::Sender<Result<StartedPlayback, PlaybackManagerError>>;
type UnitReply = oneshot::Sender<Result<(), PlaybackManagerError>>;

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
        reply: DescriptorReply,
    },
    Suspend {
        session_id: PlaybackSessionId,
        reply: UnitReply,
    },
    Reopen {
        session_id: PlaybackSessionId,
        reply: DescriptorReply,
    },
    Stop {
        session_id: PlaybackSessionId,
        reply: UnitReply,
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
    suspend_intents: VecDeque<PlaybackSessionId>,
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
            suspend_intents: VecDeque::with_capacity(MAX_SUSPEND_INTENTS),
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
            ControlCommand::Reopen { reply, .. } => {
                let _ = reply.send(Err(PlaybackManagerError::Cancelled));
                ActorState::Idle
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
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                let _ = reply.send(Ok(()));
                ActorState::Dormant(opening.session)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
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
            ControlCommand::Stop { session_id, reply } if session_id == opening.session.id => {
                drop(opening.pending);
                let _ = opening
                    .reply
                    .take()
                    .expect("opening reply exists")
                    .send(Err(PlaybackManagerError::Cancelled));
                drop(opening.session);
                self.retire(session_id);
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

    fn control_streaming(
        &mut self,
        streaming: StreamingState,
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
                drop(streaming.stream);
                let _ = reply.send(Ok(()));
                ActorState::Dormant(streaming.session)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
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
            ControlCommand::Stop { session_id, reply } if session_id == streaming.session.id => {
                drop(streaming.stream);
                drop(streaming.session);
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::Stop { session_id, reply } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Streaming(streaming)
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
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                let _ = reply.send(Ok(()));
                ActorState::Dormant(reading.session)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
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
            ControlCommand::Stop { session_id, reply } if session_id == reading.session.id => {
                drop(reading.pending);
                let _ = reading.reply.send(Err(PlaybackManagerError::Cancelled));
                drop(reading.session);
                self.retire(session_id);
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

    fn control_dormant(&mut self, session: Session, command: ControlCommand) -> ActorState {
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
                let _ = reply.send(Ok(()));
                ActorState::Dormant(session)
            }
            ControlCommand::Suspend { session_id, reply } => {
                self.remember_suspend(session_id);
                let _ = reply.send(Ok(()));
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
            ControlCommand::Stop { session_id, reply } if session_id == session.id => {
                drop(session);
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Idle
            }
            ControlCommand::Stop { session_id, reply } => {
                self.retire(session_id);
                let _ = reply.send(Ok(()));
                ActorState::Dormant(session)
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

        let activity = self.core.begin_playback_activity();
        let source = match self.core.resolve_playback(&channel_id) {
            Ok(source) => Arc::new(source),
            Err(error) => {
                drop(activity);
                let _ = reply.send(Err(PlaybackManagerError::Core(error)));
                return ActorState::Idle;
            }
        };
        let session = Session {
            id: session_id,
            source,
        };
        if self.take_suspend(&session.id) {
            drop(activity);
            return self.reply_with_dormant(reply, Err(PlaybackManagerError::Cancelled), session);
        }
        self.begin_open(session, activity, reply)
    }

    fn begin_reopen(&mut self, session: Session, reply: DescriptorReply) -> ActorState {
        let activity = self.core.begin_playback_activity();
        self.begin_open(session, activity, reply)
    }

    fn begin_open(
        &mut self,
        session: Session,
        activity: PlaybackActivityLease,
        reply: DescriptorReply,
    ) -> ActorState {
        let stream_handle = match self.allocate_handle() {
            Some(stream_handle) => stream_handle,
            None => {
                drop(activity);
                return self.reply_with_dormant(
                    reply,
                    Err(PlaybackManagerError::Unavailable),
                    session,
                );
            }
        };
        let pending = PendingOpen::new(self.access.open(Arc::clone(&session.source)), activity);
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
        result: Result<(PlaybackByteStream, PlaybackActivityLease), PlaybackAccessError>,
    ) -> ActorState {
        let reply = opening.reply.take().expect("opening reply exists");
        match result {
            Ok((body, activity)) => {
                let descriptor = StartedPlayback {
                    session_id: opening.session.id.clone(),
                    stream_handle: opening.stream_handle.clone(),
                };
                let stream = StreamInstance {
                    body,
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
            Err(error) => self.reply_with_dormant(
                reply,
                Err(PlaybackManagerError::Access(error)),
                opening.session,
            ),
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
            ReadCompletion::Eof => {
                self.reply_with_dormant(reading.reply, Ok(Vec::new()), reading.session)
            }
            ReadCompletion::Failed(error) => self.reply_with_dormant(
                reading.reply,
                Err(PlaybackManagerError::Read(error)),
                reading.session,
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
        session: Session,
    ) -> ActorState {
        match reply.send(result) {
            Ok(()) => ActorState::Dormant(session),
            Err(_) => self.drop_session(session),
        }
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

    fn retire(&mut self, session_id: PlaybackSessionId) {
        if let Some(index) = self
            .suspend_intents
            .iter()
            .position(|candidate| candidate == &session_id)
        {
            self.suspend_intents.remove(index);
        }
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
    Streaming(StreamingState),
    Reading(ReadingState),
    Dormant(Session),
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

/// Pins one resolved source independently of its replaceable transport.
struct Session {
    id: PlaybackSessionId,
    source: Arc<ResolvedPlaybackSource>,
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
