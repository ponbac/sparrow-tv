use std::{collections::HashMap, future::pending, sync::Arc, time::Duration};

use sparrow_core::{CatalogAccess, LifecycleSignal};
use sparrow_source_http::PlaybackAccessError;
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, sleep_until},
};

use crate::{
    AudioTrackId, NativeStreamHandle, PlaybackManagerError, PlaybackRestartIntent,
    PlaybackSessionId,
    android::{
        AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity,
        AndroidPlaybackPhase, AndroidPlaybackViewport, AndroidPresentationGate,
    },
};

use super::{
    TvHostId, TvPauseCause, TvPlaybackCommand as Command, TvPlaybackFailure as Failure,
    TvPlaybackPhase as Phase, TvPlaybackSnapshot as Snapshot, TvPresentation, TvPresentationEvent,
    TvPresentationKey, TvPresentationStop, TvRecoveryFrame, TvStillHandoff, TvTransportToken,
    TvTuneTarget,
    transport::{Open, Opened, Opening, Transport},
};

const RECOVERY: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
const ZAP_DELAY: Duration = Duration::from_millis(350);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const STALL_TIMEOUT: Duration = Duration::from_secs(10);
const EARLY_STALL: Duration = Duration::from_secs(20);
const FIRST_STILL_DELAY: Duration = Duration::from_millis(825);
const STILL_INTERVAL: Duration = Duration::from_secs(2);
const CAPTURE_DEADLINE: Duration = Duration::from_millis(180);
const STABLE_TIME: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct PendingViewports {
    slots: std::sync::Mutex<HashMap<TvHostId, Viewports>>,
    ready: tokio::sync::Notify,
}

impl PendingViewports {
    pub(super) fn set(
        &self,
        host: TvHostId,
        docked: Option<AndroidPlaybackViewport>,
        fullscreen: Option<AndroidPlaybackViewport>,
    ) {
        let mut slots = self.slots.lock().unwrap();
        let wake = slots.is_empty();
        slots.insert(host, Viewports { docked, fullscreen });
        if wake {
            self.ready.notify_one();
        }
    }
}

struct Viewports {
    docked: Option<AndroidPlaybackViewport>,
    fullscreen: Option<AndroidPlaybackViewport>,
}

struct Session {
    id: PlaybackSessionId,
    target: TvTuneTarget,
    last_handle: Option<NativeStreamHandle>,
    opening: Option<Opening>,
    opened: Option<AndroidPlaybackIdentity>,
    // Retain the key when the view has stopped but is still showing a paused frame.
    picture: Option<TvPresentationKey>,
    attempted_picture: Option<TvPresentationKey>,
    bound: bool,
    opens: u32,
    first_frame: Option<Instant>,
    last_frame: Option<Instant>,
    decoded: u64,
    sampled_at: Option<Instant>,
    clean_frame: Option<Arc<TvRecoveryFrame>>,
}

impl Session {
    fn new(id: PlaybackSessionId, target: TvTuneTarget) -> Self {
        Self {
            id,
            target,
            last_handle: None,
            opening: None,
            opened: None,
            picture: None,
            attempted_picture: None,
            bound: false,
            opens: 0,
            first_frame: None,
            last_frame: None,
            decoded: 0,
            sampled_at: None,
            clean_frame: None,
        }
    }

    fn active_picture(&self) -> Option<&TvPresentationKey> {
        self.picture.as_ref().filter(|_| self.bound)
    }

    fn pictures_to_release(&self, stop: TvPresentationStop) -> [Option<TvPresentationKey>; 2] {
        match stop {
            TvPresentationStop::RetainPicture => [self.active_picture().cloned(), None],
            TvPresentationStop::ClearPicture => {
                [self.attempted_picture.clone(), self.picture.clone()]
            }
        }
    }
}

pub(super) struct Owner {
    catalog: Option<CatalogAccess>,
    transport: Arc<dyn Transport>,
    presentation: Arc<dyn TvPresentation>,
    gate: AndroidPresentationGate,
    pub(super) published: watch::Sender<Snapshot>,
    snapshot: Snapshot,
    session: Option<Session>,
    nonce: u128,
    sequence: u64,
    transport_revision: u64,
    reported_visible: bool,
    started_hosts: Vec<TvHostId>,
    pub(super) pending_viewports: Arc<PendingViewports>,
    viewports: HashMap<TvHostId, Viewports>,
    viewport_pending: bool,
    last_host: Option<TvHostId>,
    zap_at: Option<Instant>,
    retry_at: Option<Instant>,
    start_by: Option<Instant>,
    status_at: Option<Instant>,
    audio_request: Option<AudioTrackId>,
    source_change: Option<oneshot::Receiver<()>>,
}

impl Owner {
    pub(super) fn new(
        catalog: Option<CatalogAccess>,
        transport: Arc<dyn Transport>,
        presentation: Arc<dyn TvPresentation>,
        nonce: u128,
    ) -> Self {
        Self {
            catalog,
            transport,
            presentation,
            gate: AndroidPresentationGate::default(),
            published: watch::Sender::new(Snapshot::default()),
            snapshot: Snapshot::default(),
            session: None,
            nonce,
            sequence: 0,
            transport_revision: 0,
            reported_visible: false,
            started_hosts: Vec::new(),
            pending_viewports: Arc::default(),
            viewports: HashMap::new(),
            viewport_pending: false,
            last_host: None,
            zap_at: None,
            retry_at: None,
            start_by: None,
            status_at: None,
            audio_request: None,
            source_change: None,
        }
    }

    pub(super) async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let pending = Arc::clone(&self.pending_viewports);
        loop {
            self.release_completed_source_change();
            // Drain actions queued during cleanup before starting a replacement.
            for _ in 0..32 {
                let Ok(command) = commands.try_recv() else {
                    break;
                };
                self.command(command).await;
            }
            if commands.is_closed() && commands.is_empty() {
                break;
            }
            self.publish();
            if self.drive().await {
                continue;
            }
            self.publish();
            let deadline = self.next_deadline();
            let opening = self
                .session
                .as_mut()
                .and_then(|session| session.opening.as_mut());
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                () = pending.ready.notified() => {
                    let previous = self.current_viewport();
                    self.viewports.extend(pending.slots.lock().unwrap().drain());
                    if self.current_viewport() != previous {
                        self.update_viewport().await;
                    }
                }
                result = opened(opening) => self.complete_open(result).await,
                () = deadline_elapsed(deadline) => self.on_deadline(Instant::now()).await,
                () = released(self.source_change.as_mut()) => self.source_change = None,
            }
        }
        let _ = self.stop_session().await;
    }

    fn publish(&self) {
        self.published.send_if_modified(|snapshot| {
            // Sampled status is available to diagnostics on demand.
            snapshot.status = self.snapshot.status;
            if snapshot == &self.snapshot {
                return false;
            }
            *snapshot = self.snapshot.clone();
            true
        });
    }

    fn release_completed_source_change(&mut self) {
        if self.source_change.as_mut().is_some_and(|receiver| {
            !matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            )
        }) {
            self.source_change = None;
        }
    }

    async fn command(&mut self, command: Command) {
        self.release_completed_source_change();
        match command {
            Command::HostVisibility {
                host,
                started,
                applied,
            } => {
                let result = if started {
                    self.host_started(host).await
                } else {
                    self.host_stopped(host, false, false).await
                };
                let _ = applied.send(result);
            }
            Command::HostStarted(host) => {
                let _ = self.host_started(host).await;
            }
            Command::HostStopped(host) => {
                let _ = self.host_stopped(host, false, false).await;
            }
            Command::HostFinished(host) => {
                let _ = self.host_stopped(host, true, true).await;
            }
            Command::HostDestroyed { host, finishing } => {
                let _ = self.host_stopped(host, finishing, true).await;
            }
            Command::SetViewports {
                host,
                docked,
                fullscreen,
            } => {
                let previous = self.current_viewport();
                self.viewports
                    .insert(host, Viewports { docked, fullscreen });
                if self.current_viewport() != previous {
                    self.update_viewport().await;
                }
            }
            Command::Tune { target, repeat } => {
                if !self.can_tune() {
                    return;
                }
                if self.same_channel(&target) {
                    if !repeat {
                        self.set_fullscreen(true).await;
                    }
                } else {
                    self.cancel_zap();
                    self.select(target, self.snapshot.controls.fullscreen);
                }
            }
            Command::Zap { target, revision } => {
                if revision == self.snapshot.zap_revision
                    && self.can_tune()
                    && self.snapshot.selected.is_some()
                {
                    self.snapshot.pending_zap = Some(target);
                    self.zap_at = Some(Instant::now() + ZAP_DELAY);
                }
            }
            Command::Pause => self.pause(TvPauseCause::User).await,
            Command::Resume => {
                if matches!(self.snapshot.phase, Phase::Paused(_)) {
                    self.ready_to_resume();
                }
            }
            Command::Restart => self.restart().await,
            command @ (Command::Stop | Command::StopConfirmed(_)) => {
                self.cancel_zap();
                self.snapshot.selected = None;
                self.audio_request = None;
                self.cancel_deadlines();
                if self.session.is_some() {
                    self.snapshot.phase = Phase::Stopping;
                } else {
                    self.set_idle();
                }
                if let Command::StopConfirmed(stopped) = command {
                    self.publish();
                    self.drive().await;
                    self.publish();
                    let result = if self.snapshot.phase == Phase::Idle {
                        Ok(())
                    } else {
                        Err(Failure::CleanupUnconfirmed)
                    };
                    let _ = stopped.send(result);
                }
            }
            Command::SelectAudio { token, track } => {
                if self.snapshot.transport_token == Some(token) {
                    self.select_audio(track).await;
                }
            }
            Command::SetVolume(volume) => {
                if volume.is_finite() {
                    self.snapshot.controls.volume = volume.clamp(0., 1.);
                    self.update_controls().await;
                }
            }
            Command::ToggleMute => {
                self.snapshot.controls.muted = !self.snapshot.controls.muted;
                self.update_controls().await;
            }
            Command::Fullscreen => self.set_fullscreen(true).await,
            Command::ExitFullscreen => self.set_fullscreen(false).await,
            Command::PresentationEvent { key, event } => {
                self.presentation_event(&key, event, Instant::now()).await
            }
            Command::StopForSourceChange { released, resume } => {
                if self.source_change.is_some() {
                    let _ = released.send(Err(Failure::CleanupUnconfirmed));
                    return;
                }
                self.cancel_zap();
                let result = self.stop_session().await;
                if result.is_ok() {
                    self.snapshot.last_played = None;
                    self.set_idle();
                    self.source_change = Some(resume);
                } else {
                    self.cleanup_unconfirmed();
                }
                let _ = released.send(result);
            }
        }
    }

    /// Runs reconciliation work that can change the inputs of the next loop.
    async fn drive(&mut self) -> bool {
        if self.viewport_pending {
            self.update_viewport().await;
        }
        if self.session.as_ref().is_some_and(|session| {
            self.snapshot.phase == Phase::Stopping
                || self
                    .snapshot
                    .selected
                    .as_ref()
                    .is_none_or(|selected| selected.channel != session.target.channel)
        }) {
            if self.cleanup_is_blocked() {
                return false;
            }
            if self.stop_session().await.is_err() {
                self.cleanup_unconfirmed();
            } else if self.snapshot.selected.is_none() {
                self.set_idle();
            } else {
                self.ready_to_resume();
            }
            return true;
        }
        if self.retry_at.is_some()
            || !matches!(
                self.snapshot.phase,
                Phase::Starting | Phase::ReplacingAudio | Phase::Recovering { .. }
            )
        {
            return false;
        }
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.opening.is_some() || session.bound)
        {
            return false;
        }
        let Some(viewport) = self.current_viewport() else {
            if self.snapshot.visible_host.is_some() && self.start_by.is_none() {
                self.start_by = Some(Instant::now() + START_TIMEOUT);
            }
            return false;
        };
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.opened.is_some())
        {
            self.present(viewport).await;
        } else {
            self.begin_open().await;
        }
        true
    }

    async fn begin_open(&mut self) {
        let Some(target) = self.snapshot.selected.clone() else {
            self.set_idle();
            return;
        };
        if self.session.is_none() {
            let Some(sequence) = self.sequence.checked_add(1) else {
                self.set_failed(Failure::PlayerUnavailable);
                return;
            };
            self.sequence = sequence;
            let wire = format!("play1_{:032x}_{sequence:x}", self.nonce);
            let Ok(id) = PlaybackSessionId::parse(wire) else {
                self.set_failed(Failure::PlayerUnavailable);
                return;
            };
            self.session = Some(Session::new(id, target));
        }
        let Some(session) = &mut self.session else {
            return;
        };
        let audio = self.audio_request.take().zip(session.last_handle.clone());
        let request = if let Some((track, handle)) = audio {
            Open::Restart(handle, PlaybackRestartIntent::SelectAudio(track))
        } else if session.opens == 0 {
            Open::Start(session.target.channel.clone())
        } else {
            Open::Reopen
        };
        session.opens = session.opens.saturating_add(1);
        session.clean_frame = None;
        session.sampled_at = None;
        session.first_frame = None;
        session.last_frame = None;
        session.decoded = 0;
        self.start_by = None;
        self.snapshot.status = None;
        // Wait for actor admission, then wait on the reply alongside TV commands.
        // A suspension must follow this open in the actor's command queue.
        match Arc::clone(&self.transport)
            .admit_open(session.id.clone(), request)
            .await
        {
            Ok(response) => session.opening = Some(response),
            Err(error) => self.complete_open(Err(error)).await,
        }
    }

    async fn complete_open(&mut self, result: Result<Opened, PlaybackManagerError>) {
        if let Some(session) = &mut self.session {
            session.opening = None;
            match result {
                Ok(opened) => {
                    session.last_handle = Some(opened.identity.stream_handle().clone());
                    self.snapshot.audio = Some(opened.audio);
                    session.opened = Some(opened.identity);
                }
                Err(error) => {
                    let (failure, retryable, early_timeout) = classify(error);
                    self.fail(failure, retryable, early_timeout, Instant::now())
                        .await;
                }
            }
        }
    }

    async fn present(&mut self, viewport: AndroidPlaybackViewport) {
        let Some(host) = self.snapshot.visible_host else {
            return;
        };
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(opened) = session.opened.take() else {
            return;
        };
        // A rejected start can leave the previous picture visible. Keep both
        // keys until the new presentation is acknowledged or release settles.
        session.attempted_picture = Some(TvPresentationKey {
            host,
            identity: opened.clone(),
        });
        let transport = Arc::clone(&self.transport);
        let presentation = Arc::clone(&self.presentation);
        let controls = self.controls();
        let result = self
            .gate
            .validate_then_start(
                opened,
                move |identity| {
                    transport.validate(
                        identity.session_id().clone(),
                        identity.stream_handle().clone(),
                    )
                },
                move |identity| {
                    presentation.start(TvPresentationKey { host, identity }, viewport, controls)
                },
            )
            .await;
        if let Err(error) = result {
            let now = Instant::now();
            if matches!(error, PlaybackManagerError::Cancelled) {
                self.fail(Failure::PlayerUnavailable, false, false, now)
                    .await;
            } else {
                self.presentation_unavailable(now).await;
            }
            return;
        }
        if let Some(session) = &mut self.session {
            session.picture = session.attempted_picture.take();
            session.bound = true;
        }
        let Some(revision) = self.transport_revision.checked_add(1) else {
            self.finish_failure(Failure::PlayerUnavailable).await;
            return;
        };
        self.transport_revision = revision;
        self.snapshot.transport_token = Some(TvTransportToken(revision));
        if !matches!(self.snapshot.phase, Phase::Recovering { .. }) {
            self.snapshot.phase = Phase::Starting;
        }
        let now = Instant::now();
        self.start_by = Some(now + START_TIMEOUT);
        self.status_at = Some(now + STATUS_INTERVAL);
    }

    async fn stop_session(&mut self) -> Result<(), Failure> {
        self.cancel_deadlines();
        self.audio_request = None;
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        // A transport may have published its handle just before Stop arrived.
        // Cancel a pending open first, then harvest any completed handle so the
        // terminal stop uses the exact actor owner.
        if session.opening.is_some() {
            Arc::clone(&self.transport)
                .suspend(session.id.clone(), None)
                .await
                .map_err(|_| Failure::CleanupUnconfirmed)?;
            settle_open(session).await;
        }
        let transport = Arc::clone(&self.transport);
        let presentation = Arc::clone(&self.presentation);
        let session_id = session.id.clone();
        let handle = session.last_handle.clone();
        let pictures = session.pictures_to_release(TvPresentationStop::ClearPicture);
        self.gate
            .stop_after_transport(
                move || transport.stop(session_id, handle),
                move || stop_pictures(presentation, pictures, TvPresentationStop::ClearPicture),
            )
            .await
            .map_err(|_| Failure::CleanupUnconfirmed)?;
        self.session = None;
        self.snapshot.status = None;
        self.snapshot.audio = None;
        Ok(())
    }

    async fn suspend_session(
        &mut self,
        stop: TvPresentationStop,
        exact: Option<NativeStreamHandle>,
    ) -> Result<(), Failure> {
        self.cancel_deadlines();
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let transport = Arc::clone(&self.transport);
        let presentation = Arc::clone(&self.presentation);
        let session_id = session.id.clone();
        let pictures = session.pictures_to_release(stop);
        self.gate
            .stop_after_transport(
                move || transport.suspend(session_id, exact),
                move || stop_pictures(presentation, pictures, stop),
            )
            .await
            .map_err(|_| Failure::CleanupUnconfirmed)?;
        settle_open(session).await;
        session.opened = None;
        session.bound = false;
        if stop == TvPresentationStop::ClearPicture {
            session.attempted_picture = None;
            session.picture = None;
        }
        Ok(())
    }

    async fn pause(&mut self, cause: TvPauseCause) {
        if self.snapshot.selected.is_none()
            || matches!(
                self.snapshot.phase,
                Phase::Failed { .. } | Phase::Stopping | Phase::Idle
            )
        {
            return;
        }
        self.cancel_zap();
        if matches!(self.snapshot.phase, Phase::Paused(_)) {
            if cause == TvPauseCause::User {
                self.snapshot.phase = Phase::Paused(cause);
            }
            return;
        }
        self.audio_request = None;
        self.snapshot.phase = Phase::Pausing;
        if self
            .suspend_session(TvPresentationStop::RetainPicture, None)
            .await
            .is_err()
        {
            self.cleanup_unconfirmed();
            return;
        }
        self.snapshot.phase = Phase::Paused(cause);
    }

    async fn restart(&mut self) {
        if self.snapshot.selected.is_none()
            || self.cleanup_is_blocked()
            || self.source_change.is_some()
        {
            return;
        }
        self.cancel_zap();
        self.audio_request = None;
        self.snapshot.recovery_attempts = 0;
        if self
            .suspend_session(TvPresentationStop::RetainPicture, None)
            .await
            .is_err()
        {
            self.cleanup_unconfirmed();
        } else {
            self.ready_to_resume();
        }
    }

    async fn select_audio(&mut self, track: AudioTrackId) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.bound || self.cleanup_is_blocked() {
            return;
        }
        let Some(handle) = session.last_handle.clone() else {
            return;
        };
        let Some(audio) = &self.snapshot.audio else {
            return;
        };
        if !audio
            .tracks
            .iter()
            .any(|item| item.id() == &track && !item.selected())
        {
            return;
        }
        self.cancel_zap();
        self.snapshot.phase = Phase::ReplacingAudio;
        if self
            .suspend_session(TvPresentationStop::RetainPicture, Some(handle))
            .await
            .is_err()
        {
            self.cleanup_unconfirmed();
        } else {
            self.audio_request = Some(track);
        }
    }

    async fn host_started(&mut self, host: TvHostId) -> Result<(), Failure> {
        if self.started_hosts.contains(&host) {
            return Ok(());
        }
        let previous = self.snapshot.visible_host;
        self.started_hosts.push(host);
        self.snapshot.visible_host = Some(host);
        self.last_host = Some(host);
        if previous.is_none() {
            if self
                .gate
                .transport_transition(|| Arc::clone(&self.transport).lifecycle(true))
                .await
                .is_err()
            {
                self.started_hosts.retain(|id| *id != host);
                self.snapshot.visible_host = previous;
                return Err(Failure::PlayerUnavailable);
            }
            if let Some(catalog) = &self.catalog {
                catalog.report_lifecycle(LifecycleSignal::Resumed);
            }
            self.reported_visible = true;
        } else {
            self.handoff_picture().await?;
        }
        if self.snapshot.phase == Phase::Paused(TvPauseCause::Visibility) {
            self.ready_to_resume();
        }
        Ok(())
    }

    async fn host_stopped(
        &mut self,
        host: TvHostId,
        finishing: bool,
        destroyed: bool,
    ) -> Result<(), Failure> {
        let previous = self.snapshot.visible_host;
        let owns_session =
            previous == Some(host) || (previous.is_none() && self.last_host == Some(host));
        self.started_hosts.retain(|id| *id != host);
        self.snapshot.visible_host = self.started_hosts.last().copied();
        if destroyed {
            self.pending_viewports.slots.lock().unwrap().remove(&host);
            self.viewports.remove(&host);
        }
        if finishing && owns_session {
            if self.stop_session().await.is_err() {
                self.cleanup_unconfirmed();
                return Err(Failure::CleanupUnconfirmed);
            }
            self.cancel_zap();
            self.set_idle();
        }
        if previous == self.snapshot.visible_host
            && (self.snapshot.visible_host.is_some() || !self.reported_visible)
        {
            return Ok(());
        }
        if let Some(next) = self.snapshot.visible_host {
            self.last_host = Some(next);
            return self.handoff_picture().await;
        }
        self.cancel_zap();
        self.cancel_deadlines();
        self.audio_request = None;
        let pictures = self
            .session
            .as_ref()
            .map_or_else(Default::default, |session| {
                session.pictures_to_release(TvPresentationStop::ClearPicture)
            });
        let transport = Arc::clone(&self.transport);
        let presentation = Arc::clone(&self.presentation);
        if self
            .gate
            .stop_after_transport(
                move || transport.lifecycle(false),
                move || stop_pictures(presentation, pictures, TvPresentationStop::ClearPicture),
            )
            .await
            .is_err()
        {
            self.cleanup_unconfirmed();
            return Err(Failure::CleanupUnconfirmed);
        }
        if let Some(session) = &mut self.session {
            settle_open(session).await;
            session.opened = None;
            session.attempted_picture = None;
            session.picture = None;
            session.bound = false;
        }
        if let Some(catalog) = &self.catalog {
            catalog.report_lifecycle(LifecycleSignal::Suspended);
        }
        self.reported_visible = false;
        if matches!(
            self.snapshot.phase,
            Phase::Starting
                | Phase::Playing
                | Phase::Pausing
                | Phase::ReplacingAudio
                | Phase::Recovering { .. }
        ) {
            self.snapshot.phase = Phase::Paused(TvPauseCause::Visibility);
        }
        Ok(())
    }

    async fn handoff_picture(&mut self) -> Result<(), Failure> {
        let retry_at = self.retry_at;
        if self.session.is_some()
            && self
                .suspend_session(TvPresentationStop::ClearPicture, None)
                .await
                .is_err()
        {
            self.cleanup_unconfirmed();
            return Err(Failure::CleanupUnconfirmed);
        }
        self.retry_at = retry_at;
        if matches!(
            self.snapshot.phase,
            Phase::Playing | Phase::Starting | Phase::ReplacingAudio
        ) {
            self.ready_to_resume();
        }
        Ok(())
    }

    async fn presentation_event(
        &mut self,
        key: &TvPresentationKey,
        event: TvPresentationEvent,
        now: Instant,
    ) {
        if self.current_picture() != Some(key) {
            return;
        }
        match event {
            TvPresentationEvent::FirstFrame => self.mark_playing(now).await,
            TvPresentationEvent::Ended => self.fail(Failure::Ended, false, false, now).await,
            TvPresentationEvent::StreamInterrupted => {
                self.fail(
                    Failure::StreamInterrupted,
                    true,
                    self.early_decoder_stall(now),
                    now,
                )
                .await
            }
            TvPresentationEvent::PlayerUnavailable => {
                self.fail(Failure::PlayerUnavailable, false, false, now)
                    .await
            }
            TvPresentationEvent::AudioFocusLost => self.pause(TvPauseCause::User).await,
        }
    }

    async fn mark_playing(&mut self, now: Instant) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.first_frame.is_some() {
            return;
        }
        session.first_frame = Some(now);
        session.last_frame = Some(now);
        self.start_by = None;
        if Arc::clone(&self.transport)
            .wake(session.id.clone())
            .await
            .is_err()
        {
            self.fail(Failure::PlayerUnavailable, false, false, now)
                .await;
            return;
        }
        self.snapshot.first_picture.get_or_insert(now.into_std());
        self.snapshot.recovery_frame = None;
        self.snapshot.phase = Phase::Playing;
        self.snapshot.last_played = Some(session.target.clone());
    }

    fn early_decoder_stall(&self, now: Instant) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session.opens == 1
                && session.bound
                && session
                    .first_frame
                    .is_none_or(|first| now.saturating_duration_since(first) < EARLY_STALL)
        })
    }

    async fn presentation_unavailable(&mut self, now: Instant) {
        self.fail(Failure::PresentationUnavailable, true, false, now)
            .await;
    }

    async fn fail(&mut self, failure: Failure, retryable: bool, early: bool, now: Instant) {
        self.cancel_deadlines();
        let downgrade = if early
            && self.snapshot.pending_zap.is_none()
            && self.snapshot.quality_fallback.is_none()
        {
            self.session
                .as_ref()
                .filter(|session| session.opens == 1)
                .and_then(|session| {
                    let target = &session.target;
                    let from = target.quality?;
                    let lower = target
                        .lower
                        .as_ref()
                        .filter(|lower| lower.quality < from && lower.channel != target.channel)?;
                    Some((
                        TvTuneTarget {
                            channel: lower.channel.clone(),
                            number: target.number,
                            quality: Some(lower.quality),
                            lower: None,
                        },
                        (from, lower.quality),
                    ))
                })
        } else {
            None
        };
        if let Some((target, quality)) = downgrade {
            if self.stop_session().await.is_err() {
                self.cleanup_unconfirmed();
                return;
            }
            self.select(target, self.snapshot.controls.fullscreen);
            self.snapshot.quality_fallback = Some(quality);
            return;
        }
        // The host can park a visible picture briefly, for example while the
        // Guide flies a still to fullscreen. A cached frame stays usable then;
        // only a fresh capture needs the picture on screen.
        let visible = self.current_viewport().is_some();
        if retryable
            && self.snapshot.recovery_frame.is_none()
            && let Some(session) = self.session.as_mut()
            && session.first_frame.is_some()
            && let Some(key) = session.active_picture().cloned()
        {
            self.snapshot.recovery_frame = match session.clean_frame.take() {
                Some(frame) => Some(frame),
                None if visible => self.capture_frame(key).await,
                None => None,
            };
        }
        let attempt = self.snapshot.recovery_attempts;
        if let Some(delay) = RECOVERY
            .get(usize::from(attempt))
            .filter(|_| retryable && self.session.is_some())
        {
            self.snapshot.recovery_attempts = attempt + 1;
            self.snapshot.phase = Phase::Recovering {
                attempt: attempt + 1,
                failure,
            };
            let painted = self.wait_for_still().await;
            let stop = if painted
                || self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.attempted_picture.is_some())
            {
                TvPresentationStop::ClearPicture
            } else {
                TvPresentationStop::RetainPicture
            };
            if self.suspend_session(stop, None).await.is_err() {
                self.cleanup_unconfirmed();
                return;
            }
            if self.snapshot.visible_host.is_none() {
                self.snapshot.phase = Phase::Paused(TvPauseCause::Visibility);
                return;
            }
            let Some(session) = &self.session else {
                return;
            };
            if Arc::clone(&self.transport)
                .wake(session.id.clone())
                .await
                .is_err()
            {
                self.finish_failure(Failure::PlayerUnavailable).await;
                return;
            }
            self.retry_at = Some(now.max(Instant::now()) + *delay);
        } else {
            self.finish_failure(failure).await;
        }
    }

    /// Leave the native picture in place until the visible host has drawn its replacement.
    async fn wait_for_still(&mut self) -> bool {
        if self.snapshot.recovery_frame.is_none()
            || self
                .session
                .as_ref()
                .and_then(Session::active_picture)
                .is_none()
        {
            return false;
        }
        let handoff = TvStillHandoff::default();
        self.snapshot.still_handoff = Some(handoff.clone());
        self.publish();
        let painted = tokio::time::timeout(Duration::from_millis(350), handoff.0.notified())
            .await
            .is_ok();
        self.snapshot.still_handoff = None;
        painted
    }

    async fn finish_failure(&mut self, failure: Failure) {
        self.set_failed(failure);
        self.wait_for_still().await;
        if self.stop_session().await.is_err() {
            self.cleanup_unconfirmed();
        }
    }

    async fn on_deadline(&mut self, now: Instant) {
        if self.zap_at.is_some_and(|at| at <= now) {
            self.zap_at = None;
            if let Some(target) = self.snapshot.pending_zap.take()
                && self.can_tune()
                && !self.same_channel(&target)
            {
                self.select(target, self.snapshot.controls.fullscreen);
                return;
            }
        }
        if self.retry_at.is_some_and(|at| at <= now) {
            self.retry_at = None;
            return;
        }
        if self.status_at.is_some_and(|at| at <= now) {
            self.poll_status(now).await;
        }
        if self.start_by.is_some_and(|at| at <= now) {
            if self.current_picture().is_some() {
                self.fail(
                    Failure::StreamInterrupted,
                    true,
                    self.early_decoder_stall(now),
                    now,
                )
                .await;
            } else {
                self.fail(Failure::PlayerUnavailable, false, false, now)
                    .await;
            }
            return;
        }
        if self.stream_stalled(now) {
            self.fail(
                Failure::StreamInterrupted,
                true,
                self.early_decoder_stall(now),
                now,
            )
            .await;
        } else if self.stable_by().is_some_and(|at| at <= now) {
            self.snapshot.recovery_attempts = 0;
        }
    }

    /// Captures run outside the presentation gate: an abandoned capture must
    /// not hold the next status poll, pause or stop until the host answers.
    async fn capture_frame(&mut self, key: TvPresentationKey) -> Option<Arc<TvRecoveryFrame>> {
        tokio::time::timeout(CAPTURE_DEADLINE, self.presentation.capture_frame(key))
            .await
            .ok()
            .and_then(Result::ok)
    }

    async fn poll_status(&mut self, now: Instant) {
        let Some(key) = self.current_picture().cloned() else {
            self.status_at = None;
            return;
        };
        let presentation = Arc::clone(&self.presentation);
        let status_key = key.clone();
        let result = self
            .gate
            .presentation_call(move || presentation.status(status_key))
            .await;
        let status = match result {
            Ok(status) => status,
            Err(_) => {
                self.presentation_unavailable(now).await;
                return;
            }
        };
        self.snapshot.status = Some(status);
        // Align the status poll with the first still so it can sample just
        // after the picture lands, independently of the watchdog cadence.
        let next_poll = now + STATUS_INTERVAL;
        self.status_at = Some(
            self.session
                .as_ref()
                .and_then(|session| session.first_frame)
                .map(|first| first + FIRST_STILL_DELAY)
                .filter(|deadline| *deadline > now)
                .map_or(next_poll, |deadline| deadline.min(next_poll)),
        );
        match status.phase() {
            // A terminal sample can arrive before its exact callback. Leave
            // classification to that event or to the watchdog if it never arrives.
            AndroidPlaybackPhase::Failed | AndroidPlaybackPhase::Stopped => {}
            AndroidPlaybackPhase::Paused => {
                if self.start_by.is_some() {
                    self.start_by = Some(now + START_TIMEOUT);
                }
                if let Some(session) = &mut self.session
                    && session.last_frame.is_some()
                {
                    session.last_frame = Some(now);
                }
            }
            AndroidPlaybackPhase::Starting | AndroidPlaybackPhase::Playing => {
                let visible = self.current_viewport().is_some();
                let mut sample = false;
                if let Some(session) = &mut self.session {
                    let advanced = status.decoded_frames() > session.decoded;
                    sample = visible
                        && self.snapshot.phase == Phase::Playing
                        && match session.sampled_at {
                            // The first-picture callback already confirms a usable surface.
                            // Media3 may batch its next decoded-frame count.
                            None => session
                                .first_frame
                                .is_some_and(|first| now >= first + FIRST_STILL_DELAY),
                            Some(last) => {
                                status.phase() == AndroidPlaybackPhase::Playing
                                    && advanced
                                    && now >= last + STILL_INTERVAL
                            }
                        };
                    if sample {
                        session.sampled_at = Some(now);
                    }
                    if advanced {
                        session.decoded = status.decoded_frames();
                        session.last_frame = Some(now);
                    }
                }
                if sample
                    && let Some(frame) = self.capture_frame(key).await
                    && let Some(session) = self.session.as_mut()
                {
                    session.clean_frame = Some(frame);
                }
                if status.phase() == AndroidPlaybackPhase::Playing && status.decoded_frames() > 0 {
                    self.mark_playing(now).await;
                }
            }
        }
    }

    fn stream_stalled(&self, now: Instant) -> bool {
        self.stall_by().is_some_and(|at| at <= now)
    }

    fn stall_by(&self) -> Option<Instant> {
        let session = self.session.as_ref()?;
        session.active_picture()?;
        Some(session.last_frame? + STALL_TIMEOUT)
    }

    fn stable_by(&self) -> Option<Instant> {
        if self.snapshot.phase != Phase::Playing || self.snapshot.recovery_attempts == 0 {
            return None;
        }
        Some(self.session.as_ref()?.first_frame? + STABLE_TIME)
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.zap_at,
            self.retry_at,
            self.start_by,
            self.status_at,
            self.stall_by(),
            self.stable_by(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn select(&mut self, target: TvTuneTarget, fullscreen: bool) {
        self.cancel_deadlines();
        self.audio_request = None;
        self.snapshot.first_picture = None;
        self.snapshot.recovery_frame = None;
        self.snapshot.selected = Some(target);
        self.snapshot.controls.fullscreen = fullscreen;
        self.snapshot.recovery_attempts = 0;
        self.snapshot.quality_fallback = None;
        if self.session.is_some() {
            self.snapshot.phase = Phase::Stopping;
        } else {
            self.ready_to_resume();
        }
    }

    fn same_channel(&self, target: &TvTuneTarget) -> bool {
        self.snapshot
            .selected
            .as_ref()
            .is_some_and(|selected| selected.channel == target.channel)
    }

    fn can_tune(&self) -> bool {
        self.source_change.is_none() && !self.cleanup_is_blocked()
    }

    fn ready_to_resume(&mut self) {
        self.snapshot.phase = if self.snapshot.visible_host.is_some() {
            Phase::Starting
        } else {
            Phase::Paused(TvPauseCause::Visibility)
        };
    }

    fn set_idle(&mut self) {
        let controls = self.snapshot.controls;
        let visible_host = self.snapshot.visible_host;
        let last_played = self.snapshot.last_played.take();
        self.snapshot = Snapshot {
            controls: super::TvPlaybackControls {
                fullscreen: false,
                ..controls
            },
            visible_host,
            last_played,
            zap_revision: self.snapshot.zap_revision,
            ..Snapshot::default()
        };
        self.cancel_deadlines();
    }

    fn set_failed(&mut self, failure: Failure) {
        self.snapshot.phase = Phase::Failed {
            failure,
            can_restart: failure != Failure::CleanupUnconfirmed,
        };
    }

    fn cleanup_unconfirmed(&mut self) {
        self.cancel_deadlines();
        self.cancel_zap();
        if let Some(session) = &self.session {
            self.snapshot.selected = Some(session.target.clone());
        }
        self.set_failed(Failure::CleanupUnconfirmed);
    }

    fn cleanup_is_blocked(&self) -> bool {
        matches!(
            self.snapshot.phase,
            Phase::Failed {
                failure: Failure::CleanupUnconfirmed,
                ..
            }
        )
    }

    fn cancel_deadlines(&mut self) {
        self.snapshot.transport_token = None;
        self.snapshot.status = None;
        self.retry_at = None;
        self.start_by = None;
        self.status_at = None;
        if let Some(session) = &mut self.session {
            session.last_frame = None;
        }
    }

    fn cancel_zap(&mut self) {
        self.zap_at = None;
        self.snapshot.pending_zap = None;
        self.snapshot.zap_revision = self.snapshot.zap_revision.wrapping_add(1);
    }

    fn current_picture(&self) -> Option<&TvPresentationKey> {
        self.snapshot.transport_token?;
        self.visible_picture_session()?.active_picture()
    }

    fn visible_picture_session(&self) -> Option<&Session> {
        let session = self.session.as_ref()?;
        let key = session.picture.as_ref()?;
        (self.snapshot.visible_host == Some(key.host) && self.same_channel(&session.target))
            .then_some(session)
    }

    fn current_viewport(&self) -> Option<AndroidPlaybackViewport> {
        let viewports = self.viewports.get(&self.snapshot.visible_host?)?;
        if self.snapshot.controls.fullscreen {
            viewports.fullscreen
        } else {
            viewports.docked
        }
    }

    fn controls(&self) -> AndroidPlaybackControls {
        AndroidPlaybackControls::parse(
            f64::from(self.snapshot.controls.volume),
            self.snapshot.controls.muted,
            false,
        )
        .expect("TV playback volume is finite and clamped")
    }

    async fn set_fullscreen(&mut self, fullscreen: bool) {
        if self.snapshot.selected.is_some() && self.snapshot.controls.fullscreen != fullscreen {
            if fullscreen {
                self.cancel_zap();
            }
            let previous = self.current_viewport();
            self.snapshot.controls.fullscreen = fullscreen;
            if self.current_viewport() != previous {
                self.update_viewport().await;
            }
        }
    }

    async fn update_viewport(&mut self) {
        self.viewport_pending = false;
        let Some(session) = self.visible_picture_session() else {
            return;
        };
        let live = session.bound;
        if self.cleanup_is_blocked() || (live && self.snapshot.transport_token.is_none()) {
            // The next reconciliation uses the latest bounds once the picture
            // can move again. A new presentation starts at those bounds itself.
            self.viewport_pending = true;
            return;
        }
        let Some(key) = session.picture.clone() else {
            return;
        };
        let viewport = self.current_viewport();
        let presentation = Arc::clone(&self.presentation);
        let result = self
            .gate
            .presentation_call(move || presentation.set_viewport(key, viewport))
            .await;
        // An Activity can retire a paused picture before the viewport update.
        if live && result.is_err() {
            self.presentation_unavailable(Instant::now()).await;
        }
    }

    async fn update_controls(&mut self) {
        if let Some(key) = self.current_picture().cloned() {
            let presentation = Arc::clone(&self.presentation);
            let controls = self.controls();
            if self
                .gate
                .presentation_call(move || presentation.set_controls(key, controls))
                .await
                .is_err()
            {
                self.presentation_unavailable(Instant::now()).await;
            }
        }
    }
}

async fn settle_open(session: &mut Session) {
    if let Some(opening) = session.opening.take()
        && let Ok(opened) = opening.await
    {
        session.last_handle = Some(opened.identity.stream_handle().clone());
    }
}

async fn opened(opening: Option<&mut Opening>) -> Result<Opened, PlaybackManagerError> {
    match opening {
        Some(opening) => opening.await,
        None => pending().await,
    }
}

async fn deadline_elapsed(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

async fn released(receiver: Option<&mut oneshot::Receiver<()>>) {
    match receiver {
        Some(receiver) => {
            let _ = receiver.await;
        }
        None => pending().await,
    }
}

async fn stop_pictures(
    presentation: Arc<dyn TvPresentation>,
    pictures: [Option<TvPresentationKey>; 2],
    how: TvPresentationStop,
) -> Result<(), AndroidPlaybackError> {
    let mut result = Ok(());
    for key in pictures.into_iter().flatten() {
        result = result.and(presentation.stop(key, how).await);
    }
    result
}

fn classify(error: PlaybackManagerError) -> (Failure, bool, bool) {
    match error {
        PlaybackManagerError::Core(_) => (Failure::ChannelUnavailable, false, false),
        PlaybackManagerError::Access(PlaybackAccessError::Rejected) => {
            (Failure::SourceRejected, false, false)
        }
        PlaybackManagerError::Access(PlaybackAccessError::TimedOut) => {
            (Failure::SourceTimeout, true, true)
        }
        PlaybackManagerError::Access(PlaybackAccessError::Unavailable) => {
            (Failure::SourceUnavailable, true, false)
        }
        PlaybackManagerError::Access(PlaybackAccessError::InvalidResponse) => {
            (Failure::InvalidSource, false, false)
        }
        PlaybackManagerError::Read(_) => (Failure::StreamInterrupted, true, false),
        PlaybackManagerError::TransportStream(error) if error.retryable() => {
            (Failure::StreamInterrupted, true, false)
        }
        PlaybackManagerError::TransportStream(_) => (Failure::UnsupportedMedia, false, false),
        PlaybackManagerError::Mpv(_)
        | PlaybackManagerError::Cancelled
        | PlaybackManagerError::Unavailable => (Failure::PlayerUnavailable, false, false),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
