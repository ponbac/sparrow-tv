//! Embedded Linux libmpv presentation for the shared TV Playback Session owner.
#![cfg(all(target_os = "linux", feature = "embedded"))]

mod engine;
mod frame;
mod render;

use std::{
    sync::{Arc, OnceLock, Weak},
    thread::{self, JoinHandle, Thread},
};

use sparrow_installed_playback::{
    PlaybackManager,
    android::{
        AndroidPlaybackAudio, AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackPhase,
        AndroidPlaybackStatus, AndroidPlaybackViewport,
    },
    tv::{
        TvPlaybackCommand, TvPlaybackRunner, TvPresentation, TvPresentationEvent,
        TvPresentationFuture, TvPresentationKey, TvPresentationStop, TvRecoveryFrame,
    },
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot, watch},
};
use tokio_util::sync::CancellationToken;

use engine::{Engine, Event};
pub use frame::VideoFrame;
use render::{Publication, RenderWorker};

#[derive(Clone, Copy, Debug, Default)]
#[repr(i32)]
pub enum Backend {
    #[default]
    Software = 0,
    #[cfg(feature = "egl")]
    Egl = 1,
}

/// Selected counters and controls only. No raw libmpv properties can escape.
#[derive(Clone, Copy, Debug)]
pub struct MpvDiagnostics {
    pub presented_frames: u64,
    pub current_time_ms: u64,
    pub vo_delayed_frames: Option<u64>,
    pub decoder_dropped_frames: Option<u64>,
    pub vo_dropped_frames: Option<u64>,
    pub volume: f64,
    pub muted: bool,
    pub paused: bool,
    pub audio_output_active: bool,
    pub audio_stream_id: Option<u64>,
    pub track_count: Option<u64>,
    pub hardware_decoding: bool,
    pub opaque_playlist: bool,
}

type Operation = Box<dyn FnOnce(&mut Worker) + Send>;
type Events = OnceLock<mpsc::WeakSender<TvPlaybackCommand>>;

pub struct MpvPresentation {
    operations: mpsc::Sender<Operation>,
    latest: Arc<Publication>,
    events: Arc<Events>,
    shutdown: CancellationToken,
    worker: Thread,
    join: Option<JoinHandle<()>>,
}

impl MpvPresentation {
    /// Starts the dedicated presentation and render worker. The Playback
    /// Manager and its runtime must remain live while the presentation is used.
    pub fn new(
        playback: &Arc<PlaybackManager>,
        backend: Backend,
    ) -> Result<Self, AndroidPlaybackError> {
        let runtime = Handle::try_current().map_err(|_| AndroidPlaybackError)?;
        let (operations, inbox) = mpsc::channel(16);
        let latest = Arc::new(Publication::default());
        let events = Arc::new(OnceLock::new());
        let shutdown = CancellationToken::new();
        let weak_playback = Arc::downgrade(playback);
        let publication = Arc::clone(&latest);
        let event_sink = Arc::clone(&events);
        let finished = shutdown.clone();
        let join = thread::Builder::new()
            .name("sparrow-mpv".into())
            .spawn(move || {
                Worker {
                    playback: weak_playback,
                    runtime,
                    backend,
                    latest: publication,
                    events: event_sink,
                    binding: None,
                }
                .run(inbox, finished);
            })
            .map_err(|_| AndroidPlaybackError)?;
        Ok(Self {
            operations,
            latest,
            events,
            shutdown,
            worker: join.thread().clone(),
            join: Some(join),
        })
    }

    /// Routes exact-key presentation events to the owner without retaining the runner.
    pub fn connect(&self, runner: &TvPlaybackRunner) {
        let _ = self.events.set(runner.commands().downgrade());
    }

    /// Loads the latest visible picture without waiting on the render worker.
    pub fn latest_frame(&self) -> Option<Arc<VideoFrame>> {
        self.latest.visible_frame()
    }

    /// Coalesces frame publication, visibility changes and picture clears.
    pub fn subscribe_frames(&self) -> watch::Receiver<()> {
        self.latest.changed.subscribe()
    }

    pub async fn diagnostics(&self) -> Result<MpvDiagnostics, AndroidPlaybackError> {
        self.request(|worker| {
            Ok(worker
                .binding
                .as_ref()
                .ok_or(AndroidPlaybackError)?
                .diagnostics())
        })
        .await
    }

    fn request<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Worker) -> Result<T, AndroidPlaybackError> + Send + 'static,
    ) -> TvPresentationFuture<T> {
        let operations = self.operations.clone();
        let worker = self.worker.clone();
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            operations
                .send(Box::new(move |worker| {
                    if !reply.is_closed() {
                        let _ = reply.send(operation(worker));
                    }
                }))
                .await
                .map_err(|_| AndroidPlaybackError)?;
            worker.unpark();
            response.await.map_err(|_| AndroidPlaybackError)?
        })
    }
}

impl Drop for MpvPresentation {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.worker.unpark();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct StartGuard {
    cancel: Option<CancellationToken>,
    worker: Thread,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
            self.worker.unpark();
        }
    }
}

impl TvPresentation for MpvPresentation {
    fn start(
        &self,
        key: TvPresentationKey,
        viewport: AndroidPlaybackViewport,
        controls: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()> {
        let cancelled = CancellationToken::new();
        let mut guard = StartGuard {
            cancel: Some(cancelled.clone()),
            worker: self.worker.clone(),
        };
        let start = self.request(move |worker| worker.start(key, viewport, controls, cancelled));
        Box::pin(async move {
            start.await?;
            guard.cancel.take();
            Ok(())
        })
    }

    fn stop(&self, key: TvPresentationKey, stop: TvPresentationStop) -> TvPresentationFuture<()> {
        self.request(move |worker| {
            worker.stop(&key, stop);
            Ok(())
        })
    }

    fn capture_frame(&self, key: TvPresentationKey) -> TvPresentationFuture<Arc<TvRecoveryFrame>> {
        self.request(move |worker| {
            let frame = worker
                .latest
                .picture(&key)
                .and_then(|picture| picture.frame.load_full())
                .ok_or(AndroidPlaybackError)?;
            Ok(Arc::new(frame.still()))
        })
    }

    fn status(&self, key: TvPresentationKey) -> TvPresentationFuture<AndroidPlaybackStatus> {
        self.request(move |worker| Ok(worker.binding_mut(&key)?.status()))
    }

    fn set_controls(
        &self,
        key: TvPresentationKey,
        controls: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()> {
        self.request(move |worker| worker.binding_mut(&key)?.engine.controls(controls))
    }

    fn set_viewport(
        &self,
        key: TvPresentationKey,
        viewport: Option<AndroidPlaybackViewport>,
    ) -> TvPresentationFuture<()> {
        self.request(move |worker| {
            if let Ok(binding) = worker.binding_mut(&key) {
                binding.render.set_viewport(viewport);
            } else {
                worker
                    .latest
                    .picture(&key)
                    .ok_or(AndroidPlaybackError)?
                    .set_viewport(viewport);
            }
            worker.latest.changed.send_replace(());
            Ok(())
        })
    }
}

struct Binding {
    key: TvPresentationKey,
    render: RenderWorker,
    engine: Engine,
    cancelled: CancellationToken,
    first_picture: bool,
    terminal: bool,
}

impl Drop for Binding {
    fn drop(&mut self) {
        // Field drop order joins the renderer before terminating the core.
        self.engine.cancel_reads();
    }
}

impl Binding {
    fn diagnostics(&self) -> MpvDiagnostics {
        let engine = &self.engine;
        MpvDiagnostics {
            presented_frames: self.render.presented(),
            current_time_ms: (engine.number(c"time-pos") * 1_000.) as u64,
            vo_delayed_frames: engine.integer(c"vo-delayed-frame-count"),
            decoder_dropped_frames: engine.integer(c"decoder-frame-drop-count"),
            vo_dropped_frames: engine.integer(c"frame-drop-count"),
            volume: engine.number(c"volume"),
            muted: engine.flag(c"mute"),
            paused: engine.flag(c"pause"),
            audio_output_active: engine.has_string(c"current-ao")
                && !engine.string_is(c"current-ao", c"null"),
            audio_stream_id: engine.integer(c"current-tracks/audio/src-id"),
            track_count: engine.integer(c"track-list/count"),
            hardware_decoding: engine.has_string(c"hwdec-current")
                && !engine.string_is(c"hwdec-current", c"no"),
            opaque_playlist: engine.private_path(),
        }
    }

    fn status(&self) -> AndroidPlaybackStatus {
        let engine = &self.engine;
        let phase = if self.terminal {
            AndroidPlaybackPhase::Failed
        } else if engine.flag(c"pause") {
            AndroidPlaybackPhase::Paused
        } else if self.render.presented() == 0 {
            AndroidPlaybackPhase::Starting
        } else {
            AndroidPlaybackPhase::Playing
        };
        let audio = if engine.has_string(c"current-ao") {
            AndroidPlaybackAudio::BundledDecoder
        } else if self.render.presented() == 0 {
            AndroidPlaybackAudio::Pending
        } else {
            AndroidPlaybackAudio::Absent
        };
        AndroidPlaybackStatus::from_sample(
            phase,
            self.render.presented(),
            engine.integer(c"decoder-frame-drop-count").unwrap_or(0)
                + engine.integer(c"frame-drop-count").unwrap_or(0),
            (engine.number(c"demuxer-cache-duration") * 1_000.) as u64,
            engine.flag(c"mute") || engine.number(c"volume") <= 0.,
            audio,
        )
    }
}

struct Worker {
    playback: Weak<PlaybackManager>,
    runtime: Handle,
    backend: Backend,
    latest: Arc<Publication>,
    events: Arc<Events>,
    binding: Option<Binding>,
}

impl Worker {
    fn run(&mut self, mut inbox: mpsc::Receiver<Operation>, shutdown: CancellationToken) {
        while !shutdown.is_cancelled() {
            // Commands, client events and render state changes wake this worker.
            // Limit command work per pass so layout updates cannot starve rendering.
            for _ in 0..16 {
                self.retire_cancelled();
                match inbox.try_recv() {
                    Ok(operation) => operation(self),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => return,
                }
            }
            self.retire_cancelled();
            if let Some(event) = self.event() {
                self.emit(event);
            }
            if inbox.is_empty() && !shutdown.is_cancelled() {
                thread::park();
            }
        }
    }

    fn start(
        &mut self,
        key: TvPresentationKey,
        viewport: AndroidPlaybackViewport,
        controls: AndroidPlaybackControls,
        cancelled: CancellationToken,
    ) -> Result<(), AndroidPlaybackError> {
        if self.binding.is_some() || cancelled.is_cancelled() {
            return Err(AndroidPlaybackError);
        }
        let engine = Engine::new(
            self.playback.clone(),
            self.runtime.clone(),
            key.identity.clone(),
            self.backend,
        )?;
        engine.controls(controls)?;
        let render = RenderWorker::spawn(
            engine.render_handle(),
            self.backend,
            key.clone(),
            viewport,
            self.latest.clone(),
        )?;
        let binding = Binding {
            key,
            engine,
            render,
            cancelled,
            first_picture: false,
            terminal: false,
        };
        binding.engine.load()?;
        self.binding = Some(binding);
        Ok(())
    }

    fn retire_cancelled(&mut self) {
        if let Some(binding) = self.binding.as_ref()
            && binding.cancelled.is_cancelled()
        {
            let key = binding.key.clone();
            self.stop(&key, TvPresentationStop::ClearPicture);
        }
    }

    fn stop(&mut self, key: &TvPresentationKey, stop: TvPresentationStop) {
        if self
            .binding
            .as_ref()
            .is_some_and(|binding| binding.key == *key)
        {
            self.binding.take();
        }
        if stop == TvPresentationStop::ClearPicture {
            self.latest.clear(key);
        }
    }

    fn binding_mut(
        &mut self,
        key: &TvPresentationKey,
    ) -> Result<&mut Binding, AndroidPlaybackError> {
        self.binding
            .as_mut()
            .filter(|binding| binding.key == *key)
            .ok_or(AndroidPlaybackError)
    }

    fn event(&mut self) -> Option<TvPresentationEvent> {
        let binding = self.binding.as_mut()?;
        if binding.terminal {
            return None;
        }
        loop {
            let event = match binding.engine.event() {
                Event::Pending => break,
                Event::Other => continue,
                Event::Ended => TvPresentationEvent::Ended,
                Event::Failed => TvPresentationEvent::StreamInterrupted,
            };
            binding.terminal = true;
            return Some(event);
        }
        if binding.render.failed() {
            binding.terminal = true;
            return Some(TvPresentationEvent::PlayerUnavailable);
        }
        if !binding.first_picture && binding.render.presented() > 0 {
            binding.first_picture = true;
            return Some(TvPresentationEvent::FirstFrame);
        }
        None
    }

    fn emit(&self, event: TvPresentationEvent) {
        let Some(binding) = self.binding.as_ref() else {
            return;
        };
        let Some(sender) = self.events.get().and_then(mpsc::WeakSender::upgrade) else {
            return;
        };
        let command = TvPlaybackCommand::PresentationEvent {
            key: binding.key.clone(),
            event,
        };
        if let Err(mpsc::error::TrySendError::Full(command)) = sender.try_send(command) {
            self.runtime.spawn(async move {
                let _ = sender.send(command).await;
            });
        }
    }
}
