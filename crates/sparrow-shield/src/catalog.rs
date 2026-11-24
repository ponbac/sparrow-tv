//! Process-owned installed catalog and Activity lifecycle delivery.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, LazyLock, OnceLock},
};

use sparrow_core::{CatalogAccess, CatalogStatus, CoreEvent, LifecycleSignal, SourceState};
use sparrow_installed_catalog::InstalledCatalog;
use tokio::{
    runtime::{Builder, Runtime},
    sync::{mpsc, watch},
};

#[cfg(feature = "fixture")]
use {sparrow_installed_catalog::SourceConfigurationUpdate, std::time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Starting,
    Unconfigured,
    Unavailable,
    Ready,
    StartupFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HttpsProbe {
    Idle,
    #[cfg(feature = "fixture")]
    Running,
    #[cfg(feature = "fixture")]
    Verified,
    #[cfg(feature = "fixture")]
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FixtureSeed {
    Idle,
    #[cfg(feature = "fixture")]
    Running,
    #[cfg(feature = "fixture")]
    Complete,
    #[cfg(feature = "fixture")]
    Failed,
}

/// Allowlisted diagnostics shared with GPUI and Android's main thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Snapshot {
    pub(super) phase: Phase,
    pub(super) generation_changes: u64,
    pub(super) active: bool,
    pub(super) refreshing: bool,
    pub(super) https: HttpsProbe,
    pub(super) seed: FixtureSeed,
}

struct Process {
    catalog: OnceLock<Arc<InstalledCatalog>>,
    worker: OnceLock<Option<Worker>>,
    publication: watch::Sender<Publication>,
}

/// The Guide's typed catalog state and the activity that owns each active window.
pub(super) struct Publication {
    snapshot: Snapshot,
    active_activities: HashMap<u64, u64>,
    next_activation: u64,
    pub(super) status: Option<Arc<CatalogStatus>>,
    pub(super) status_revision: u64,
}

impl Publication {
    pub(super) fn activation(&self, host: u64) -> Option<u64> {
        self.active_activities.get(&host).copied()
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
    runtime: Runtime,
}

enum Command {
    ActivityActive {
        id: u64,
        active: bool,
    },
    ReconcileStatus,
    #[cfg(feature = "fixture")]
    Seed,
    #[cfg(feature = "fixture")]
    CheckHttps,
}

static PROCESS: LazyLock<Process> = LazyLock::new(|| Process {
    catalog: OnceLock::new(),
    worker: OnceLock::new(),
    publication: watch::channel(Publication {
        snapshot: Snapshot {
            phase: Phase::Starting,
            generation_changes: 0,
            active: false,
            refreshing: false,
            https: HttpsProbe::Idle,
            seed: FixtureSeed::Idle,
        },
        active_activities: HashMap::new(),
        next_activation: 0,
        status: None,
        status_revision: 0,
    })
    .0,
});

pub(super) fn start(data_dir: PathBuf) {
    PROCESS.worker.get_or_init(|| {
        let runtime = match Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .max_blocking_threads(4)
            .thread_name("sparrow-catalog")
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                startup_failed();
                return None;
            }
        };
        modify_snapshot(|state| state.phase = Phase::Starting);
        let (commands, pending) = mpsc::unbounded_channel();
        runtime.spawn(run(data_dir, pending));
        Some(Worker { commands, runtime })
    });
}

#[cfg(feature = "fixture")]
pub(super) fn fixture_command(command: i32) -> bool {
    let command = match command {
        1 => Command::Seed,
        2 => Command::CheckHttps,
        _ => return false,
    };
    PROCESS
        .worker
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|worker| worker.commands.send(command).is_ok())
}

pub(super) fn set_active(id: u64, active: bool) {
    if let Some(Some(worker)) = PROCESS.worker.get() {
        let _ = worker.commands.send(Command::ActivityActive { id, active });
    }
}

pub(super) fn set_start_failed() {
    // Failure on a later Activity cannot replace the running catalog.
    if PROCESS.worker.get().is_none() {
        startup_failed();
    }
}

pub(super) fn subscribe() -> watch::Receiver<Publication> {
    PROCESS.publication.subscribe()
}

pub(super) fn reconcile_status() {
    if let Some(Some(worker)) = PROCESS.worker.get() {
        let _ = worker.commands.send(Command::ReconcileStatus);
    }
}

pub(super) fn diagnostics() -> (Snapshot, usize) {
    (
        PROCESS.publication.borrow().snapshot,
        PROCESS.publication.receiver_count(),
    )
}

pub(super) fn read<R: Send + 'static>(
    read: impl FnOnce(&CatalogAccess) -> R + Send + 'static,
) -> Option<tokio::task::JoinHandle<R>> {
    let worker = PROCESS.worker.get()?.as_ref()?;
    let core = PROCESS.catalog.get()?.core().clone();
    Some(worker.runtime.spawn_blocking(move || read(&core)))
}

async fn run(data_dir: PathBuf, mut commands: mpsc::UnboundedReceiver<Command>) {
    let Ok(Ok(prepared)) =
        tokio::task::spawn_blocking(move || InstalledCatalog::prepare(&data_dir)).await
    else {
        startup_failed();
        return;
    };
    let Ok(catalog) = prepared.finish_open().await else {
        startup_failed();
        return;
    };
    let _ = PROCESS.catalog.set(Arc::clone(&catalog));
    let core = catalog.core();
    let mut events = core.subscribe();
    core.report_lifecycle(LifecycleSignal::Started);
    publish_status(catalog.status(), false);

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::ActivityActive { id, active }) => {
                    publish_activity(core, id, active);
                }
                Some(Command::ReconcileStatus) => publish_status(core.status(), true),
                #[cfg(feature = "fixture")]
                Some(Command::Seed) => seed_fixture(&catalog),
                #[cfg(feature = "fixture")]
                Some(Command::CheckHttps) => check_platform_https(),
                None => break,
            },
            event = events.recv() => match event {
                // The event can be queued behind a reconciliation or resume that
                // already published newer state. Read the latest status here.
                Some(CoreEvent::CatalogStatusChanged { .. }) => publish_status(core.status(), false),
                Some(_) => {}
                None => break,
            }
        }
    }
}

fn publish_activity(core: &CatalogAccess, id: u64, active: bool) {
    PROCESS.publication.send_if_modified(|state| {
        let was_active = state.snapshot.active;
        if active == state.active_activities.contains_key(&id) {
            return false;
        }
        if active {
            state.next_activation = state.next_activation.wrapping_add(1);
            state.active_activities.insert(id, state.next_activation);
        } else {
            state.active_activities.remove(&id);
        }
        // An older Activity may stop after its replacement has already started.
        state.snapshot.active = !state.active_activities.is_empty();
        if state.snapshot.active != was_active {
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

fn modify_snapshot(update: impl FnOnce(&mut Snapshot)) {
    PROCESS.publication.send_if_modified(|state| {
        let previous = state.snapshot;
        update(&mut state.snapshot);
        state.snapshot != previous
    });
}

fn startup_failed() {
    log::warn!("Shield Channel Catalog startup failed");
    modify_snapshot(|state| state.phase = Phase::StartupFailed);
}

fn publish_status(status: CatalogStatus, reconcile: bool) {
    PROCESS
        .publication
        .send_if_modified(|state| state.set_status(status, reconcile));
}

#[cfg(feature = "fixture")]
fn seed_fixture(catalog: &Arc<InstalledCatalog>) {
    if PROCESS.publication.borrow().snapshot.seed == FixtureSeed::Running {
        return;
    }
    modify_snapshot(|state| state.seed = FixtureSeed::Running);
    let catalog = Arc::clone(catalog);
    tokio::spawn(async move {
        let update = SourceConfigurationUpdate::parse(
            "http://127.0.0.1:18108/catalog.m3u".into(),
            Some("http://127.0.0.1:18108/guide.xml".into()),
        );
        let ready = match update {
            Ok(update) => catalog
                .replace_configuration(update)
                .await
                .is_ok_and(|status| fixture_ready(&status)),
            Err(_) => false,
        };
        modify_snapshot(|state| {
            state.seed = if ready {
                FixtureSeed::Complete
            } else {
                FixtureSeed::Failed
            };
        });
    });
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

#[cfg(feature = "fixture")]
fn check_platform_https() {
    if PROCESS.publication.borrow().snapshot.https == HttpsProbe::Running {
        return;
    }
    modify_snapshot(|state| state.https = HttpsProbe::Running);
    tokio::spawn(async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build();
        let verified = match client {
            Ok(client) => client.head("https://example.com/").send().await.is_ok(),
            Err(_) => false,
        };
        modify_snapshot(|state| {
            state.https = if verified {
                HttpsProbe::Verified
            } else {
                HttpsProbe::Failed
            };
        });
    });
}
