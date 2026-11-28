use std::{
    collections::HashMap,
    future::pending,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use gpui::{App, BackgroundExecutor, RenderImage, Task, Window};
use image::{Frame, RgbaImage};
use sparrow_channel_icons::{
    ChannelIconClient, DecodedChannelIcon,
    cache::{CacheLimits, IconCache, REQUEST_LIMIT, RequestId},
    disk::DiskCache,
};
use sparrow_core::{CatalogGeneration, ChannelIcon};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{Semaphore, mpsc, watch},
    task::{AbortHandle, spawn_blocking},
};

use crate::guide_view::FormFactor;

const NOTIFICATION_BATCH: Duration = Duration::from_millis(24);

/// Owns decoded images and outstanding requests for one Guide window.
pub(super) struct ChannelIcons {
    disk: Option<Arc<DiskCache>>,
    disk_budget: (Duration, Duration),
    revision: u64,
    generation: Option<CatalogGeneration>,
    cache: IconCache<IconImage>,
    requests: HashMap<RequestId, AbortOnDrop>,
    completions: mpsc::Sender<Completion>,
    disk_completions: mpsc::Sender<Completion>,
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

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) enum Completion {
    Network {
        request: RequestId,
        image: Option<IconImage>,
    },
    Disk {
        request: RequestId,
        hit: Option<(IconImage, bool)>,
    },
}

pub(super) struct IconImage {
    render_image: Arc<RenderImage>,
    tint: Option<f32>,
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

    pub(super) fn tint(&self) -> Option<f32> {
        self.tint
    }

    fn new(decoded: DecodedChannelIcon) -> Option<Self> {
        let source_dimensions = decoded.source_dimensions();
        let (width, height, bgra) = decoded.into_bgra();
        // Charge the retained pixel storage plus the sprite's texel allocation.
        let charged_bytes = bgra.capacity() + bgra.len();
        let tint = dominant_hue(&bgra);
        let pixels = RgbaImage::from_raw(width, height, bgra)?;
        Some(Self {
            render_image: Arc::new(RenderImage::new([Frame::new(pixels)])),
            source_dimensions,
            charged_bytes,
            tint,
        })
    }

    fn from_disk(disk: &DiskCache, location: &str) -> Option<(Self, bool)> {
        let hit = disk.load(location)?;
        Some((Self::new(hit.icon)?, hit.stale))
    }

    fn charged(self) -> (Self, usize) {
        let bytes = self.charged_bytes;
        (self, bytes)
    }
}

/// Alpha and chroma weight the hue histogram so transparent padding and white
/// lettering cannot outvote the coloured part of a Channel Icon.
fn dominant_hue(bgra: &[u8]) -> Option<f32> {
    let mut bins = [(0_f32, 0_f32); 24];
    let mut coverage = 0.;
    let mut chroma_sum = 0.;
    for &[b, g, r, a] in bgra.as_chunks::<4>().0 {
        let alpha = f32::from(a) / 255.;
        coverage += alpha;
        let chroma = f32::from(r.max(g).max(b) - r.min(g).min(b)) / 255.;
        if alpha == 0. || chroma < 0.12 {
            continue;
        }
        let color = gpui::Hsla::from(gpui::rgb(u32::from_be_bytes([0, r, g, b])));
        if !(0.08..=0.92).contains(&color.l) {
            continue;
        }
        let weight = alpha * chroma;
        chroma_sum += weight;
        let index = (color.h * bins.len() as f32) as usize % bins.len();
        bins[index].0 += weight;
        bins[index].1 += weight * color.h;
    }
    if coverage == 0. || chroma_sum < 0.12 * coverage {
        return None;
    }
    let (weight, sum) = bins.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    Some(sum / weight)
}

impl ChannelIcons {
    pub(super) fn new(
        disk: Option<Arc<DiskCache>>,
        form_factor: FormFactor,
    ) -> (Self, IconUpdates, mpsc::Receiver<Completion>) {
        let (limits, warm_ms, cold_ms) = match form_factor {
            FormFactor::Desktop => (
                CacheLimits {
                    max_bytes: 128 << 20,
                    max_entries: 512,
                    max_desired: 128,
                },
                14,
                30,
            ),
            FormFactor::Handheld => (
                CacheLimits {
                    max_bytes: 64 << 20,
                    max_entries: 256,
                    max_desired: 96,
                },
                6,
                16,
            ),
            FormFactor::Television => (
                CacheLimits {
                    max_bytes: 32 << 20,
                    max_entries: 160,
                    max_desired: 64,
                },
                4,
                12,
            ),
        };
        let (completions, receiver) = mpsc::channel(REQUEST_LIMIT);
        let (disk_completions, disk_receiver) = mpsc::channel(limits.max_desired);
        let (wakeup, wakeups) = watch::channel(Wakeup::default());
        (
            Self {
                disk,
                disk_budget: (
                    Duration::from_millis(warm_ms),
                    Duration::from_millis(cold_ms),
                ),
                revision: 0,
                generation: None,
                cache: IconCache::with_limits(limits),
                requests: HashMap::new(),
                completions,
                disk_completions,
                wakeup,
            },
            IconUpdates {
                completions: receiver,
                wakeup: wakeups,
                admission_timer: None,
            },
            disk_receiver,
        )
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
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
            self.revision = self.revision.wrapping_add(1);
            self.publish_wakeup(Instant::now());
        }
    }

    pub(super) fn reconcile<'a>(
        &mut self,
        locations: impl IntoIterator<Item = &'a str>,
        current_window: Option<&mut Window>,
        cx: &mut App,
    ) {
        let now = Instant::now();
        let retired = self.cache.reconcile(locations, now);
        self.cancel(retired);
        if let Some(disk) = self.disk.clone() {
            self.hydrate_disk(&disk, now, current_window, cx);
        }
        self.fill(Instant::now());
    }

    fn hydrate_disk(
        &mut self,
        disk: &Arc<DiskCache>,
        now: Instant,
        mut current_window: Option<&mut Window>,
        cx: &mut App,
    ) {
        let (warm, cold) = self.disk_budget;
        let deadline = now + if self.cache.is_empty() { cold } else { warm };
        let candidates: Vec<_> = self.cache.disk_candidates().collect();
        let mut candidates = candidates.into_iter().peekable();
        let mut changed = false;
        while Instant::now() < deadline
            && let Some(location) = candidates.next()
        {
            if let Some((image, stale)) = IconImage::from_disk(disk, &location) {
                let (published, evicted) =
                    self.cache
                        .insert_cached(&location, image.charged(), stale, now);
                changed |= published;
                for image in evicted {
                    cx.drop_image(image.render_image, current_window.as_deref_mut());
                }
            } else {
                self.cache.note_disk_miss(location);
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        if candidates.peek().is_none() {
            return;
        }
        if let Some(worker) = IconWorker::get() {
            for location in candidates {
                let request = self.cache.begin_disk_read(Arc::clone(&location));
                let disk = Arc::clone(disk);
                let sender = self.disk_completions.clone();
                let task = worker.runtime.spawn(async move {
                    let load = spawn_blocking(move || IconImage::from_disk(&disk, &location));
                    let _cancel = AbortOnDrop(load.abort_handle());
                    let hit = load.await.ok().flatten();
                    let _ = sender.send(Completion::Disk { request, hit }).await;
                });
                self.requests
                    .insert(request, AbortOnDrop(task.abort_handle()));
            }
        }
    }

    pub(super) fn get(&self, icon: &ChannelIcon) -> Option<&IconImage> {
        self.cache.get(icon.as_str())
    }

    /// Apply completions, or fill a slot when the HTTP navigation delay expires.
    pub(super) fn apply(
        &mut self,
        completions: impl IntoIterator<Item = Completion>,
        cx: &mut App,
    ) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for completion in completions {
            let (published, evicted) = match completion {
                Completion::Network { request, image } => {
                    self.requests.remove(&request);
                    self.cache
                        .complete(request, image.map(IconImage::charged), now)
                }
                Completion::Disk { request, hit } => {
                    self.requests.remove(&request);
                    self.cache.complete_disk_read(
                        request,
                        hit.map(|(image, stale)| (image.charged(), stale)),
                        now,
                    )
                }
            };
            changed |= published;
            for image in evicted {
                cx.drop_image(image.render_image, None);
            }
        }
        self.fill(now);
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
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
            let disk = self.disk.clone();
            let task = worker.runtime.spawn(async move {
                let image = worker.load(disk, request.location()).await;
                let _ = sender
                    .send(Completion::Network { request: id, image })
                    .await;
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
    ) -> Option<Vec<Completion>> {
        let first = loop {
            let admission = self.wakeup.borrow_and_update().admission_at;
            if let Some(admission) = admission {
                let remaining = admission.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Some(Vec::new());
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
        Some(completions)
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

    async fn load(
        &'static self,
        disk: Option<Arc<DiskCache>>,
        location: &str,
    ) -> Option<IconImage> {
        let request = self.requests.acquire().await.ok()?;
        let client = self
            .client
            .get_or_init(|| ChannelIconClient::new().ok())
            .as_ref()?;
        let encoded = client.fetch(location).await.ok()?;
        let decoder = self.decoder.acquire().await.ok()?;
        let location = location.to_owned();
        let decode = spawn_blocking(move || {
            // Cancellation stops the async task immediately. A started decode
            // keeps both permits until it finishes and releases its buffers.
            let permits = (request, decoder);
            let decoded = encoded.decode().ok()?;
            let sweep = disk.filter(|disk| disk.store(&location, &decoded).unwrap_or(false));
            let image = IconImage::new(decoded);
            drop(permits);
            if let Some(disk) = sweep {
                spawn_blocking(move || disk.trim());
            }
            image
        });
        let _cancel = AbortOnDrop(decode.abort_handle());
        decode.await.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    async fn seed_icons(root: &std::path::Path, locations: &[&str]) -> Arc<DiskCache> {
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&[255; 8], 2, 1, ExtendedColorType::Rgba8)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!("http://{}/icon", listener.local_addr().unwrap());
        let client = ChannelIconClient::new().unwrap();
        let serve = async {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert_ne!(socket.read(&mut [0; 1024]).await.unwrap(), 0);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", png.len());
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&png).await.unwrap();
        };
        let (_, encoded) = tokio::join!(serve, client.fetch(&location));
        let image = encoded.unwrap().decode().unwrap();
        let disk = Arc::new(DiskCache::new(root));
        for location in locations {
            disk.store(location, &image).unwrap();
        }
        disk
    }

    #[gpui::test]
    fn reconcile_hydrates_disk_hits_before_paint_during_navigation(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let locations = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let disk = runtime.block_on(seed_icons(root.path(), &locations));

        for form in [
            FormFactor::Desktop,
            FormFactor::Handheld,
            FormFactor::Television,
        ] {
            let (mut icons, _, _) = ChannelIcons::new(Some(Arc::clone(&disk)), form);
            icons.note_navigation();
            cx.update(|cx| icons.reconcile(locations, None, cx));
            for location in locations {
                assert_eq!(
                    icons.cache.get(location).map(IconImage::source_dimensions),
                    Some((2, 1)),
                );
            }
        }
    }

    #[gpui::test]
    fn disk_overflow_completes_while_http_is_quiet(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let locations = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let disk = runtime.block_on(seed_icons(root.path(), &locations));
        let (mut icons, network, mut disk_updates) =
            ChannelIcons::new(Some(disk), FormFactor::Desktop);
        icons.disk_budget = (Duration::ZERO, Duration::ZERO);
        icons.note_navigation();
        cx.update(|cx| icons.reconcile(locations, None, cx));

        let updates = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut updates = Vec::new();
                for _ in locations {
                    updates.push(disk_updates.recv().await.unwrap());
                }
                updates
            })
            .await
            .unwrap()
        });
        assert!(cx.update(|cx| icons.apply(updates, cx)));
        assert!(
            locations
                .iter()
                .all(|location| icons.cache.get(location).is_some())
        );
        assert!(network.completions.is_empty());
    }
}
