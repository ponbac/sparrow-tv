//! Process lifetime for Android TV playback and its Media3 bridge.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use sparrow_guide_ui::PlaybackHost;
use sparrow_installed_catalog::InstalledCatalog;
use sparrow_installed_playback::{
    PlaybackManager,
    android::{AndroidMedia3Reader, AndroidPlaybackViewport},
    tv::{TvPlaybackCommand, TvPlaybackPhase, TvPlaybackRunner},
};
use sparrow_source_http::HttpPlaybackAccess;
use tokio::runtime::Handle;

use crate::android_player::{self, AndroidPlayer};

static PLAYBACK: OnceLock<Playback> = OnceLock::new();

pub(super) struct Playback {
    pub(super) runner: TvPlaybackRunner,
    pub(super) player: Arc<AndroidPlayer>,
    pub(super) reader: AndroidMedia3Reader,
    runtime: Handle,
    _manager: Arc<PlaybackManager>,
}

pub(super) struct ShieldPlayback;

impl PlaybackHost for ShieldPlayback {
    fn start(&self, catalog: &InstalledCatalog) -> Option<TvPlaybackRunner> {
        start(catalog);
        get().map(|playback| playback.runner.clone())
    }

    fn host_active(&self, id: u64, active: bool) {
        if let Some(playback) = get() {
            playback.player.host_active(id, active);
        }
    }

    fn host_destroyed(&self, id: u64) {
        if let Some(playback) = get() {
            playback.player.host_destroyed(id);
        }
    }

    fn set_picture_bounds(&self, id: u64, docked: Option<[i32; 4]>, fullscreen: Option<[i32; 4]>) {
        let bounds = |[left, top, width, height]: [i32; 4], fullscreen| {
            AndroidPlaybackViewport::parse(
                left.max(0) as u32,
                top.max(0) as u32,
                width.max(0) as u32,
                height.max(0) as u32,
                fullscreen,
            )
            .ok()
        };
        if let Some(playback) = get() {
            playback.runner.set_viewports(
                id,
                docked.and_then(|rect| bounds(rect, false)),
                fullscreen.and_then(|rect| bounds(rect, true)),
            );
        }
    }

    fn set_ambient_sampling(&self, id: u64, epoch: u64) {
        android_player::set_ambient_sampling(id, epoch);
    }

    fn capture_flight_still(&self, id: u64, request: u64, size: [u32; 2]) -> bool {
        android_player::capture_flight_still(id, request, size);
        true
    }

    fn picture_reveal_duration(&self) -> Duration {
        crate::picture_reveal_duration()
    }
}

fn start(catalog: &InstalledCatalog) {
    let core = catalog.core().clone();
    let Ok(access) = HttpPlaybackAccess::new() else {
        log::warn!("Shield playback transport was unavailable");
        return;
    };
    let player = Arc::new(AndroidPlayer::default());
    let manager = Arc::new(PlaybackManager::new(
        core.clone(),
        access,
        catalog.private_root().to_path_buf(),
        player.clone(),
    ));
    let Ok(runner) = TvPlaybackRunner::spawn(core, manager.clone(), player.clone()) else {
        log::warn!("Shield playback runner was unavailable");
        return;
    };
    let runtime = Handle::current();
    let reader = AndroidMedia3Reader::new(runtime.clone(), Arc::downgrade(&manager));
    let _ = PLAYBACK.set(Playback {
        runner,
        player,
        reader,
        runtime,
        _manager: manager,
    });
}

pub(super) fn get() -> Option<&'static Playback> {
    PLAYBACK.get()
}

pub(super) fn diagnostics() -> (&'static str, Option<u32>, bool, u64) {
    let Some(playback) = get() else {
        return ("Unavailable", None, false, 0);
    };
    let updates = playback.runner.subscribe();
    let snapshot = updates.borrow();
    let phase = match snapshot.phase {
        TvPlaybackPhase::Idle => "Idle",
        TvPlaybackPhase::Starting => "Starting",
        TvPlaybackPhase::Playing => "Playing",
        TvPlaybackPhase::Pausing => "Pausing",
        TvPlaybackPhase::Paused(_) => "Paused",
        TvPlaybackPhase::ReplacingAudio => "ReplacingAudio",
        TvPlaybackPhase::Recovering { .. } => "Recovering",
        TvPlaybackPhase::Stopping => "Stopping",
        TvPlaybackPhase::Failed { .. } => "Failed",
    };
    let channel = snapshot.selected.as_ref().map(|channel| channel.number);
    (
        phase,
        channel,
        snapshot.controls.fullscreen,
        snapshot.status.map_or(0, |status| status.decoded_frames()),
    )
}

impl Playback {
    /// Lifecycle and final player callbacks must reach the bounded runner even
    /// if a burst of remote presses occupies its queue.
    pub(super) fn send_critical(&self, command: TvPlaybackCommand) {
        use tokio::sync::mpsc::error::TrySendError;
        let sender = self.runner.commands();
        if let Err(TrySendError::Full(command)) = sender.try_send(command) {
            self.runtime.spawn(async move {
                let _ = sender.send(command).await;
            });
        }
    }
}
