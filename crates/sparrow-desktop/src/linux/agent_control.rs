//! The Linux desktop's opt-in local Agent Control protocol.

use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sparrow_core::{CatalogAccess, ChannelSummary, PageLimit, PageRequest, SearchTerm};
use sparrow_guide_ui::{Catalog, Phase};
use sparrow_installed_playback::tv::{
    TvPlaybackCommand, TvPlaybackPhase, TvPlaybackSnapshot, TvTuneTarget,
};
use sparrow_tv_guide::normalize_preference_title;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use super::playback::DesktopPlayback;

const MAX_REQUEST_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SOCKET_PATH_BYTES: usize = 100;
const MAX_CONNECTIONS: usize = 8;
const CHANNEL_LIMIT: u16 = 40;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(12);

struct Config {
    socket: PathBuf,
    token: String,
}

impl Config {
    fn from_environment() -> Option<Self> {
        let socket = std::env::var_os("SPARROW_AGENT_SOCKET")?.into();
        let token = std::env::var("SPARROW_AGENT_TOKEN").ok()?;
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let config = Self { socket, token };
        if config.valid(runtime_dir.as_deref()) {
            Some(config)
        } else {
            eprintln!("sparrow-agent-control: unavailable");
            None
        }
    }

    fn valid(&self, runtime_dir: Option<&Path>) -> bool {
        let allowed = |path: &Path| {
            path.starts_with("/tmp")
                || runtime_dir.is_some_and(|dir| dir.is_absolute() && path.starts_with(dir))
        };
        self.socket.is_absolute()
            && self.socket.as_os_str().len() <= MAX_SOCKET_PATH_BYTES
            && !self
                .socket
                .components()
                .any(|part| part == Component::ParentDir)
            && allowed(&self.socket)
            && self
                .socket
                .parent()
                .and_then(|parent| parent.canonicalize().ok())
                .is_some_and(|parent| allowed(&parent))
            && (16..=128).contains(&self.token.len())
            && self.token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    token: String,
    request: Request,
}

#[derive(Deserialize)]
#[serde(tag = "_tag", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Ping {},
    Search { term: String },
    Tune { term: String },
    Snapshot {},
    Stop {},
}

#[derive(Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
enum Failure {
    Unauthenticated,
    InvalidRequest,
    Unavailable,
    CatalogNotReady,
    SearchIncomplete,
    ChannelNotFound,
    AmbiguousChannel { names: Vec<String> },
    Timeout,
    CleanupFailed,
}

type Response = Result<Value, Failure>;

pub(super) struct Server {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Server {
    pub(super) fn start(
        catalog: Arc<Catalog>,
        playback: Arc<DesktopPlayback>,
        ui: mpsc::Sender<Intent>,
    ) -> Option<Self> {
        let config = Config::from_environment()?;
        let runtime = catalog.runtime()?.handle().clone();
        let (shutdown, closing) = oneshot::channel();
        let task = runtime.spawn(async move {
            if serve(config, Dispatcher::new(catalog, playback, ui), closing)
                .await
                .is_err()
            {
                eprintln!("sparrow-agent-control: unavailable");
            }
        });
        Some(Self { shutdown, task })
    }

    pub(super) async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.task.await;
    }
}

/// Holds the bound socket's identity so shutdown cannot unlink a replacement.
struct SocketFile {
    path: PathBuf,
    inode: u64,
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|file| file.file_type().is_socket() && file.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn serve(
    config: Config,
    dispatcher: Dispatcher,
    mut shutdown: oneshot::Receiver<()>,
) -> io::Result<()> {
    let (listener, _socket_file) = bind(config.socket).await?;
    eprintln!("sparrow-agent-control: listening");
    let dispatcher = Arc::new(dispatcher);
    let token: Arc<str> = config.token.into();
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            _ = &mut shutdown => break Ok(()),
            _ = connections.join_next(), if !connections.is_empty() => {}
            connection = listener.accept() => {
                let (stream, _) = match connection {
                    Ok(connection) => connection,
                    Err(error) => break Err(error),
                };
                if connections.len() < MAX_CONNECTIONS {
                    let dispatcher = Arc::clone(&dispatcher);
                    let token = Arc::clone(&token);
                    connections.spawn(async move { handle(stream, &token, &dispatcher).await });
                }
            }
        }
    };
    connections.shutdown().await;
    result
}

async fn bind(path: PathBuf) -> io::Result<(UnixListener, SocketFile)> {
    match std::fs::symlink_metadata(&path) {
        Ok(file) if file.file_type().is_socket() => {
            // Reclaim a stale socket left by a crashed process. A live socket
            // and filesystem entries of other types stay under their owner.
            match timeout(WRITE_TIMEOUT, UnixStream::connect(&path)).await {
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    if std::fs::symlink_metadata(&path)?.ino() == file.ino() {
                        std::fs::remove_file(&path)?;
                    }
                }
                _ => return Err(io::ErrorKind::AddrInUse.into()),
            }
        }
        Ok(_) => return Err(io::ErrorKind::AddrInUse.into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(&path)?;
    let socket_file = SocketFile {
        inode: std::fs::symlink_metadata(&path)?.ino(),
        path,
    };
    std::fs::set_permissions(&socket_file.path, std::fs::Permissions::from_mode(0o600))?;
    Ok((listener, socket_file))
}

async fn handle(stream: UnixStream, token: &str, dispatcher: &Dispatcher) {
    let mut stream = BufReader::new(stream);
    let result = match timeout(READ_TIMEOUT, read_request(&mut stream, token)).await {
        Ok(Ok(request)) => {
            let deadline = Instant::now() + DISPATCH_TIMEOUT;
            tokio::select! {
                biased;
                // EOF or extra input abandons the pending request.
                _ = stream.read_u8() => return,
                response = timeout(DISPATCH_TIMEOUT, dispatcher.execute(request, deadline)) => {
                    response.unwrap_or(Err(Failure::Timeout))
                }
            }
        }
        Ok(Err(failure)) => Err(failure),
        Err(_) => Err(Failure::InvalidRequest),
    };
    let response = match result {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error }),
    };
    let Ok(mut payload) = serde_json::to_vec(&response) else {
        return;
    };
    if payload.len() > MAX_RESPONSE_BYTES {
        payload = br#"{"ok":false,"error":{"_tag":"unavailable"}}"#.to_vec();
    }
    payload.push(b'\n');
    let _ = timeout(WRITE_TIMEOUT, stream.write_all(&payload)).await;
}

async fn read_request(stream: &mut BufReader<UnixStream>, token: &str) -> Result<Request, Failure> {
    let mut bytes = Vec::with_capacity(MAX_REQUEST_BYTES);
    for _ in 0..MAX_REQUEST_BYTES {
        let byte = stream
            .read_u8()
            .await
            .map_err(|_| Failure::InvalidRequest)?;
        if byte == b'\n' {
            let envelope: Envelope =
                serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidRequest)?;
            let supplied = envelope.token.as_bytes();
            let expected = token.as_bytes();
            if supplied.len() != expected.len()
                || supplied
                    .iter()
                    .zip(expected)
                    .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
                    != 0
            {
                return Err(Failure::Unauthenticated);
            }
            return Ok(envelope.request);
        }
        bytes.push(byte);
    }
    Err(Failure::InvalidRequest)
}

/// Only the catalog read runs off the main thread. An abandoned request cannot
/// commit a queued UI intent, and the Guide supersedes its pending tune reads
/// when it accepts an external action.
pub(super) struct Intent {
    command: TvPlaybackCommand,
    committed: oneshot::Sender<bool>,
    deadline: Instant,
    cancellation: watch::Receiver<()>,
}

impl Intent {
    pub(super) fn commit(self, apply: impl FnOnce(TvPlaybackCommand) -> bool) {
        if !self.committed.is_closed()
            && Instant::now() < self.deadline
            && self.cancellation.has_changed().is_ok()
        {
            let _ = self.committed.send(apply(self.command));
        }
    }
}

struct Dispatcher {
    catalog: Arc<Catalog>,
    playback: Arc<DesktopPlayback>,
    ui: mpsc::Sender<Intent>,
    mutation: Mutex<Option<(bool, watch::Sender<()>)>>,
}

impl Dispatcher {
    fn new(
        catalog: Arc<Catalog>,
        playback: Arc<DesktopPlayback>,
        ui: mpsc::Sender<Intent>,
    ) -> Self {
        Self {
            catalog,
            playback,
            ui,
            mutation: Mutex::default(),
        }
    }

    async fn execute(&self, request: Request, deadline: Instant) -> Response {
        match request {
            Request::Ping {} => Ok(json!({ "_tag": "pong", "ready": self.ready() })),
            Request::Search { term } => {
                let (term, _) = parse_term(term)?;
                let channels = self
                    .query(move |core| {
                        let page = core
                            .search_channels(term, first_page())
                            .map_err(|_| Failure::CatalogNotReady)?;
                        Ok(page.items().iter().map(|channel| {
                        json!({ "name": channel.name(), "group": channel.group() })
                    }).collect::<Vec<_>>())
                    })
                    .await?;
                Ok(json!({ "_tag": "channels", "channels": channels }))
            }
            Request::Tune { term } => {
                let (term, canonical) = parse_term(term)?;
                let mut mutation = self.begin_mutation(false)?;
                let cancellation = mutation.cancellation.clone();
                tokio::select! {
                    biased;
                    _ = mutation.cancellation.changed() => Err(Failure::Timeout),
                    result = self.tune(term, canonical, deadline, cancellation) => result,
                }
            }
            Request::Snapshot {} => self.snapshot().await,
            Request::Stop {} => {
                let mutation = self.begin_mutation(true)?;
                if self.catalog.runner().is_some() {
                    let (stopped, confirmation) = oneshot::channel();
                    self.commit(
                        TvPlaybackCommand::StopConfirmed(stopped),
                        deadline,
                        mutation.cancellation.clone(),
                    )
                    .await?;
                    confirmation
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .ok_or(Failure::CleanupFailed)?;
                }
                Ok(json!({ "_tag": "stopped" }))
            }
        }
    }

    fn ready(&self) -> bool {
        let (catalog, _) = self.catalog.diagnostics();
        catalog.active && catalog.phase == Phase::Ready && self.catalog.runner().is_some()
    }

    async fn query<T: Send + 'static>(
        &self,
        read: impl FnOnce(&CatalogAccess) -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, Failure> {
        self.catalog
            .playback_query(read)
            .ok_or(Failure::CatalogNotReady)?
            .await
            .ok()
            .flatten()
            .ok_or(Failure::Unavailable)?
    }

    async fn tune(
        &self,
        term: SearchTerm,
        canonical: String,
        deadline: Instant,
        cancellation: watch::Receiver<()>,
    ) -> Response {
        let (target, name) = self
            .query(move |core| {
                let page = core
                    .search_channels(term, first_page())
                    .map_err(|_| Failure::CatalogNotReady)?;
                if page.next().is_some() {
                    return Err(Failure::SearchIncomplete);
                }
                let channel = select_channel(page.items(), &canonical)?;
                let family = core
                    .channel_family(page.generation(), channel.number())
                    .ok()
                    .flatten()
                    .ok_or(Failure::CatalogNotReady)?;
                Ok((
                    TvTuneTarget::from_family(&family, channel),
                    channel.name().to_owned(),
                ))
            })
            .await?;
        self.commit(
            TvPlaybackCommand::Tune {
                target,
                repeat: false,
            },
            deadline,
            cancellation,
        )
        .await?;
        Ok(json!({ "_tag": "tuned", "name": name }))
    }

    async fn commit(
        &self,
        command: TvPlaybackCommand,
        deadline: Instant,
        cancellation: watch::Receiver<()>,
    ) -> Result<(), Failure> {
        let (committed, confirmation) = oneshot::channel();
        self.ui
            .try_send(Intent {
                command,
                committed,
                deadline,
                cancellation,
            })
            .map_err(|_| Failure::Unavailable)?;
        match confirmation.await {
            Ok(true) => Ok(()),
            Ok(false) => Err(Failure::Unavailable),
            Err(_) => Err(Failure::Timeout),
        }
    }

    async fn snapshot(&self) -> Response {
        let idle = || json!({ "_tag": "snapshot", "channelName": null, "diagnostics": null });
        let Some(runner) = self.catalog.runner() else {
            return Ok(idle());
        };
        let updates = runner.subscribe();
        loop {
            let state = updates.borrow().clone();
            let Some(target) = &state.selected else {
                return Ok(idle());
            };
            let channel = target.channel.clone();
            let name = self
                .query(move |core| {
                    Ok(core
                        .channel(&channel)
                        .ok()
                        .map(|channel| channel.name().to_owned()))
                })
                .await?;
            let media = if state.phase == TvPlaybackPhase::Playing {
                match self.playback.player() {
                    Some(player) => player.diagnostics().await.ok().map(|sample| {
                        json!({
                            "paused": sample.paused,
                            "currentTimeMs": sample.current_time_ms,
                            "presentedFrames": sample.presented_frames,
                            "decodedFrames": sample.presented_frames,
                            "totalVideoFrames": sample.presented_frames,
                            "droppedVideoFrames": sample.decoder_dropped_frames.unwrap_or(0)
                                + sample.vo_dropped_frames.unwrap_or(0),
                        })
                    }),
                    None => None,
                }
            } else {
                None
            };
            let current = updates.borrow();
            if current.selected == state.selected
                && current.phase == state.phase
                && current.transport_token == state.transport_token
            {
                return Ok(
                    json!({ "_tag": "snapshot", "channelName": name, "diagnostics": {
                        "version": 2, "phase": phase_name(&state), "engine": "embedded-libmpv",
                        "recoveryCount": state.recovery_attempts, "media": media,
                        "controls": {
                            "volumePercent": state.controls.volume * 100.,
                            "muted": state.controls.muted, "fullscreen": state.controls.fullscreen,
                        },
                    }}),
                );
            }
        }
    }

    fn begin_mutation(&self, stopping: bool) -> Result<Mutation<'_>, Failure> {
        let mut current = self.mutation.lock().unwrap();
        if current
            .as_ref()
            .is_some_and(|(active_stop, _)| *active_stop || !stopping)
        {
            return Err(Failure::Unavailable);
        }
        let (cancel, cancellation) = watch::channel(());
        *current = Some((stopping, cancel));
        Ok(Mutation {
            slot: &self.mutation,
            cancellation,
        })
    }
}

struct Mutation<'a> {
    slot: &'a Mutex<Option<(bool, watch::Sender<()>)>>,
    cancellation: watch::Receiver<()>,
}

impl Drop for Mutation<'_> {
    fn drop(&mut self) {
        let mut current = self.slot.lock().unwrap();
        if current
            .as_ref()
            .is_some_and(|(_, signal)| signal.subscribe().same_channel(&self.cancellation))
        {
            *current = None;
        }
    }
}

fn first_page() -> PageRequest {
    PageRequest::first(PageLimit::new(CHANNEL_LIMIT).expect("valid Agent Control page size"))
}

fn parse_term(term: String) -> Result<(SearchTerm, String), Failure> {
    // Same NFKC and JavaScript whitespace rule used by the installed CLI's
    // selection contract. The core also enforces its own search limit.
    let canonical = normalize_preference_title(&term);
    if canonical.is_empty() || canonical.len() > 256 {
        return Err(Failure::InvalidRequest);
    }
    let term = SearchTerm::parse(term).map_err(|_| Failure::InvalidRequest)?;
    Ok((term, canonical))
}

fn select_channel<'a>(
    channels: &'a [ChannelSummary],
    term: &str,
) -> Result<&'a ChannelSummary, Failure> {
    let hits: Vec<_> = channels
        .iter()
        .map(|channel| (channel, normalize_preference_title(channel.name())))
        .filter(|(_, name)| name.contains(term))
        .collect();
    let exact = hits.iter().any(|(_, name)| name == term);
    let mut matches = hits
        .into_iter()
        .filter(|(_, name)| !exact || name == term)
        .map(|(channel, _)| channel);
    let first = matches.next().ok_or(Failure::ChannelNotFound)?;
    let Some(second) = matches.next() else {
        return Ok(first);
    };
    let mut names = Vec::new();
    for channel in [first, second].into_iter().chain(matches) {
        if !names.iter().any(|name| name == channel.name()) {
            names.push(channel.name().to_owned());
        }
    }
    Err(Failure::AmbiguousChannel { names })
}

fn phase_name(snapshot: &TvPlaybackSnapshot) -> &'static str {
    match snapshot.phase {
        TvPlaybackPhase::Idle => "idle",
        TvPlaybackPhase::Starting => "starting",
        TvPlaybackPhase::Playing => "playing",
        TvPlaybackPhase::Pausing => "suspending",
        TvPlaybackPhase::Paused(_) => "paused",
        TvPlaybackPhase::ReplacingAudio => "replacing-audio",
        TvPlaybackPhase::Recovering { .. } => "recovering",
        TvPlaybackPhase::Stopping => "stopping",
        TvPlaybackPhase::Failed { .. } => "failed",
    }
}

#[cfg(test)]
mod tests;
