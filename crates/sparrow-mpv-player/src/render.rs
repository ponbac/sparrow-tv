use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use arc_swap::ArcSwapOption;
use sparrow_installed_playback::{
    android::{AndroidPlaybackError, AndroidPlaybackViewport},
    tv::TvPresentationKey,
};

use crate::{
    Backend, VideoFrame,
    engine::{RenderHandle, Renderer, Update},
    frame::{FrameSize, ambient},
};

#[derive(Default)]
pub(crate) struct Publication {
    current: ArcSwapOption<Picture>,
    serial: AtomicU64,
}

pub(crate) struct Picture {
    key: TvPresentationKey,
    size: AtomicU64,
    visible: AtomicBool,
    pub frame: ArcSwapOption<VideoFrame>,
}

impl Publication {
    pub(crate) fn picture(&self, key: &TvPresentationKey) -> Option<Arc<Picture>> {
        self.current
            .load_full()
            .filter(|picture| picture.key == *key)
    }

    pub(crate) fn visible_frame(&self) -> Option<Arc<VideoFrame>> {
        let current = self.current.load();
        let picture = current
            .as_ref()
            .filter(|picture| picture.visible.load(Ordering::Acquire))?;
        picture.frame.load_full()
    }

    pub(crate) fn clear(&self, key: &TvPresentationKey) {
        // Only the exact binding can publish. Its render worker has joined
        // before clearing, but a replacement binding may publish concurrently.
        if let Some(picture) = self.picture(key) {
            self.current.compare_and_swap(&picture, None);
        }
    }
}

impl Picture {
    fn new(key: TvPresentationKey, size: FrameSize) -> Self {
        Self {
            key,
            size: AtomicU64::new(Self::pack(size)),
            visible: AtomicBool::new(true),
            frame: ArcSwapOption::empty(),
        }
    }

    fn pack(size: FrameSize) -> u64 {
        u64::from(size.width) << 32 | u64::from(size.height)
    }

    fn size(&self) -> FrameSize {
        let value = self.size.load(Ordering::Acquire);
        FrameSize {
            width: (value >> 32) as u32,
            height: value as u32,
        }
    }

    pub(crate) fn set_viewport(&self, viewport: Option<AndroidPlaybackViewport>) {
        if let Some(viewport) = viewport {
            self.size
                .store(Self::pack(viewport.into()), Ordering::Release);
        }
        self.visible.store(viewport.is_some(), Ordering::Release);
    }
}

#[derive(Default)]
struct Progress {
    stop: AtomicBool,
    presented: AtomicU64,
    failed: AtomicBool,
}

pub(crate) struct RenderWorker {
    picture: Arc<Picture>,
    progress: Arc<Progress>,
    join: Option<JoinHandle<()>>,
}

impl RenderWorker {
    pub(crate) fn spawn(
        handle: RenderHandle,
        backend: Backend,
        key: TvPresentationKey,
        viewport: AndroidPlaybackViewport,
        publication: Arc<Publication>,
    ) -> Result<Self, AndroidPlaybackError> {
        let picture = Arc::new(Picture::new(key, viewport.into()));
        let progress = Arc::new(Progress::default());
        let (accept, ready) = mpsc::sync_channel(0);
        let current = picture.clone();
        let status = progress.clone();
        let client = thread::current();
        let join = thread::Builder::new()
            .name("sparrow-mpv-render".into())
            .spawn(move || {
                let Ok(renderer) = handle.create(backend) else {
                    return;
                };
                if accept.send(()).is_err() {
                    return;
                }
                render(renderer, current, &status, &publication, &client);
                client.unpark();
            })
            .map_err(|_| AndroidPlaybackError)?;
        if ready.recv().is_err() {
            let _ = join.join();
            return Err(AndroidPlaybackError);
        }
        Ok(Self {
            picture,
            progress,
            join: Some(join),
        })
    }

    pub(crate) fn set_viewport(&self, viewport: Option<AndroidPlaybackViewport>) {
        self.picture.set_viewport(viewport);
        if let Some(join) = &self.join {
            join.thread().unpark();
        }
    }

    pub(crate) fn presented(&self) -> u64 {
        self.progress.presented.load(Ordering::Acquire)
    }

    pub(crate) fn failed(&self) -> bool {
        self.progress.failed.load(Ordering::Acquire)
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            self.progress.stop.store(true, Ordering::Release);
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

fn render(
    mut renderer: Renderer,
    picture: Arc<Picture>,
    progress: &Progress,
    publication: &Publication,
    client: &thread::Thread,
) {
    let mut size = picture.size();
    let mut spare = Vec::new();
    let mut colors = [0; 3];
    let mut sampled_at: Option<Instant> = None;
    while !progress.stop.load(Ordering::Acquire) {
        let update = renderer.updated();
        let next_size = picture.size();
        let first = progress.presented.load(Ordering::Relaxed) == 0;
        if update == Update::None && (first || next_size == size) {
            thread::park();
            continue;
        }
        let mut bgra = std::mem::take(&mut spare);
        let started = Instant::now();
        let target = match update {
            Update::Frame(delay) => started + delay,
            _ => started,
        };
        let Ok(native_render_time) = renderer.draw(next_size, &mut bgra) else {
            progress.failed.store(true, Ordering::Release);
            break;
        };
        let now = Instant::now();
        if first && !matches!(update, Update::Frame(_)) {
            spare = bgra;
            continue;
        }
        size = next_size;
        if sampled_at.is_none_or(|last| now.duration_since(last) >= Duration::from_millis(125)) {
            colors = ambient(&bgra, size.width, size.height);
            sampled_at = Some(now);
        }
        // Draw ahead of mpv's presentation deadline, then publish on its
        // audio clock. Stop can interrupt the bounded wait.
        while let Some(remaining) = target.checked_duration_since(Instant::now()) {
            if progress.stop.load(Ordering::Acquire) {
                return;
            }
            thread::park_timeout(remaining);
        }
        let frame = Arc::new(VideoFrame {
            key: picture.key.clone(),
            width: size.width,
            height: size.height,
            bgra,
            serial: publication.serial.fetch_add(1, Ordering::Relaxed) + 1,
            ambient: colors,
            native_render_time,
            render_time: now.duration_since(started),
        });
        if let Some(previous) = picture.frame.swap(Some(frame))
            && let Ok(previous) = Arc::try_unwrap(previous)
        {
            spare = previous.bgra;
        }
        if first {
            publication.current.store(Some(picture.clone()));
        }
        if matches!(update, Update::Frame(_)) {
            progress.presented.fetch_add(1, Ordering::Release);
            if first {
                client.unpark();
            }
        }
    }
}
