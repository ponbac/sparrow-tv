use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use sparrow_core::ResolvedPlaybackSource;

pub(super) type MpvLaunchFuture =
    Pin<Box<dyn Future<Output = Result<MpvProcess, MpvFailure>> + Send + 'static>>;
type MpvControlFuture = Pin<Box<dyn Future<Output = Result<(), MpvFailure>> + Send + 'static>>;
type MpvStopFuture = Pin<Box<dyn Future<Output = Result<(), MpvFailure>> + Send + 'static>>;
type MpvExitFuture = Pin<Box<dyn Future<Output = MpvExit> + Send + 'static>>;
type ControlProcess = Arc<dyn Fn(MpvPlaybackControl) -> MpvControlFuture + Send + Sync + 'static>;
type StopProcess = Box<dyn FnOnce() -> MpvStopFuture + Send + 'static>;
type AbortProcess = Box<dyn FnOnce() + Send + 'static>;

/// A bounded system-mpv volume percentage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MpvVolume(u8);

impl MpvVolume {
    pub(crate) const fn parse(percent: u8) -> Result<Self, MpvFailure> {
        if percent <= 100 {
            Ok(Self(percent))
        } else {
            Err(MpvFailure::InvalidControl)
        }
    }

    const fn percent(self) -> u8 {
        self.0
    }
}

/// One privacy-safe control for the system-mpv process owning a Playback Session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MpvPlaybackControl {
    HealthCheck,
    Pause,
    Resume,
    SetVolume(MpvVolume),
    SetMuted(bool),
    SetFullscreen(bool),
}

/// The private process-launch seam used by the playback actor.
///
/// Implementations receive the pinned source only inside Rust and must never
/// include it in command-line arguments, diagnostics, or returned values.
pub(super) trait NativeMpvPlayer: Send + Sync + 'static {
    fn launch(&self, source: Arc<ResolvedPlaybackSource>) -> MpvLaunchFuture;
}

/// A running mpv process whose lifetime remains owned by the playback actor.
pub(super) struct MpvProcess {
    pub(super) exited: MpvExitFuture,
    control: ControlProcess,
    stop: Option<StopProcess>,
    abort: Option<AbortProcess>,
}

impl MpvProcess {
    pub(super) fn controlled(
        exited: MpvExitFuture,
        control: ControlProcess,
        stop: StopProcess,
        abort: AbortProcess,
    ) -> Self {
        Self {
            exited,
            control,
            stop: Some(stop),
            abort: Some(abort),
        }
    }

    pub(super) fn control(&self, control: MpvPlaybackControl) -> MpvControlFuture {
        (self.control)(control)
    }

    pub(super) async fn stop(mut self) -> Result<(), MpvFailure> {
        let stop = self.stop.take().ok_or(MpvFailure::ControlUnavailable)?;
        let result = match stop().await {
            // A closed process-control channel means the owner task has already
            // reaped/dropped the child and its private socket. Consuming final
            // cleanup is therefore complete even though health/control calls
            // must continue to surface the unexpected termination.
            Err(MpvFailure::Terminated) => Ok(()),
            result => result,
        };
        if result.is_ok() {
            self.abort.take();
        }
        result
    }
}

impl Drop for MpvProcess {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MpvExit {
    Terminated,
}

/// Privacy-safe failures from the system-mpv adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum MpvFailure {
    #[cfg(any(test, not(target_os = "linux")))]
    #[error("mpv failover is unsupported on this platform")]
    Unsupported,
    #[error("the primary playback engine is still active")]
    PrimaryActive,
    #[error("the playback session is stale")]
    StaleSession,
    #[error("system mpv is not installed")]
    NotInstalled,
    #[error("system mpv does not satisfy the required version")]
    Incompatible,
    #[error("system mpv could not be launched")]
    LaunchFailed,
    #[error("system mpv control is unavailable")]
    ControlUnavailable,
    #[error("system mpv control is invalid")]
    InvalidControl,
    #[error("system mpv terminated")]
    Terminated,
}

impl MpvFailure {
    pub(crate) const fn retryable(self) -> bool {
        matches!(
            self,
            Self::LaunchFailed | Self::ControlUnavailable | Self::Terminated
        )
    }

    pub(crate) const fn reason(self) -> &'static str {
        match self {
            #[cfg(any(test, not(target_os = "linux")))]
            Self::Unsupported => "unsupported",
            Self::PrimaryActive => "primary-active",
            Self::StaleSession => "stale-session",
            Self::NotInstalled => "not-installed",
            Self::Incompatible => "incompatible",
            Self::LaunchFailed => "launch-failed",
            Self::ControlUnavailable => "control-unavailable",
            Self::InvalidControl => "invalid-control",
            Self::Terminated => "terminated",
        }
    }
}

pub(super) fn system_mpv_player(private_root: PathBuf) -> Arc<dyn NativeMpvPlayer> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::SystemMpvPlayer::new(private_root))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = private_root;
        Arc::new(UnsupportedMpvPlayer)
    }
}

#[cfg(any(test, not(target_os = "linux")))]
pub(super) struct UnsupportedMpvPlayer;

#[cfg(any(test, not(target_os = "linux")))]
impl NativeMpvPlayer for UnsupportedMpvPlayer {
    fn launch(&self, _source: Arc<ResolvedPlaybackSource>) -> MpvLaunchFuture {
        Box::pin(async { Err(MpvFailure::Unsupported) })
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        ffi::OsString,
        fs,
        io::{self, ErrorKind},
        os::unix::{
            ffi::OsStrExt as _,
            fs::{FileTypeExt as _, PermissionsExt as _},
        },
        path::{Path, PathBuf},
        process::Stdio,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    use serde::Deserialize;
    use sparrow_core::ResolvedPlaybackSource;
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
        net::UnixStream,
        process::{Child, Command},
        sync::{mpsc, oneshot},
        task::{AbortHandle, JoinHandle},
        time::{Instant, sleep, timeout},
    };

    #[cfg(test)]
    use super::MpvVolume;
    use super::{
        AbortProcess, ControlProcess, MpvControlFuture, MpvExit, MpvFailure, MpvLaunchFuture,
        MpvPlaybackControl, MpvProcess, MpvStopFuture, NativeMpvPlayer, StopProcess,
    };

    const MPV_DIRECTORY: &str = "mpv-v1";
    const REQUIRED_MPV_MAJOR: u64 = 0;
    const REQUIRED_MPV_MINOR: u64 = 41;
    const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
    const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
    const IPC_TIMEOUT: Duration = Duration::from_secs(3);
    #[cfg(not(test))]
    const MEDIA_START_TIMEOUT: Duration = Duration::from_secs(15);
    #[cfg(test)]
    const MEDIA_START_TIMEOUT: Duration = Duration::from_millis(500);
    const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(2);
    const KILL_TIMEOUT: Duration = Duration::from_secs(2);
    const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(20);
    const MAX_VERSION_BYTES: u64 = 4 * 1024;
    const MAX_IPC_LINE_BYTES: u64 = 8 * 1024;
    const MAX_IPC_MESSAGES: usize = 32;
    const MAX_MEDIA_START_MESSAGES: usize = 128;
    const MAX_UNIX_SOCKET_PATH_BYTES: usize = 100;
    const LOAD_REQUEST_ID: u64 = 1;
    const FIRST_CONTROL_REQUEST_ID: u64 = 2;

    const PLAYER_ARGUMENTS: [&str; 15] = [
        "--no-config",
        "--no-terminal",
        "--really-quiet",
        "--msg-level=all=no",
        "--idle=yes",
        "--force-window=immediate",
        "--mute=yes",
        "--vo=gpu-next",
        "--gpu-context=wayland",
        "--gpu-sw=yes",
        "--input-terminal=no",
        "--save-position-on-quit=no",
        "--use-filedir-conf=no",
        "--title=Sparrow TV",
        "--force-media-title=Sparrow TV",
    ];

    pub(super) struct SystemMpvPlayer {
        private_root: PathBuf,
        executable: PathBuf,
        sequence: Arc<AtomicU64>,
    }

    impl SystemMpvPlayer {
        pub(super) fn new(private_root: PathBuf) -> Self {
            Self {
                private_root,
                executable: PathBuf::from("mpv"),
                sequence: Arc::new(AtomicU64::new(1)),
            }
        }

        #[cfg(test)]
        fn with_executable(private_root: PathBuf, executable: PathBuf) -> Self {
            Self {
                private_root,
                executable,
                sequence: Arc::new(AtomicU64::new(1)),
            }
        }
    }

    impl NativeMpvPlayer for SystemMpvPlayer {
        fn launch(&self, source: Arc<ResolvedPlaybackSource>) -> MpvLaunchFuture {
            let private_root = self.private_root.clone();
            let executable = self.executable.clone();
            let sequence = Arc::clone(&self.sequence);
            Box::pin(async move {
                probe_version(&executable).await?;
                let socket = PrivateSocket::reserve(&private_root, &sequence)?;
                launch_player(&executable, source, socket).await
            })
        }
    }

    async fn probe_version(executable: &Path) -> Result<(), MpvFailure> {
        let mut command = Command::new(executable);
        command
            .args(["--no-config", "--version"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(classify_spawn_error)?;
        let stdout = child.stdout.take().ok_or(MpvFailure::LaunchFailed)?;
        let read = async move {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_VERSION_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| MpvFailure::LaunchFailed)?;
            Ok::<_, MpvFailure>(bytes)
        };
        let checked = timeout(VERSION_TIMEOUT, async {
            let (bytes, status) = tokio::join!(read, child.wait());
            let bytes = bytes?;
            let status = status.map_err(|_| MpvFailure::LaunchFailed)?;
            if !status.success() || bytes.len() as u64 > MAX_VERSION_BYTES {
                return Err(MpvFailure::Incompatible);
            }
            parse_supported_version(&bytes)
        })
        .await;
        match checked {
            Ok(result) => result,
            Err(_) => {
                terminate(&mut child).await;
                Err(MpvFailure::LaunchFailed)
            }
        }
    }

    fn parse_supported_version(bytes: &[u8]) -> Result<(), MpvFailure> {
        let line = std::str::from_utf8(bytes)
            .map_err(|_| MpvFailure::Incompatible)?
            .lines()
            .next()
            .ok_or(MpvFailure::Incompatible)?;
        let version = line
            .strip_prefix("mpv ")
            .and_then(|rest| rest.strip_prefix('v').or(Some(rest)))
            .and_then(|rest| rest.split_ascii_whitespace().next())
            .ok_or(MpvFailure::Incompatible)?;
        let mut numbers = version.split('.');
        let major = numbers
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(MpvFailure::Incompatible)?;
        let minor = numbers
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(MpvFailure::Incompatible)?;
        if (major, minor) < (REQUIRED_MPV_MAJOR, REQUIRED_MPV_MINOR) {
            return Err(MpvFailure::Incompatible);
        }
        Ok(())
    }

    async fn launch_player(
        executable: &Path,
        source: Arc<ResolvedPlaybackSource>,
        socket: PrivateSocket,
    ) -> Result<MpvProcess, MpvFailure> {
        let mut command = player_command(executable, socket.path());
        let child = command.spawn().map_err(classify_spawn_error)?;
        let mut launch = LaunchGuard::new(child, socket);
        let ipc = {
            let (child, socket) = launch.child_and_socket();
            wait_for_ipc(child, socket).await
        };
        let ipc = match ipc {
            Ok(ipc) => ipc,
            Err(error) => {
                launch.cleanup().await;
                return Err(error);
            }
        };
        let mut ipc = BufReader::new(ipc);
        let load = serde_json::to_vec(&serde_json::json!({
            "command": ["loadfile", source.location_for_adapter().as_str(), "replace"],
            "request_id": LOAD_REQUEST_ID,
        }));
        let load = match load {
            Ok(load) => load,
            Err(_) => {
                launch.cleanup().await;
                return Err(MpvFailure::ControlUnavailable);
            }
        };
        if let Err(error) = start_media(&mut ipc, &load).await {
            launch.cleanup().await;
            return Err(error);
        }

        let (control_sender, control_receiver) = mpsc::channel(1);
        let (exit_sender, exit_receiver) = oneshot::channel();
        let (child, socket) = launch.into_parts();
        let task = tokio::spawn(own_process(
            child,
            ipc,
            socket,
            control_receiver,
            exit_sender,
        ));
        Ok(process_handle(control_sender, exit_receiver, task))
    }

    fn player_command(executable: &Path, socket: &Path) -> Command {
        let mut command = Command::new(executable);
        command
            .args(PLAYER_ARGUMENTS)
            .arg(socket_argument(socket))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    fn socket_argument(socket: &Path) -> OsString {
        let mut argument = OsString::from("--input-ipc-server=");
        argument.push(socket.as_os_str());
        argument
    }

    async fn wait_for_ipc(
        child: &mut Child,
        socket: &PrivateSocket,
    ) -> Result<UnixStream, MpvFailure> {
        let deadline = Instant::now() + SOCKET_TIMEOUT;
        loop {
            if child
                .try_wait()
                .map_err(|_| MpvFailure::LaunchFailed)?
                .is_some()
            {
                return Err(MpvFailure::Terminated);
            }
            match UnixStream::connect(socket.path()).await {
                Ok(stream) => {
                    socket.restrict()?;
                    return Ok(stream);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::NotFound | ErrorKind::ConnectionRefused
                    ) && Instant::now() < deadline =>
                {
                    sleep(SOCKET_POLL_INTERVAL).await;
                }
                Err(_) => return Err(MpvFailure::ControlUnavailable),
            }
            if Instant::now() >= deadline {
                return Err(MpvFailure::ControlUnavailable);
            }
        }
    }

    async fn send_command(
        stream: &mut BufReader<UnixStream>,
        command: &[u8],
        request_id: u64,
    ) -> Result<(), MpvFailure> {
        timeout(IPC_TIMEOUT, async {
            write_command(stream, command).await?;

            for _ in 0..MAX_IPC_MESSAGES {
                let reply = read_message(stream).await?;
                if reply.event.as_deref() == Some("end-file") {
                    return Err(MpvFailure::Terminated);
                }
                if reply.request_id == Some(request_id) {
                    return (reply.error.as_deref() == Some("success"))
                        .then_some(())
                        .ok_or(MpvFailure::ControlUnavailable);
                }
            }
            Err(MpvFailure::ControlUnavailable)
        })
        .await
        .map_err(|_| MpvFailure::ControlUnavailable)?
    }

    /// Loads the private source and does not acknowledge presentation until mpv
    /// has actually restarted playback. Command replies and events may arrive in
    /// either order on the IPC socket, so both are observed in one bounded loop.
    async fn start_media(
        stream: &mut BufReader<UnixStream>,
        command: &[u8],
    ) -> Result<(), MpvFailure> {
        timeout(MEDIA_START_TIMEOUT, async {
            write_command(stream, command).await?;
            let mut accepted = false;
            let mut playing = false;
            for _ in 0..MAX_MEDIA_START_MESSAGES {
                let message = read_message(stream).await?;
                if message.event.as_deref() == Some("end-file") {
                    return Err(MpvFailure::Terminated);
                }
                if message.event.as_deref() == Some("playback-restart") {
                    playing = true;
                }
                if message.request_id == Some(LOAD_REQUEST_ID) {
                    if message.error.as_deref() != Some("success") {
                        return Err(MpvFailure::ControlUnavailable);
                    }
                    accepted = true;
                }
                if accepted && playing {
                    return Ok(());
                }
            }
            Err(MpvFailure::ControlUnavailable)
        })
        .await
        .map_err(|_| MpvFailure::ControlUnavailable)?
    }

    async fn write_command(
        stream: &mut BufReader<UnixStream>,
        command: &[u8],
    ) -> Result<(), MpvFailure> {
        stream
            .get_mut()
            .write_all(command)
            .await
            .map_err(|_| MpvFailure::ControlUnavailable)?;
        stream
            .get_mut()
            .write_all(b"\n")
            .await
            .map_err(|_| MpvFailure::ControlUnavailable)?;
        stream
            .get_mut()
            .flush()
            .await
            .map_err(|_| MpvFailure::ControlUnavailable)
    }

    async fn read_message(stream: &mut BufReader<UnixStream>) -> Result<MpvMessage, MpvFailure> {
        let mut line = Vec::new();
        let read = (&mut *stream)
            .take(MAX_IPC_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| MpvFailure::ControlUnavailable)?;
        if read == 0 {
            return Err(MpvFailure::Terminated);
        }
        if line.len() as u64 > MAX_IPC_LINE_BYTES {
            return Err(MpvFailure::ControlUnavailable);
        }
        serde_json::from_slice(&line).map_err(|_| MpvFailure::ControlUnavailable)
    }

    fn playback_control_command(
        control: MpvPlaybackControl,
        request_id: u64,
    ) -> Result<Vec<u8>, MpvFailure> {
        let command = match control {
            MpvPlaybackControl::HealthCheck => serde_json::json!(["get_property", "idle-active"]),
            MpvPlaybackControl::Pause => serde_json::json!(["set_property", "pause", true]),
            MpvPlaybackControl::Resume => serde_json::json!(["set_property", "pause", false]),
            MpvPlaybackControl::SetVolume(volume) => {
                serde_json::json!(["set_property", "volume", volume.percent()])
            }
            MpvPlaybackControl::SetMuted(muted) => {
                serde_json::json!(["set_property", "mute", muted])
            }
            MpvPlaybackControl::SetFullscreen(fullscreen) => {
                serde_json::json!(["set_property", "fullscreen", fullscreen])
            }
        };
        serde_json::to_vec(&serde_json::json!({
            "command": command,
            "request_id": request_id,
        }))
        .map_err(|_| MpvFailure::ControlUnavailable)
    }

    async fn send_quit(stream: &mut BufReader<UnixStream>) {
        let command = serde_json::to_vec(&serde_json::json!({
            "command": ["quit", 0],
        }));
        if let Ok(command) = command {
            let _ = timeout(IPC_TIMEOUT, async {
                stream.get_mut().write_all(&command).await?;
                stream.get_mut().write_all(b"\n").await?;
                stream.get_mut().flush().await
            })
            .await;
        }
    }

    async fn own_process(
        mut child: Child,
        mut ipc: BufReader<UnixStream>,
        _socket: PrivateSocket,
        mut controls: mpsc::Receiver<ProcessControl>,
        exited: oneshot::Sender<MpvExit>,
    ) {
        let mut next_request_id = FIRST_CONTROL_REQUEST_ID;
        loop {
            tokio::select! {
                _ = child.wait() => {
                    let _ = exited.send(MpvExit::Terminated);
                    return;
                }
                message = read_message(&mut ipc) => {
                    match message {
                        Ok(message) if message.event.as_deref() != Some("end-file") => {}
                        Ok(_) | Err(_) => {
                            terminate(&mut child).await;
                            let _ = exited.send(MpvExit::Terminated);
                            return;
                        }
                    }
                }
                command = controls.recv() => {
                    match command {
                        Some(ProcessControl::Apply(control, reply)) => {
                            let Some(following_request_id) = next_request_id.checked_add(1) else {
                                let _ = reply.send(Err(MpvFailure::ControlUnavailable));
                                continue;
                            };
                            let request_id = next_request_id;
                            next_request_id = following_request_id;
                            let result = match playback_control_command(control, request_id) {
                                Ok(command) => send_command(&mut ipc, &command, request_id).await,
                                Err(error) => Err(error),
                            };
                            let terminated = result == Err(MpvFailure::Terminated);
                            let _ = reply.send(result);
                            if terminated {
                                terminate(&mut child).await;
                                let _ = exited.send(MpvExit::Terminated);
                                return;
                            }
                        }
                        Some(ProcessControl::Stop(reply)) => {
                            send_quit(&mut ipc).await;
                            let result = reap_after_quit(&mut child).await;
                            let _ = reply.send(result);
                            return;
                        }
                        Some(ProcessControl::Abort) => {
                            terminate(&mut child).await;
                            return;
                        }
                        None => {
                            terminate(&mut child).await;
                            return;
                        }
                    }
                }
            }
        }
    }

    async fn reap_after_quit(child: &mut Child) -> Result<(), MpvFailure> {
        match timeout(GRACEFUL_STOP_TIMEOUT, child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err(MpvFailure::ControlUnavailable),
            Err(_) => {
                child
                    .start_kill()
                    .map_err(|_| MpvFailure::ControlUnavailable)?;
                timeout(KILL_TIMEOUT, child.wait())
                    .await
                    .map_err(|_| MpvFailure::ControlUnavailable)?
                    .map(|_| ())
                    .map_err(|_| MpvFailure::ControlUnavailable)
            }
        }
    }

    async fn terminate(child: &mut Child) {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = child.start_kill();
        let _ = timeout(KILL_TIMEOUT, child.wait()).await;
    }

    fn process_handle(
        controls: mpsc::Sender<ProcessControl>,
        exited: oneshot::Receiver<MpvExit>,
        task: JoinHandle<()>,
    ) -> MpvProcess {
        let abort = task.abort_handle();
        let control_sender = controls.clone();
        let control: ControlProcess = Arc::new(move |control| {
            let controls = control_sender.clone();
            Box::pin(async move {
                let (reply, response) = oneshot::channel();
                controls
                    .send(ProcessControl::Apply(control, reply))
                    .await
                    .map_err(|_| MpvFailure::Terminated)?;
                response.await.map_err(|_| MpvFailure::ControlUnavailable)?
            }) as MpvControlFuture
        });
        let stop_controls = controls.clone();
        let stop: StopProcess = Box::new(move || {
            Box::pin(async move {
                let (reply, response) = oneshot::channel();
                stop_controls
                    .send(ProcessControl::Stop(reply))
                    .await
                    .map_err(|_| MpvFailure::Terminated)?;
                response.await.map_err(|_| MpvFailure::Terminated)?
            }) as MpvStopFuture
        });
        let abort: AbortProcess = Box::new(move || abort_process(controls, abort));
        let exited = Box::pin(async move { exited.await.unwrap_or(MpvExit::Terminated) });
        MpvProcess::controlled(exited, control, stop, abort)
    }

    fn abort_process(controls: mpsc::Sender<ProcessControl>, abort: AbortHandle) {
        match controls.try_send(ProcessControl::Abort) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => abort.abort(),
        }
    }

    fn classify_spawn_error(error: io::Error) -> MpvFailure {
        if error.kind() == ErrorKind::NotFound {
            MpvFailure::NotInstalled
        } else {
            MpvFailure::LaunchFailed
        }
    }

    enum ProcessControl {
        Apply(MpvPlaybackControl, oneshot::Sender<Result<(), MpvFailure>>),
        Stop(oneshot::Sender<Result<(), MpvFailure>>),
        Abort,
    }

    #[derive(Deserialize)]
    struct MpvMessage {
        error: Option<String>,
        request_id: Option<u64>,
        event: Option<String>,
    }

    struct PrivateSocket {
        path: PathBuf,
    }

    impl PrivateSocket {
        fn reserve(private_root: &Path, sequence: &AtomicU64) -> Result<Self, MpvFailure> {
            let directory = private_root.join(MPV_DIRECTORY);
            prepare_private_directory(&directory)?;
            let sequence = sequence.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!("mpv-{}-{sequence:016x}.sock", std::process::id()));
            if path.as_os_str().as_bytes().len() > MAX_UNIX_SOCKET_PATH_BYTES {
                return Err(MpvFailure::LaunchFailed);
            }
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self { path }),
                _ => Err(MpvFailure::LaunchFailed),
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn restrict(&self) -> Result<(), MpvFailure> {
            let metadata =
                fs::symlink_metadata(&self.path).map_err(|_| MpvFailure::ControlUnavailable)?;
            if !metadata.file_type().is_socket() {
                return Err(MpvFailure::ControlUnavailable);
            }
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))
                .map_err(|_| MpvFailure::ControlUnavailable)
        }
    }

    impl Drop for PrivateSocket {
        fn drop(&mut self) {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(_) => {}
            }
        }
    }

    fn prepare_private_directory(path: &Path) -> Result<(), MpvFailure> {
        fs::create_dir_all(path).map_err(|_| MpvFailure::LaunchFailed)?;
        let metadata = fs::symlink_metadata(path).map_err(|_| MpvFailure::LaunchFailed)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(MpvFailure::LaunchFailed);
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| MpvFailure::LaunchFailed)
    }

    struct LaunchGuard {
        child: Option<Child>,
        socket: Option<PrivateSocket>,
    }

    impl LaunchGuard {
        fn new(child: Child, socket: PrivateSocket) -> Self {
            Self {
                child: Some(child),
                socket: Some(socket),
            }
        }

        fn child_and_socket(&mut self) -> (&mut Child, &PrivateSocket) {
            let Self { child, socket } = self;
            (
                child.as_mut().expect("launch guard owns child"),
                socket.as_ref().expect("launch guard owns socket"),
            )
        }

        fn into_parts(mut self) -> (Child, PrivateSocket) {
            let child = self.child.take().expect("launch guard owns child");
            let socket = self.socket.take().expect("launch guard owns socket");
            (child, socket)
        }

        async fn cleanup(&mut self) {
            if let Some(child) = self.child.as_mut() {
                terminate(child).await;
            }
            drop(self.child.take());
            drop(self.socket.take());
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{
            ffi::{OsStr, OsString},
            fs,
            path::PathBuf,
        };

        use tempfile::TempDir;

        use super::*;

        #[test]
        fn required_version_parser_is_exact_and_bounded() {
            for accepted in [
                b"mpv v0.41.0 Copyright\n".as_slice(),
                b"mpv 0.42.1\n".as_slice(),
                b"mpv v1.0.0\n".as_slice(),
            ] {
                assert_eq!(parse_supported_version(accepted), Ok(()));
            }
            for rejected in [
                b"mpv v0.40.0\n".as_slice(),
                b"mpv unknown\n".as_slice(),
                b"not-mpv v0.41.0\n".as_slice(),
                &[0xff][..],
            ] {
                assert_eq!(
                    parse_supported_version(rejected),
                    Err(MpvFailure::Incompatible)
                );
            }
        }

        #[test]
        fn player_command_has_only_fixed_safe_arguments_and_private_socket() {
            let socket = Path::new("/tmp/sparrow-private/mpv.sock");
            let command = player_command(Path::new("/usr/bin/mpv"), socket);
            let command = command.as_std();
            assert_eq!(command.get_program(), OsStr::new("/usr/bin/mpv"));
            let arguments = command.get_args().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(
                &arguments[..PLAYER_ARGUMENTS.len()],
                &PLAYER_ARGUMENTS.map(OsString::from)
            );
            assert_eq!(
                arguments.last(),
                Some(&OsString::from(
                    "--input-ipc-server=/tmp/sparrow-private/mpv.sock"
                ))
            );
            let joined = arguments
                .iter()
                .map(|argument| argument.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(!joined.contains("http://"));
            assert!(!joined.contains("https://"));
        }

        #[test]
        fn socket_guard_uses_private_bounded_names_and_removes_its_file() {
            let directory = TempDir::new().expect("temporary private root");
            let sequence = AtomicU64::new(7);
            let socket =
                PrivateSocket::reserve(directory.path(), &sequence).expect("socket path reserves");
            let path = socket.path().to_owned();
            assert!(path.starts_with(directory.path().join(MPV_DIRECTORY)));
            assert!(path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("mpv-") && name.ends_with(".sock")
            }));
            fs::write(&path, b"fixture").expect("fixture socket marker writes");
            drop(socket);
            assert!(!path.exists());
            let mode = fs::metadata(directory.path().join(MPV_DIRECTORY))
                .expect("private directory exists")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }

        #[tokio::test]
        async fn consuming_stop_succeeds_when_the_owner_drops_an_accepted_reply() {
            let (controls, mut control_receiver) = mpsc::channel(1);
            let (exit_sender, exit_receiver) = oneshot::channel();
            let owner = tokio::spawn(async move {
                let command = control_receiver
                    .recv()
                    .await
                    .expect("owner receives the accepted stop");
                let ProcessControl::Stop(reply) = command else {
                    panic!("owner receives a stop command");
                };
                drop(reply);
                drop(exit_sender);
            });
            let process = process_handle(controls, exit_receiver, owner);

            process
                .stop()
                .await
                .expect("a dropped accepted stop reply means ownership already ended");
        }

        #[tokio::test]
        async fn consuming_stop_preserves_an_explicit_owner_cleanup_failure() {
            let (controls, mut control_receiver) = mpsc::channel(1);
            let (exit_sender, exit_receiver) = oneshot::channel();
            let owner = tokio::spawn(async move {
                let command = control_receiver
                    .recv()
                    .await
                    .expect("owner receives the accepted stop");
                let ProcessControl::Stop(reply) = command else {
                    panic!("owner receives a stop command");
                };
                let _ = reply.send(Err(MpvFailure::ControlUnavailable));
                drop(exit_sender);
            });
            let process = process_handle(controls, exit_receiver, owner);

            assert_eq!(process.stop().await, Err(MpvFailure::ControlUnavailable));
        }

        #[tokio::test]
        async fn missing_executable_is_a_typed_failure_without_socket_residue() {
            let directory = TempDir::new().expect("temporary private root");
            let launcher = SystemMpvPlayer::with_executable(
                directory.path().to_owned(),
                directory.path().join("missing-mpv"),
            );
            let source = crate::playback::tests::CoreFixture::one(
                "https://provider.invalid/private.ts?token=canary",
            )
            .await;
            assert!(matches!(
                launcher
                    .launch(Arc::new(
                        source
                            .core
                            .resolve_playback(&source.channel)
                            .expect("fixture source resolves")
                    ))
                    .await,
                Err(MpvFailure::NotInstalled)
            ));
            assert!(!directory.path().join(MPV_DIRECTORY).exists());
        }

        #[tokio::test]
        async fn production_ipc_keeps_source_and_controls_private_and_reaps_on_stop() {
            let directory = TempDir::new().expect("temporary private root");
            let executable = install_fake_mpv(directory.path());
            let launcher =
                SystemMpvPlayer::with_executable(directory.path().to_owned(), executable);
            let private_location = "https://provider.invalid/private.ts?token=canary";
            let source = crate::playback::tests::CoreFixture::one(private_location).await;
            let process = launcher
                .launch(Arc::new(
                    source
                        .core
                        .resolve_playback(&source.channel)
                        .expect("fixture source resolves"),
                ))
                .await
                .expect("fake mpv accepts private IPC load");

            let arguments: Vec<String> = serde_json::from_slice(
                &fs::read(directory.path().join("argv.json")).expect("argv record reads"),
            )
            .expect("argv record parses");
            assert!(arguments.iter().all(|argument| {
                !argument.contains("provider.invalid") && !argument.contains("token=")
            }));
            let load: serde_json::Value = serde_json::from_slice(
                &fs::read(directory.path().join("load.json")).expect("load record reads"),
            )
            .expect("load record parses");
            assert!(
                load.pointer("/command/1")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|location| location == private_location),
                "the source crosses only the private IPC seam"
            );
            let socket_path = PathBuf::from(
                fs::read_to_string(directory.path().join("socket-path"))
                    .expect("socket path record reads"),
            );
            let mode = fs::symlink_metadata(&socket_path)
                .expect("live socket exists")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);

            for control in [
                MpvPlaybackControl::HealthCheck,
                MpvPlaybackControl::Pause,
                MpvPlaybackControl::Resume,
                MpvPlaybackControl::SetVolume(
                    MpvVolume::parse(37).expect("fixture volume is valid"),
                ),
                MpvPlaybackControl::SetMuted(true),
                MpvPlaybackControl::SetFullscreen(true),
            ] {
                process
                    .control(control)
                    .await
                    .expect("fake mpv accepts private IPC control");
            }

            process.stop().await.expect("fake mpv stops and reaps");
            assert!(
                !socket_path.exists(),
                "the IPC socket is removed after reap"
            );
            let controls: Vec<serde_json::Value> = serde_json::from_slice(
                &fs::read(directory.path().join("control.json")).expect("control record reads"),
            )
            .expect("control record parses");
            let commands = controls
                .iter()
                .map(|control| {
                    control
                        .pointer("/command")
                        .cloned()
                        .expect("command exists")
                })
                .collect::<Vec<_>>();
            assert_eq!(
                commands,
                vec![
                    serde_json::json!(["get_property", "idle-active"]),
                    serde_json::json!(["set_property", "pause", true]),
                    serde_json::json!(["set_property", "pause", false]),
                    serde_json::json!(["set_property", "volume", 37]),
                    serde_json::json!(["set_property", "mute", true]),
                    serde_json::json!(["set_property", "fullscreen", true]),
                    serde_json::json!(["quit", 0]),
                ]
            );
            let controls = serde_json::to_string(&controls).expect("controls serialize");
            assert!(!controls.contains("provider.invalid"));
            assert!(!controls.contains("token="));
        }

        #[tokio::test]
        async fn ipc_launch_failure_reaps_the_child_and_removes_its_socket() {
            let directory = TempDir::new().expect("temporary private root");
            let executable = install_fake_mpv(directory.path());
            fs::write(directory.path().join("fail-before-reply"), b"fail")
                .expect("failure marker writes");
            let launcher =
                SystemMpvPlayer::with_executable(directory.path().to_owned(), executable);
            let source = crate::playback::tests::CoreFixture::one(
                "https://provider.invalid/private.ts?token=canary",
            )
            .await;

            assert!(matches!(
                launcher
                    .launch(Arc::new(
                        source
                            .core
                            .resolve_playback(&source.channel)
                            .expect("fixture source resolves")
                    ))
                    .await,
                Err(MpvFailure::Terminated)
            ));
            let socket_path = PathBuf::from(
                fs::read_to_string(directory.path().join("socket-path"))
                    .expect("socket path record reads"),
            );
            assert!(!socket_path.exists(), "failed launch removes its socket");
            let pid = fs::read_to_string(directory.path().join("pid"))
                .expect("pid record reads")
                .parse::<u32>()
                .expect("pid record parses");
            assert!(
                !Path::new("/proc").join(pid.to_string()).exists(),
                "failed launch reaps its child"
            );
        }

        #[tokio::test]
        async fn media_start_accepts_playback_event_before_load_reply() {
            let directory = TempDir::new().expect("temporary private root");
            let executable = install_fake_mpv(directory.path());
            fs::write(directory.path().join("event-before-reply"), b"fixture")
                .expect("event-order marker writes");
            let launcher =
                SystemMpvPlayer::with_executable(directory.path().to_owned(), executable);
            let source = crate::playback::tests::CoreFixture::one(
                "https://provider.invalid/private.ts?token=canary",
            )
            .await;

            let process = launcher
                .launch(Arc::new(
                    source
                        .core
                        .resolve_playback(&source.channel)
                        .expect("fixture source resolves"),
                ))
                .await
                .expect("the order-independent start handshake succeeds");
            process.stop().await.expect("fake mpv stops and reaps");
        }

        #[tokio::test]
        async fn missing_playback_start_event_reaps_the_child_and_socket() {
            let directory = TempDir::new().expect("temporary private root");
            let executable = install_fake_mpv(directory.path());
            fs::write(directory.path().join("omit-playback-restart"), b"fixture")
                .expect("missing-event marker writes");
            let launcher =
                SystemMpvPlayer::with_executable(directory.path().to_owned(), executable);
            let source = crate::playback::tests::CoreFixture::one(
                "https://provider.invalid/private.ts?token=canary",
            )
            .await;

            assert!(matches!(
                launcher
                    .launch(Arc::new(
                        source
                            .core
                            .resolve_playback(&source.channel)
                            .expect("fixture source resolves")
                    ))
                    .await,
                Err(MpvFailure::ControlUnavailable)
            ));
            let socket_path = PathBuf::from(
                fs::read_to_string(directory.path().join("socket-path"))
                    .expect("socket path record reads"),
            );
            assert!(!socket_path.exists(), "timed-out launch removes its socket");
        }

        #[tokio::test]
        async fn end_file_event_terminates_an_idle_process_and_removes_its_socket() {
            let directory = TempDir::new().expect("temporary private root");
            let executable = install_fake_mpv(directory.path());
            fs::write(directory.path().join("end-after-restart"), b"fixture")
                .expect("end-event marker writes");
            let launcher =
                SystemMpvPlayer::with_executable(directory.path().to_owned(), executable);
            let source = crate::playback::tests::CoreFixture::one(
                "https://provider.invalid/private.ts?token=canary",
            )
            .await;
            let mut process = launcher
                .launch(Arc::new(
                    source
                        .core
                        .resolve_playback(&source.channel)
                        .expect("fixture source resolves"),
                ))
                .await
                .expect("fake media reaches playback");
            let socket_path = PathBuf::from(
                fs::read_to_string(directory.path().join("socket-path"))
                    .expect("socket path record reads"),
            );

            assert_eq!(
                timeout(Duration::from_secs(2), &mut process.exited)
                    .await
                    .expect("end-file is observed"),
                MpvExit::Terminated
            );
            let deadline = Instant::now() + Duration::from_secs(2);
            while socket_path.exists() && Instant::now() < deadline {
                sleep(Duration::from_millis(5)).await;
            }
            assert!(!socket_path.exists(), "end-file removes the private socket");
        }

        fn install_fake_mpv(root: &Path) -> PathBuf {
            let executable = root.join("fake-mpv.py");
            fs::write(&executable, FAKE_MPV).expect("fake mpv writes");
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
                .expect("fake mpv becomes executable");
            executable
        }

        const FAKE_MPV: &str = r#"#!/usr/bin/env python3
import json
import os
import socket
import sys
import time

root = os.path.dirname(os.path.realpath(__file__))
if "--version" in sys.argv[1:]:
    print("mpv v0.41.0")
    raise SystemExit(0)

arguments = sys.argv[1:]
with open(os.path.join(root, "argv.json"), "w", encoding="utf-8") as record:
    json.dump(arguments, record)
with open(os.path.join(root, "pid"), "w", encoding="ascii") as record:
    record.write(str(os.getpid()))
socket_argument = next(value for value in arguments if value.startswith("--input-ipc-server="))
socket_path = socket_argument.split("=", 1)[1]
with open(os.path.join(root, "socket-path"), "w", encoding="utf-8") as record:
    record.write(socket_path)
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(socket_path)
server.listen(1)
connection, _ = server.accept()
deadline = time.monotonic() + 2
while (os.stat(socket_path).st_mode & 0o777) != 0o600 and time.monotonic() < deadline:
    time.sleep(0.005)
reader = connection.makefile("rb")
load = reader.readline()
with open(os.path.join(root, "load.json"), "wb") as record:
    record.write(load)
if os.path.exists(os.path.join(root, "fail-before-reply")):
    connection.close()
    server.close()
    raise SystemExit(23)
request = json.loads(load)
reply = (json.dumps({"error": "success", "request_id": request["request_id"]}) + "\n").encode()
restarted = (json.dumps({"event": "playback-restart"}) + "\n").encode()
if os.path.exists(os.path.join(root, "event-before-reply")):
    connection.sendall(restarted)
    connection.sendall(reply)
else:
    connection.sendall(reply)
    if not os.path.exists(os.path.join(root, "omit-playback-restart")):
        connection.sendall(restarted)
if os.path.exists(os.path.join(root, "end-after-restart")):
    connection.sendall((json.dumps({"event": "end-file", "reason": "error", "file_error": "redacted-fixture"}) + "\n").encode())
controls = []
while True:
    wire = reader.readline()
    if not wire:
        break
    control = json.loads(wire)
    controls.append(control)
    if control["command"][0] == "quit":
        break
    connection.sendall((json.dumps({"error": "success", "request_id": control["request_id"]}) + "\n").encode())
with open(os.path.join(root, "control.json"), "wb") as record:
    record.write(json.dumps(controls).encode())
connection.close()
server.close()
"#;
    }
}
