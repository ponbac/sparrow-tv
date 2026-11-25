use std::{
    collections::HashMap,
    future::pending,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use gpui::{App, BackgroundExecutor, RenderImage, Task};
use image::{Frame, RgbaImage};
use sparrow_channel_icons::{
    ChannelIconClient, DecodedChannelIcon,
    cache::{IconCache, REQUEST_LIMIT, RequestId},
};
use sparrow_core::{CatalogGeneration, ChannelIcon};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{Semaphore, mpsc, watch},
    task::{AbortHandle, spawn_blocking},
};

const NOTIFICATION_BATCH: Duration = Duration::from_millis(24);

/// Owns decoded images and outstanding requests for one Guide window.
pub(super) struct ChannelIcons {
    generation: Option<CatalogGeneration>,
    cache: IconCache<IconImage>,
    requests: HashMap<RequestId, AbortOnDrop>,
    completions: mpsc::Sender<Completion>,
    wakeup: watch::Sender<Wakeup>,
}

pub(super) struct IconUpdates {
    completions: mpsc::Receiver<Completion>,
    wakeup: watch::Receiver<Wakeup>,
    admission_timer: Option<Task<()>>,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Wakeup {
    admission_at: Option<Instant>,
    notification_quiet_until: Option<Instant>,
}

pub(super) struct IconBatch(Vec<Completion>);

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Completion {
    request: RequestId,
    image: Option<IconImage>,
}

pub(super) struct IconImage {
    render_image: Arc<RenderImage>,
    pub(super) tint: Option<u32>,
    source_dimensions: (u32, u32),
    charged_bytes: usize,
}

impl IconImage {
    pub(super) fn render_image(&self) -> Arc<RenderImage> {
        Arc::clone(&self.render_image)
    }

    pub(super) fn source_dimensions(&self) -> (u32, u32) {
        self.source_dimensions
    }

    fn new(decoded: DecodedChannelIcon) -> Option<Self> {
        let source_dimensions = decoded.source_dimensions();
        let (width, height, bgra) = decoded.into_bgra();
        // Charge the retained pixel storage plus the sprite's texel allocation.
        let charged_bytes = bgra.capacity() + bgra.len();
        let mut sums = [0_u64; 3];
        let mut weight = 0;
        for pixel in bgra.as_chunks::<4>().0 {
            let alpha = u64::from(pixel[3]);
            weight += alpha;
            for (sum, component) in sums.iter_mut().zip(pixel) {
                *sum += alpha * u64::from(*component);
            }
        }
        let tint = (weight > 0).then(|| {
            let [b, g, r] = sums.map(|sum| (sum / weight) as u32);
            r << 16 | g << 8 | b
        });
        let pixels = RgbaImage::from_raw(width, height, bgra)?;
        Some(Self {
            render_image: Arc::new(RenderImage::new([Frame::new(pixels)])),
            source_dimensions,
            charged_bytes,
            tint,
        })
    }
}

impl ChannelIcons {
    pub(super) fn new() -> (Self, IconUpdates) {
        let (completions, receiver) = mpsc::channel(REQUEST_LIMIT * 2);
        let (wakeup, wakeups) = watch::channel(Wakeup::default());
        (
            Self {
                generation: None,
                cache: IconCache::default(),
                requests: HashMap::new(),
                completions,
                wakeup,
            },
            IconUpdates {
                completions: receiver,
                wakeup: wakeups,
                admission_timer: None,
            },
        )
    }

    pub(super) fn note_navigation(&mut self) {
        let now = Instant::now();
        self.cache.note_navigation(now);
        self.publish_wakeup(now);
    }

    pub(super) fn hide(&mut self) {
        let retired = self.cache.hide();
        self.cancel(retired);
        self.publish_wakeup(Instant::now());
    }

    /// Retire outstanding work as soon as the view adopts a new catalog.
    pub(super) fn set_generation(&mut self, generation: Option<CatalogGeneration>) {
        if self.generation != generation {
            let retired = self.cache.reset_generation();
            self.cancel(retired);
            self.generation = generation;
            self.publish_wakeup(Instant::now());
        }
    }

    pub(super) fn reconcile<'a>(
        &mut self,
        generation: Option<CatalogGeneration>,
        icons: impl IntoIterator<Item = &'a ChannelIcon>,
    ) {
        self.set_generation(generation);
        if generation.is_some() {
            let now = Instant::now();
            let retired = self
                .cache
                .reconcile(icons.into_iter().map(ChannelIcon::as_str), now);
            self.cancel(retired);
            self.fill(now);
        }
    }

    pub(super) fn get(&self, icon: &ChannelIcon) -> Option<&IconImage> {
        self.cache.get(icon.as_str())
    }

    /// Apply a completion batch, or fill a slot when the navigation delay expires.
    pub(super) fn apply(&mut self, batch: IconBatch, cx: &mut App) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for Completion { request, image } in batch.0 {
            self.requests.remove(&request);
            let value = image.map(|image| {
                let bytes = image.charged_bytes;
                (image, bytes)
            });
            let (published, evicted) = self.cache.complete(request, value, now);
            changed |= published;
            for image in evicted {
                cx.drop_image(image.render_image, None);
            }
        }
        self.fill(now);
        changed
    }

    fn cancel(&mut self, requests: Vec<RequestId>) {
        for request in requests {
            self.requests.remove(&request);
        }
    }

    fn fill(&mut self, now: Instant) {
        for request in self.cache.admit(now) {
            let id = request.id();
            let Some(worker) = IconWorker::get() else {
                self.cache.complete(id, None, now);
                continue;
            };
            let sender = self.completions.clone();
            let task = worker.runtime.spawn(async move {
                let image = worker.load(request.location()).await;
                let _ = sender.send(Completion { request: id, image }).await;
            });
            self.requests.insert(id, AbortOnDrop(task.abort_handle()));
        }
        self.publish_wakeup(now);
    }

    fn publish_wakeup(&self, now: Instant) {
        let wakeup = Wakeup {
            admission_at: self.cache.deferred_admission(now),
            notification_quiet_until: self.cache.notification_quiet_until(),
        };
        self.wakeup.send_if_modified(|current| {
            if *current == wakeup {
                return false;
            }
            *current = wakeup;
            true
        });
    }
}

impl IconUpdates {
    pub(super) async fn next_batch(
        &mut self,
        background: &BackgroundExecutor,
    ) -> Option<IconBatch> {
        let first = loop {
            let admission = self.wakeup.borrow_and_update().admission_at;
            if let Some(admission) = admission {
                let remaining = admission.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Some(IconBatch(Vec::new()));
                }
                // GPUI's dispatcher still runs a dropped timer. Keep one armed
                // through deadline updates and re-check eligibility when it fires.
                self.admission_timer
                    .get_or_insert_with(|| background.timer(remaining));
            }
            let wait_for_admission = async {
                if let Some(timer) = &mut self.admission_timer {
                    timer.await;
                } else {
                    pending::<()>().await;
                }
            };
            tokio::select! {
                changed = self.wakeup.changed() => { changed.ok()?; }
                result = self.completions.recv() => break result?,
                () = wait_for_admission => self.admission_timer = None,
            }
        };

        background.timer(NOTIFICATION_BATCH).await;
        loop {
            let remaining = self
                .wakeup
                .borrow()
                .notification_quiet_until
                .map_or(Duration::ZERO, |until| {
                    until.saturating_duration_since(Instant::now())
                });
            if remaining.is_zero() {
                break;
            }
            background.timer(remaining).await;
        }
        let mut completions = vec![first];
        while let Ok(result) = self.completions.try_recv() {
            completions.push(result);
        }
        Some(IconBatch(completions))
    }
}

struct IconWorker {
    runtime: Runtime,
    client: OnceLock<Option<ChannelIconClient>>,
    requests: Semaphore,
    decoder: Semaphore,
}

impl IconWorker {
    fn get() -> Option<&'static Self> {
        static WORKER: OnceLock<Option<IconWorker>> = OnceLock::new();
        // No networking runtime or decoder thread is needed when the guide has no icons.
        WORKER
            .get_or_init(|| {
                let runtime = Builder::new_multi_thread()
                    .worker_threads(1)
                    .max_blocking_threads(4)
                    .thread_name("channel-icons")
                    .enable_all()
                    .build()
                    .ok()?;
                Some(Self {
                    runtime,
                    client: OnceLock::new(),
                    requests: Semaphore::new(REQUEST_LIMIT),
                    decoder: Semaphore::new(1),
                })
            })
            .as_ref()
    }

    async fn load(&'static self, location: &str) -> Option<IconImage> {
        let request = self.requests.acquire().await.ok()?;
        let client = self
            .client
            .get_or_init(|| ChannelIconClient::new().ok())
            .as_ref()?;
        let encoded = client.fetch(location).await.ok()?;
        let decoder = self.decoder.acquire().await.ok()?;
        let decode = spawn_blocking(move || {
            // Cancellation stops the async task immediately. A started decode
            // keeps both permits until it finishes and releases its buffers.
            let (_request, _decoder) = (request, decoder);
            encoded.decode().ok().and_then(IconImage::new)
        });
        let _cancel = AbortOnDrop(decode.abort_handle());
        decode.await.ok().flatten()
    }
}
