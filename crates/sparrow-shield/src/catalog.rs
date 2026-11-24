//! Process-owned installed catalog and Activity lifecycle delivery.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{LazyLock, OnceLock},
};

use sparrow_core::{CatalogGeneration, CatalogStatus, LifecycleSignal, SourceState};
use sparrow_installed_catalog::InstalledCatalog;
use tokio::{
    runtime::{Builder, Runtime},
    sync::{mpsc, watch},
};

#[cfg(feature = "fixture")]
use {
    sparrow_installed_catalog::SourceConfigurationUpdate,
    std::{sync::Arc, time::Duration},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Starting,
    Unconfigured,
    Unavailable,
    Ready,
    StartupFailed,
}

impl Phase {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Starting => "Opening Channel Catalog",
            Self::Unconfigured => "No source configured",
            Self::Unavailable => "Channel Catalog unavailable",
            Self::Ready => "Channel Catalog ready",
            Self::StartupFailed => "Channel Catalog could not open",
        }
    }
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
    worker: OnceLock<Option<Worker>>,
    snapshot: watch::Sender<Snapshot>,
}

struct Worker {
    commands: mpsc::UnboundedSender<Command>,
    _runtime: Runtime,
}

enum Command {
    ActivityActive {
        id: u64,
        active: bool,
    },
    #[cfg(feature = "fixture")]
    Seed,
    #[cfg(feature = "fixture")]
    CheckHttps,
}

static PROCESS: LazyLock<Process> = LazyLock::new(|| Process {
    worker: OnceLock::new(),
    snapshot: watch::channel(Snapshot {
        phase: Phase::Starting,
        generation_changes: 0,
        active: false,
        refreshing: false,
        https: HttpsProbe::Idle,
        seed: FixtureSeed::Idle,
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
        Some(Worker {
            commands,
            _runtime: runtime,
        })
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

pub(super) fn subscribe() -> watch::Receiver<Snapshot> {
    PROCESS.snapshot.subscribe()
}

pub(super) fn diagnostics() -> (Snapshot, usize) {
    (
        *PROCESS.snapshot.borrow(),
        PROCESS.snapshot.receiver_count(),
    )
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
    let core = catalog.core();
    let mut events = core.subscribe();
    let mut last_generation = None;
    // An older Activity may stop after its replacement has already started.
    let mut active_activities = HashSet::new();
    core.report_lifecycle(LifecycleSignal::Started);
    let initial = catalog.status();
    publish_status(&initial, &mut last_generation);

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::ActivityActive { id, active }) => {
                    let was_active = !active_activities.is_empty();
                    if active {
                        active_activities.insert(id);
                    } else {
                        active_activities.remove(&id);
                    }
                    let is_active = !active_activities.is_empty();
                    if is_active != was_active {
                        let signal = if is_active { LifecycleSignal::Resumed } else { LifecycleSignal::Suspended };
                        core.report_lifecycle(signal);
                        modify_snapshot(|state| state.active = is_active);
                    }
                }
                #[cfg(feature = "fixture")]
                Some(Command::Seed) => seed_fixture(&catalog),
                #[cfg(feature = "fixture")]
                Some(Command::CheckHttps) => check_platform_https(),
                None => break,
            },
            event = events.recv() => {
                if event.is_none() {
                    break;
                }
                publish_status(&core.status(), &mut last_generation);
            }
        }
    }
}

fn modify_snapshot(update: impl FnOnce(&mut Snapshot)) {
    PROCESS.snapshot.send_if_modified(|state| {
        let previous = *state;
        update(state);
        *state != previous
    });
}

fn startup_failed() {
    log::warn!("Shield Channel Catalog startup failed");
    modify_snapshot(|state| state.phase = Phase::StartupFailed);
}

fn publish_status(status: &CatalogStatus, last_generation: &mut Option<CatalogGeneration>) {
    let generation = status.generation();
    let generation_changed = generation != *last_generation;
    *last_generation = generation;
    modify_snapshot(|state| {
        state.refreshing = matches!(status.m3u(), SourceState::Refreshing { .. })
            || matches!(status.epg(), Some(SourceState::Refreshing { .. }));
        state.phase = if generation.is_some() {
            Phase::Ready
        } else if !status.configuration().is_configured() {
            Phase::Unconfigured
        } else if state.refreshing {
            Phase::Starting
        } else {
            Phase::Unavailable
        };
        if generation_changed {
            state.generation_changes += 1;
        }
    });
}

#[cfg(feature = "fixture")]
fn seed_fixture(catalog: &Arc<InstalledCatalog>) {
    if PROCESS.snapshot.borrow().seed == FixtureSeed::Running {
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
    if PROCESS.snapshot.borrow().https == HttpsProbe::Running {
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
