//! Installed catalog, bounded reads, and window lifecycle delivery.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use sparrow_bounded_blocking::BoundedBlocking;
use sparrow_channel_icons::disk::DiskCache;
use sparrow_core::{
    CatalogAccess, CatalogStatus, CoreError, CoreEvent, InputField, LifecycleSignal,
    RefreshOutcome, RefreshReport, RefreshSkipReason, RefreshTrigger, SourceState,
};
use sparrow_installed_catalog::{
    GuidePreferenceChange, InstalledCatalog, SavedSourceLocations, SourceConfigurationUpdate,
};
use sparrow_installed_playback::tv::{TvPlaybackCommand, TvPlaybackRunner};

use crate::{GuideDiagnostics, PlaybackHost, WorkflowDiagnostics};
use sparrow_tv_guide::VariantPreferences;
use sparrow_tv_guide::search::{SearchCompletion, SearchRead};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{mpsc, oneshot, watch},
};

#[cfg(feature = "fixture")]
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Starting,
    Unconfigured,
    Unavailable,
    Ready,
    StartupFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpsProbe {
    Idle,
    #[cfg(feature = "fixture")]
    Running,
    #[cfg(feature = "fixture")]
    Verified,
    #[cfg(feature = "fixture")]
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureSeed {
    Idle,
    #[cfg(feature = "fixture")]
    Running,
    #[cfg(feature = "fixture")]
    Complete,
    #[cfg(feature = "fixture")]
    Failed,
}

/// Allowlisted catalog diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    pub generation_changes: u64,
    pub active: bool,
    pub refreshing: bool,
    pub https: HttpsProbe,
    pub seed: FixtureSeed,
}

pub struct Catalog {
    state: Arc<State>,
    worker: OnceLock<Option<Worker>>,
    pub(crate) guide: Mutex<GuideDiagnostics>,
    pub(crate) workflows: Mutex<WorkflowDiagnostics>,
    pub(crate) icon_cache: OnceLock<Arc<DiskCache>>,
}

struct State {
    catalog: OnceLock<Arc<InstalledCatalog>>,
    playback: Option<Arc<dyn PlaybackHost>>,
    runner: OnceLock<TvPlaybackRunner>,
    publication: watch::Sender<Publication>,
    group_revision: watch::Sender<u64>,
    next_group_write: AtomicU64,
    searches: BoundedBlocking,
    variant_preferences: watch::Sender<Arc<VariantPreferences>>,
    playback_queries: BoundedBlocking,
    playback_subject_queries: BoundedBlocking,
}

enum PreferenceWrite {
    Backdrop {
        choice: sparrow_installed_catalog::GuideBackdrop,
        completed: oneshot::Sender<bool>,
    },
    SetAmbientLight {
        enabled: bool,
        completed: oneshot::Sender<bool>,
    },
    Group {
        sequence: u64,
        change: GuidePreferenceChange,
        completed: oneshot::Sender<bool>,
    },
    Variant {
        change: GuidePreferenceChange,
        completed: oneshot::Sender<bool>,
    },
}

/// The Guide's typed catalog state and the host that owns each active window.
pub(super) struct Publication {
    snapshot: Snapshot,
    active_hosts: HashMap<u64, u64>,
    next_activation: u64,
    pub(super) status: Option<Arc<CatalogStatus>>,
    pub(super) status_revision: u64,
}

impl Publication {
    pub(super) fn activation(&self, host: u64) -> Option<u64> {
        self.active_hosts.get(&host).copied()
    }

    pub(super) fn phase(&self) -> Phase {
        self.snapshot.phase
    }

    fn set_status(&mut self, status: CatalogStatus, reconcile: bool) -> bool {
        if self.status.as_deref() == Some(&status) && !reconcile {
            return false;
        }
        let generation = status.generation();
        if generation
            != self
                .status
                .as_ref()
                .and_then(|previous| previous.generation())
        {
            self.snapshot.generation_changes = self.snapshot.generation_changes.wrapping_add(1);
        }
        self.snapshot.refreshing = matches!(status.m3u(), SourceState::Refreshing { .. })
            || matches!(status.epg(), Some(SourceState::Refreshing { .. }));
        self.snapshot.phase = if generation.is_some() {
            Phase::Ready
        } else if !status.configuration().is_configured() {
            Phase::Unconfigured
        } else if self.snapshot.refreshing {
            Phase::Starting
        } else {
            Phase::Unavailable
        };
        self.status = Some(Arc::new(status));
        self.status_revision = self.status_revision.wrapping_add(1);
        true
    }
}

struct Worker {
    commands: mpsc::UnboundedSender<Command>,
    preference_changes: mpsc::Sender<PreferenceWrite>,
    runtime: Runtime,
}

enum Command {
    HostActive {
        id: u64,
        active: bool,
    },
    HostDestroyed {
        id: u64,
        finishing: bool,
    },
    ReconcileStatus,
    #[cfg(feature = "fixture")]
    Seed {
        m3u: String,
        epg: String,
        completed: oneshot::Sender<bool>,
    },
    #[cfg(feature = "fixture")]
    CheckHttps(String),
}

impl Catalog {
    pub fn new(playback: Option<Arc<dyn PlaybackHost>>) -> Self {
        Self {
            state: Arc::new(State {
                catalog: OnceLock::new(),
                playback,
                runner: OnceLock::new(),
                group_revision: watch::channel(0).0,
                next_group_write: AtomicU64::new(0),
                searches: BoundedBlocking::serial(),
                variant_preferences: watch::channel(Arc::new(VariantPreferences::default())).0,
                playback_queries: BoundedBlocking::serial(),
                playback_subject_queries: BoundedBlocking::serial(),
                publication: watch::channel(Publication {
                    snapshot: Snapshot {
                        phase: Phase::Starting,
                        generation_changes: 0,
                        active: false,
                        refreshing: false,
                        https: HttpsProbe::Idle,
                        seed: FixtureSeed::Idle,
                    },
                    active_hosts: HashMap::new(),
                    next_activation: 0,
                    status: None,
                    status_revision: 0,
                })
                .0,
            }),
            worker: OnceLock::new(),
            guide: Mutex::new(GuideDiagnostics::default()),
            workflows: Mutex::new(WorkflowDiagnostics::default()),
            icon_cache: OnceLock::new(),
        }
    }

    pub(crate) fn playback(&self) -> Option<&dyn PlaybackHost> {
        self.state.playback.as_deref()
    }

    pub(crate) fn runner(&self) -> Option<&TvPlaybackRunner> {
        self.state.runner.get()
    }

    pub fn guide_diagnostics(&self) -> (GuideDiagnostics, WorkflowDiagnostics) {
        (*self.guide.lock().unwrap(), *self.workflows.lock().unwrap())
    }

    pub fn start(&self, data_dir: PathBuf) {
        self.worker.get_or_init(|| {
            let runtime = match Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .max_blocking_threads(4)
                .thread_name("sparrow-catalog")
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    self.state.startup_failed();
                    return None;
                }
            };
            self.state
                .modify_snapshot(|state| state.phase = Phase::Starting);
            self.icon_cache
                .get_or_init(|| Arc::new(DiskCache::new(&data_dir.join("cache"))));
            let (commands, pending) = mpsc::unbounded_channel();
            let (preference_changes, preference_updates) = mpsc::channel(64);
            runtime.spawn(Arc::clone(&self.state).run(data_dir, pending, preference_updates));
            Some(Worker {
                commands,
                preference_changes,
                runtime,
            })
        });
    }

    #[cfg(feature = "fixture")]
    pub fn seed_fixture(&self, m3u: String, epg: String) -> Option<oneshot::Receiver<bool>> {
        let (completed, result) = oneshot::channel();
        self.send(Command::Seed {
            m3u,
            epg,
            completed,
        })
        .then_some(result)
    }

    #[cfg(feature = "fixture")]
    pub fn probe_https(&self, url: String) -> bool {
        self.send(Command::CheckHttps(url))
    }

    fn send(&self, command: Command) -> bool {
        self.worker
            .get()
            .and_then(Option::as_ref)
            .is_some_and(|worker| worker.commands.send(command).is_ok())
    }

    pub fn set_active(&self, id: u64, active: bool) {
        self.send(Command::HostActive { id, active });
    }

    pub fn host_destroyed(&self, id: u64, finishing: bool) {
        if let Some(playback) = self.playback() {
            playback.host_destroyed(id);
        }
        self.send(Command::HostDestroyed { id, finishing });
    }

    pub fn set_start_failed(&self) {
        // A later host cannot replace the running catalog.
        if self.worker.get().is_none() {
            self.state.startup_failed();
        }
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<Publication> {
        self.state.publication.subscribe()
    }

    pub(super) fn status(&self) -> Option<Arc<CatalogStatus>> {
        self.state.publication.borrow().status.clone()
    }

    pub(super) fn search(
        &self,
        read: SearchRead,
    ) -> Option<tokio::task::JoinHandle<SearchCompletion>> {
        let worker = self.worker.get()?.as_ref()?;
        let catalog = self.state.catalog.get()?.core().clone();
        let searches = self.state.searches.clone();
        Some(worker.runtime.spawn(async move {
            let unavailable = read.unavailable();
            searches
                .run_with_cancellation(read.cancellation(), move |_| read.run(&catalog))
                .await
                .unwrap_or(unavailable)
        }))
    }

    /// Saves the draft and returns the catalog status once its first refresh ends.
    pub(super) fn save_sources(
        &self,
        m3u: String,
        epg: Option<String>,
    ) -> Option<tokio::task::JoinHandle<Result<CatalogStatus, SourceSaveFailure>>> {
        let worker = self.worker.get()?.as_ref()?;
        let catalog = Arc::clone(self.state.catalog.get()?);
        let state = Arc::clone(&self.state);
        let commands = worker.commands.clone();
        Some(worker.runtime.spawn(async move {
            let update =
                SourceConfigurationUpdate::parse(m3u, epg).map_err(|error| match error {
                    CoreError::InvalidInput {
                        field: InputField::M3u,
                        ..
                    } => SourceSaveFailure::M3u,
                    CoreError::InvalidInput {
                        field: InputField::Epg,
                        ..
                    } => SourceSaveFailure::Epg,
                    _ => SourceSaveFailure::Unavailable,
                })?;
            let _playback_fence = if let Some(runner) = state.runner.get() {
                Some(
                    runner
                        .stop_for_source_change()
                        .await
                        .map_err(|_| SourceSaveFailure::Unavailable)?,
                )
            } else {
                None
            };
            let status = catalog
                .replace_configuration(update)
                .await
                .map_err(|_| SourceSaveFailure::Unavailable)?;
            let _ = commands.send(Command::ReconcileStatus);
            Ok(status)
        }))
    }

    /// The saved Source Configuration, for prefilling a draft. `Err` means the
    /// private record could not be read, which differs from nothing saved.
    pub(super) fn saved_source_locations(
        &self,
    ) -> Option<tokio::task::JoinHandle<Result<Option<SavedSourceLocations>, ()>>> {
        let worker = self.worker.get()?.as_ref()?;
        let catalog = Arc::clone(self.state.catalog.get()?);
        Some(
            worker
                .runtime
                .spawn(async move { catalog.saved_source_locations().await.map_err(|_| ()) }),
        )
    }

    /// Refreshes the sources now and reports how it went.
    pub(super) fn refresh_now(&self) -> Option<tokio::task::JoinHandle<RefreshReport>> {
        let worker = self.worker.get()?.as_ref()?;
        let core = self.state.catalog.get()?.core().clone();
        let commands = worker.commands.clone();
        Some(worker.runtime.spawn(async move {
            let report = core.refresh(RefreshTrigger::Manual).await;
            let _ = commands.send(Command::ReconcileStatus);
            report
        }))
    }

    pub(super) fn group_exclusions(&self) -> Option<HashSet<String>> {
        let preferences = self.state.catalog.get()?.guide_preferences();
        Some(preferences.excluded_groups().clone())
    }

    pub(super) fn variant_preferences(&self) -> watch::Receiver<Arc<VariantPreferences>> {
        self.state.variant_preferences.subscribe()
    }

    pub(super) fn ambient_light(&self) -> bool {
        self.state
            .catalog
            .get()
            .is_none_or(|catalog| catalog.guide_preferences().ambient_light())
    }

    pub(super) fn set_ambient_light(&self, enabled: bool) -> Option<oneshot::Receiver<bool>> {
        let (completed, result) = oneshot::channel();
        self.worker
            .get()?
            .as_ref()?
            .preference_changes
            .try_send(PreferenceWrite::SetAmbientLight { enabled, completed })
            .ok()?;
        Some(result)
    }

    pub(super) fn prefer_variant(
        &self,
        group: String,
        normalized_title: String,
        quality: sparrow_core::ChannelQuality,
    ) -> Option<oneshot::Receiver<bool>> {
        let worker = self.worker.get()?.as_ref()?;
        let change = GuidePreferenceChange::PreferVariant {
            group,
            normalized_title,
            quality,
        };
        let (completed, result) = oneshot::channel();
        worker
            .preference_changes
            .try_send(PreferenceWrite::Variant { change, completed })
            .ok()?;
        Some(result)
    }

    pub(super) fn playback_query<T: Send + 'static>(
        &self,
        query: impl FnOnce(&CatalogAccess) -> T + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<Option<T>>> {
        self.bounded_read(&self.state.playback_queries, query)
    }

    pub(super) fn playback_subject_query<T: Send + 'static>(
        &self,
        query: impl FnOnce(&CatalogAccess) -> T + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<Option<T>>> {
        self.bounded_read(&self.state.playback_subject_queries, query)
    }

    fn bounded_read<T: Send + 'static>(
        &self,
        requests: &BoundedBlocking,
        query: impl FnOnce(&CatalogAccess) -> T + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<Option<T>>> {
        let worker = self.worker.get()?.as_ref()?;
        let catalog = self.state.catalog.get()?.core().clone();
        let requests = requests.clone();
        Some(
            worker
                .runtime
                .spawn(async move { requests.run(move |_| query(&catalog)).await.ok() }),
        )
    }

    pub(super) fn subscribe_group_writes(&self) -> watch::Receiver<u64> {
        self.state.group_revision.subscribe()
    }

    pub(super) fn backdrop(&self) -> sparrow_installed_catalog::GuideBackdrop {
        self.state
            .catalog
            .get()
            .map(|catalog| catalog.guide_preferences().backdrop())
            .unwrap_or_default()
    }

    pub(super) fn save_backdrop(
        &self,
        choice: sparrow_installed_catalog::GuideBackdrop,
    ) -> Option<oneshot::Receiver<bool>> {
        let worker = self.worker.get()?.as_ref()?;
        let (completed, result) = oneshot::channel();
        worker
            .preference_changes
            .try_send(PreferenceWrite::Backdrop { choice, completed })
            .ok()?;
        Some(result)
    }

    pub(super) fn save_group_visibility(
        &self,
        change: GuidePreferenceChange,
    ) -> Option<(u64, oneshot::Receiver<bool>)> {
        let worker = self.worker.get()?.as_ref()?;
        let sequence = self.state.next_group_write.fetch_add(1, Ordering::Relaxed) + 1;
        let (completed, result) = oneshot::channel();
        worker
            .preference_changes
            .try_send(PreferenceWrite::Group {
                sequence,
                change,
                completed,
            })
            .ok()?;
        Some((sequence, result))
    }

    pub(super) fn reconcile_status(&self) {
        if let Some(Some(worker)) = self.worker.get() {
            let _ = worker.commands.send(Command::ReconcileStatus);
        }
    }

    pub fn diagnostics(&self) -> (Snapshot, usize) {
        (
            self.state.publication.borrow().snapshot,
            self.state.publication.receiver_count(),
        )
    }

    pub(super) fn read<R: Send + 'static>(
        &self,
        read: impl FnOnce(&CatalogAccess) -> R + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<R>> {
        let worker = self.worker.get()?.as_ref()?;
        let core = self.state.catalog.get()?.core().clone();
        Some(worker.runtime.spawn_blocking(move || read(&core)))
    }
}

#[derive(Clone, Copy)]
pub(super) enum SourceSaveFailure {
    M3u,
    Epg,
    Unavailable,
}

pub(super) fn refresh_summary(report: &RefreshReport) -> &'static str {
    match report.m3u() {
        RefreshOutcome::Failed { .. } => "The channel source couldn’t refresh.",
        _ if matches!(report.epg(), Some(RefreshOutcome::Failed { .. })) => {
            "The Programme Guide couldn’t refresh."
        }
        RefreshOutcome::Skipped {
            reason: RefreshSkipReason::Fresh,
            ..
        } => "The channel source was already fresh, so only the guide was refreshed.",
        RefreshOutcome::NotModified { .. } => {
            "The channel source has not changed since the last check."
        }
        _ if report.status().generation().is_none() => "No catalog is available yet.",
        _ => "The catalog is up to date.",
    }
}

impl State {
    async fn run(
        self: Arc<Self>,
        data_dir: PathBuf,
        mut commands: mpsc::UnboundedReceiver<Command>,
        preference_updates: mpsc::Receiver<PreferenceWrite>,
    ) {
        let Ok(Ok(prepared)) =
            tokio::task::spawn_blocking(move || InstalledCatalog::prepare(&data_dir)).await
        else {
            self.startup_failed();
            return;
        };
        let Ok(catalog) = prepared.finish_open().await else {
            self.startup_failed();
            return;
        };
        let _ = catalog.load_guide_preferences().await;
        self.publish_variant_preferences(&catalog);
        if let Some(host) = self.playback.as_deref()
            && let Some(runner) = host.start(&catalog)
        {
            let _ = self.runner.set(runner);
        }
        tokio::spawn(Arc::clone(&self).write_preferences(Arc::clone(&catalog), preference_updates));
        let _ = self.catalog.set(Arc::clone(&catalog));
        let core = catalog.core();
        let mut events = core.subscribe();
        self.publish_status(catalog.status(), false);

        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::HostActive { id, active }) => {
                        if let Some(runner) = self.runner.get() {
                            if let Some(playback) = self.playback.as_deref() {
                                playback.host_active(id, active);
                            }
                            let _ = runner.host_visibility(id, active).await;
                        }
                        self.publish_host(core, id, active);
                    }
                    Some(Command::HostDestroyed { id, finishing }) => {
                        if let Some(runner) = self.runner.get() {
                            let _ = runner.commands().send(TvPlaybackCommand::HostDestroyed { host: id, finishing }).await;
                        }
                        self.publish_host(core, id, false);
                    }
                    Some(Command::ReconcileStatus) => self.publish_status(core.status(), true),
                    #[cfg(feature = "fixture")]
                    Some(Command::Seed { m3u, epg, completed }) => self.seed_fixture(&catalog, m3u, epg, completed),
                    #[cfg(feature = "fixture")]
                    Some(Command::CheckHttps(url)) => self.check_https(url),
                    None => break,
                },
                event = events.recv() => match event {
                    // The event can be queued behind a reconciliation or resume that
                    // already published newer state. Read the latest status here.
                    Some(CoreEvent::CatalogStatusChanged { .. }) => self.publish_status(core.status(), false),
                    Some(_) => {}
                    None => break,
                }
            }
        }
    }

    fn publish_variant_preferences(&self, catalog: &InstalledCatalog) {
        let preferences = catalog.guide_preferences();
        self.variant_preferences
            .send_replace(Arc::new(VariantPreferences::from_normalized(
                preferences.variant_choices(),
            )));
    }

    async fn write_preferences(
        self: Arc<Self>,
        catalog: Arc<InstalledCatalog>,
        mut requests: mpsc::Receiver<PreferenceWrite>,
    ) {
        while let Some(request) = requests.recv().await {
            match request {
                PreferenceWrite::Backdrop { choice, completed } => {
                    let saved = catalog
                        .update_guide_preferences(
                            sparrow_installed_catalog::GuidePreferenceWrite::SetBackdrop(choice),
                        )
                        .await
                        .is_ok();
                    let _ = completed.send(saved);
                }
                PreferenceWrite::SetAmbientLight { enabled, completed } => {
                    let saved = catalog
                        .update_guide_preferences(
                            sparrow_installed_catalog::GuidePreferenceWrite::SetAmbientLight {
                                enabled,
                            },
                        )
                        .await
                        .is_ok();
                    let _ = completed.send(saved);
                }
                PreferenceWrite::Group {
                    sequence,
                    change,
                    completed,
                } => {
                    let saved = catalog.update_guide_preferences(vec![change]).await.is_ok();
                    self.group_revision.send_replace(sequence);
                    let _ = completed.send(saved);
                }
                PreferenceWrite::Variant { change, completed } => {
                    let saved = catalog.update_guide_preferences(vec![change]).await.is_ok();
                    self.publish_variant_preferences(&catalog);
                    let _ = completed.send(saved);
                }
            }
        }
    }

    fn publish_host(&self, core: &CatalogAccess, id: u64, active: bool) {
        self.publication.send_if_modified(|state| {
            let was_active = state.snapshot.active;
            if active == state.active_hosts.contains_key(&id) {
                return false;
            }
            if active {
                state.next_activation = state.next_activation.wrapping_add(1);
                state.active_hosts.insert(id, state.next_activation);
            } else {
                state.active_hosts.remove(&id);
            }
            // An older Activity may stop after its replacement has already started.
            state.snapshot.active = !state.active_hosts.is_empty();
            if state.snapshot.active != was_active && self.runner.get().is_none() {
                let signal = if state.snapshot.active {
                    LifecycleSignal::Resumed
                } else {
                    LifecycleSignal::Suspended
                };
                core.report_lifecycle(signal);
            }
            if active {
                state.set_status(core.status(), false);
            }
            true
        });
    }

    fn modify_snapshot(&self, update: impl FnOnce(&mut Snapshot)) {
        self.publication.send_if_modified(|state| {
            let previous = state.snapshot;
            update(&mut state.snapshot);
            state.snapshot != previous
        });
    }

    fn startup_failed(&self) {
        log::warn!("Channel Catalog startup failed");
        self.modify_snapshot(|state| state.phase = Phase::StartupFailed);
    }

    fn publish_status(&self, status: CatalogStatus, reconcile: bool) {
        self.publication
            .send_if_modified(|state| state.set_status(status, reconcile));
    }

    #[cfg(feature = "fixture")]
    fn seed_fixture(
        self: &Arc<Self>,
        catalog: &Arc<InstalledCatalog>,
        m3u: String,
        epg: String,
        completed: oneshot::Sender<bool>,
    ) {
        if self.publication.borrow().snapshot.seed == FixtureSeed::Running {
            return;
        }
        self.modify_snapshot(|state| state.seed = FixtureSeed::Running);
        let catalog = Arc::clone(catalog);
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let update = SourceConfigurationUpdate::parse(m3u, Some(epg));
            let ready = match update {
                Ok(update) => catalog
                    .replace_configuration(update)
                    .await
                    .is_ok_and(|status| fixture_ready(&status)),
                Err(_) => false,
            };
            state.modify_snapshot(|state| {
                state.seed = if ready {
                    FixtureSeed::Complete
                } else {
                    FixtureSeed::Failed
                };
            });
            let _ = completed.send(ready);
        });
    }

    #[cfg(feature = "fixture")]
    fn check_https(self: &Arc<Self>, url: String) {
        if self.publication.borrow().snapshot.https == HttpsProbe::Running {
            return;
        }
        self.modify_snapshot(|state| state.https = HttpsProbe::Running);
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .redirect(reqwest::redirect::Policy::none())
                .build();
            let verified = match client {
                Ok(client) => client.head(url).send().await.is_ok(),
                Err(_) => false,
            };
            state.modify_snapshot(|state| {
                state.https = if verified {
                    HttpsProbe::Verified
                } else {
                    HttpsProbe::Failed
                };
            });
        });
    }
}

#[cfg(feature = "fixture")]
fn fixture_ready(status: &CatalogStatus) -> bool {
    status.generation().is_some()
        && matches!(
            status.epg(),
            Some(
                SourceState::Fresh { .. }
                    | SourceState::Stale { .. }
                    | SourceState::Refreshing {
                        validated_at: Some(_),
                        ..
                    }
                    | SourceState::Deferred {
                        validated_at: Some(_),
                        ..
                    }
                    | SourceState::Failed {
                        validated_at: Some(_),
                        ..
                    }
            )
        )
}
