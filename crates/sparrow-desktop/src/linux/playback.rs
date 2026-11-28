use std::{
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui::{App, RenderImage, WindowHandle};
use sparrow_guide_ui::{GuideSurface, HostEvent, PlaybackHost};
use sparrow_installed_catalog::InstalledCatalog;
use sparrow_installed_playback::{
    PlaybackManager, android::AndroidPlaybackViewport, noop_screen_wake, tv::TvPlaybackRunner,
};
use sparrow_mpv_player::{Backend, MpvPresentation};
use sparrow_source_http::HttpPlaybackAccess;
use tokio::sync::watch;

#[derive(Default)]
pub(super) struct DesktopPlayback {
    runner: OnceLock<TvPlaybackRunner>,
    player: watch::Sender<Option<Weak<MpvPresentation>>>,
    ambient_epoch: AtomicU64,
}

impl PlaybackHost for DesktopPlayback {
    fn start(&self, catalog: &InstalledCatalog) -> Option<TvPlaybackRunner> {
        let core = catalog.core().clone();
        let manager = Arc::new(PlaybackManager::new(
            core.clone(),
            HttpPlaybackAccess::new().ok()?,
            catalog.private_root().to_path_buf(),
            noop_screen_wake(),
        ));
        let player = Arc::new(MpvPresentation::new(&manager, Backend::Software).ok()?);
        let runner = TvPlaybackRunner::spawn(core, manager, player.clone()).ok()?;
        player.connect(&runner);
        self.runner.set(runner.clone()).ok()?;
        self.player.send_replace(Some(Arc::downgrade(&player)));
        Some(runner)
    }

    fn embedded_picture(&self) -> bool {
        true
    }

    fn set_picture_bounds(&self, id: u64, docked: Option<[i32; 4]>, fullscreen: Option<[i32; 4]>) {
        let viewport = |[_, _, width, height]: [i32; 4], fullscreen| {
            AndroidPlaybackViewport::parse(
                0,
                0,
                width.max(0) as u32,
                height.max(0) as u32,
                fullscreen,
            )
            .ok()
        };
        if let Some(runner) = self.runner.get() {
            runner.set_viewports(
                id,
                docked.and_then(|rect| viewport(rect, false)),
                fullscreen.and_then(|rect| viewport(rect, true)),
            );
        }
    }

    fn set_ambient_sampling(&self, _id: u64, epoch: u64) {
        self.ambient_epoch.store(epoch, Ordering::Relaxed);
    }

    fn picture_reveal_duration(&self) -> Duration {
        Duration::from_millis(200)
    }
}

impl DesktopPlayback {
    pub(super) fn player(&self) -> Option<Arc<MpvPresentation>> {
        self.player.borrow().as_ref()?.upgrade()
    }

    pub(super) fn present(self: Arc<Self>, window: WindowHandle<GuideSurface>, cx: &App) {
        let mut player = self.player.subscribe();
        let background = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let Ok(player) = player
                .wait_for(Option::is_some)
                .await
                .map(|player| player.clone())
            else {
                return;
            };
            let Some(mut ready) = player
                .as_ref()
                .and_then(Weak::upgrade)
                .map(|p| p.subscribe_frames())
            else {
                return;
            };
            let mut serial = None;
            loop {
                ready.borrow_and_update();
                let frame = player
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .and_then(|p| p.latest_frame());
                let next = frame.as_ref().map(|frame| frame.serial);
                if next != serial {
                    serial = next;
                    let epoch = self.ambient_epoch.load(Ordering::Relaxed);
                    let ambient = frame.as_ref().map(|frame| frame.ambient);
                    let picture = if let Some(frame) = frame {
                        background
                            .spawn(async move {
                                let pixels = image::RgbaImage::from_raw(
                                    frame.width,
                                    frame.height,
                                    frame.bgra.clone(),
                                )?;
                                Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
                            })
                            .await
                    } else {
                        None
                    };
                    if cx
                        .update(|cx| {
                            window.update(cx, |view, window, cx| {
                                view.host_event(HostEvent::LivePicture(picture), window, cx);
                                if let Some(colors) = ambient.filter(|_| epoch != 0) {
                                    view.host_event(
                                        HostEvent::Ambient { epoch, colors },
                                        window,
                                        cx,
                                    );
                                }
                            })
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                if ready.changed().await.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}
