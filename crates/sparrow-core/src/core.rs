use arc_swap::ArcSwap;
use futures_util::{FutureExt, StreamExt};
use std::{
    collections::HashSet,
    io::{self, BufRead, Read},
    ops::Deref,
    panic::{AssertUnwindSafe, resume_unwind},
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, Notify, broadcast, oneshot, watch};

use crate::{
    catalog::ChannelCatalog,
    domain::{
        CatalogGeneration, CatalogStatus, ChannelDetails, ChannelDirection, ChannelFamily,
        ChannelGroupView, ChannelId, ChannelQuery, ChannelSummary, CoreError, CoreEvent,
        GuideWindowChannel, GuideWindowQuery, LifecycleSignal, Page, PageRequest, ProgrammeSummary,
        RefreshOutcome, RefreshReport, RefreshTrigger, ResolvedPlaybackSource, SafeFailure,
        ScheduleQuery, SearchRequest, SearchResults, SearchTerm, SnapshotOperation,
        SnapshotRecoveryDiagnostic, SnapshotRecoveryReason, SourceAccessError, SourceConfiguration,
        SourceConfigurationInput, SourceKind, SourceState,
    },
    m3u,
    ports::{
        CoreAdapters, PrivateSourceValidators, SnapshotCandidate, SnapshotMetadata,
        SnapshotRevalidation, SnapshotSource, SnapshotStage, SnapshotStageRequest, SourceRequest,
        SourceResponseInner, ValidatedStage,
    },
    xmltv,
};

const M3U_DECODED_LIMIT: u64 = 128 * 1024 * 1024;
const EPG_DECODED_LIMIT: u64 = 64 * 1024 * 1024;
const BLOCKING_OFFLOAD_BYTES: u64 = 32 * 1024;
const BLOCKING_OFFLOAD_CHANNELS: usize = 64;
const FRESHNESS: chrono::Duration = chrono::Duration::hours(6);
const EVENT_CAPACITY: usize = 32;
const BACKOFF_MINUTES: [i64; 4] = [1, 5, 15, 60];

/// Owns the Source Configuration of a Sparrow catalog.
#[derive(Clone)]
pub struct SparrowCore {
    catalog: CatalogAccess,
}

/// Shareable queries, playback access, and refresh lifecycle for a Channel Catalog.
#[derive(Clone)]
pub struct CatalogAccess {
    runtime: Arc<CoreRuntime>,
}

impl Deref for SparrowCore {
    type Target = CatalogAccess;

    fn deref(&self) -> &Self::Target {
        self.catalog()
    }
}

impl SparrowCore {
    pub fn catalog(&self) -> &CatalogAccess {
        &self.catalog
    }

    /// Validates and refines user-supplied source locations without exposing them.
    pub fn parse_source_configuration(
        input: SourceConfigurationInput,
    ) -> Result<SourceConfiguration, CoreError> {
        SourceConfiguration::parse(input)
    }

    /// Builds a usable core after independently validating the required M3U and
    /// optional EPG Source Snapshots. EPG failure yields a Channel-only catalog.
    pub async fn bootstrap(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
    ) -> Result<Self, CoreError> {
        let Some(configuration) = configuration else {
            let core = Self::from_runtime(CoreRuntime::new(
                None,
                adapters,
                CoreView::not_configured(),
                BootstrapFailures::default(),
            ));
            core.runtime.start_automation();
            return Ok(core);
        };

        let redacted = configuration.redacted();
        let (view, bootstrap_failures) = match load_catalog(&configuration, &adapters).await {
            Ok(loaded) => {
                let bootstrap_failures = loaded.bootstrap_failures;
                let status = CatalogStatus::published(
                    loaded.catalog.generation(),
                    redacted,
                    loaded.m3u,
                    loaded.epg,
                    loaded.m3u_recovery,
                    loaded.epg_recovery,
                );
                (
                    CoreView {
                        status,
                        catalog: Some(Arc::new(loaded.catalog)),
                        sources: loaded.sources,
                    },
                    bootstrap_failures,
                )
            }
            Err(failure) => {
                let bootstrap_failure = failure.failure.clone();
                let mut status = CatalogStatus::unavailable(redacted, Some(failure.failure));
                status.set_recovery(SourceKind::M3u, failure.m3u_recovery);
                status.set_recovery(SourceKind::Epg, failure.epg_recovery);
                (
                    CoreView {
                        status,
                        catalog: None,
                        sources: PublishedSources::default(),
                    },
                    BootstrapFailures {
                        m3u: Some(bootstrap_failure),
                        epg: None,
                    },
                )
            }
        };

        let core = Self::from_runtime(CoreRuntime::new(
            Some(configuration),
            adapters,
            view,
            bootstrap_failures,
        ));
        core.runtime.start_automation();

        Ok(core)
    }

    /// Builds a usable core strictly from eligible on-device snapshots.
    ///
    /// Source access is never awaited by this call. The caller hands the recovered
    /// view to its client, then calls [`CatalogAccess::activate_automation`] to refresh
    /// configured sources in the background. This keeps an installed shell
    /// responsive when a provider is offline while preserving any matching
    /// catalog snapshot.
    pub async fn bootstrap_from_snapshots(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
    ) -> Result<Self, CoreError> {
        Self::bootstrap_snapshots(configuration, adapters, false).await
    }

    /// Opens a processed catalog when possible. On a miss, makes Channels usable
    /// first and restores the saved EPG on a blocking worker after activation.
    pub async fn bootstrap_installed(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
    ) -> Result<Self, CoreError> {
        Self::bootstrap_snapshots(configuration, adapters, true).await
    }

    async fn bootstrap_snapshots(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
        progressive: bool,
    ) -> Result<Self, CoreError> {
        let Some(configuration) = configuration else {
            return Ok(Self::from_runtime(CoreRuntime::new(
                None,
                adapters,
                CoreView::not_configured(),
                BootstrapFailures::default(),
            )));
        };

        let configuration = Arc::new(configuration);
        let freshness = if progressive {
            RecoveredFreshness::AgeBased
        } else {
            RecoveredFreshness::PendingRevalidation
        };
        let cached = recover_catalog_cache(&configuration, &adapters, freshness).await;
        let cache_hit = cached.is_some();
        let defer_epg = cached.is_none() && progressive && configuration.has_epg();
        let mut recovered = match cached {
            Some(view) => RecoveredConfiguration { view },
            None => {
                recover_configuration_parts(
                    Arc::clone(&configuration),
                    &adapters,
                    freshness,
                    !defer_epg,
                )
                .await
            }
        };
        if defer_epg {
            recovered.view.status.set_source_state(
                SourceKind::Epg,
                SourceState::Refreshing {
                    validated_at: None,
                    started_at: adapters.clock().now(),
                },
            );
        }
        let core = Self::from_runtime(CoreRuntime::new_shared(
            Some(configuration),
            adapters,
            recovered.view,
            BootstrapFailures::default(),
        ));
        if defer_epg {
            *core
                .runtime
                .deferred_epg_restore
                .lock()
                .expect("recovery state poisoned") =
                core.runtime.ready_configuration(SourceKind::Epg);
        } else if !cache_hit {
            if progressive {
                core.runtime.cache_on_start.store(true, Ordering::Release);
            } else {
                core.runtime.queue_catalog_cache(false);
            }
        }
        Ok(core)
    }

    /// Replaces the private Source Configuration through one serialized transition.
    ///
    /// The previous catalog becomes ineligible before this method waits for old
    /// refresh work. A configured replacement recovers matching on-device
    /// snapshots, then performs a foreground refresh. Fetch failure is represented
    /// in the returned safe status while the newly selected configuration remains
    /// active. Passing `None` removes the configuration.
    pub async fn replace_source_configuration(
        &self,
        configuration: Option<SourceConfiguration>,
    ) -> CatalogStatus {
        self.runtime.replace_configuration(configuration).await
    }

    fn from_runtime(runtime: CoreRuntime) -> Self {
        Self {
            catalog: CatalogAccess {
                runtime: Arc::new(runtime),
            },
        }
    }
}

impl CatalogAccess {
    /// Starts background source automation once the snapshot-backed status has
    /// been handed to the installed client. Repeated calls are harmless.
    pub fn activate_automation(&self) {
        self.runtime.start_automation();
    }

    pub fn status(&self) -> CatalogStatus {
        self.runtime.view.load().status.clone()
    }

    /// Returns a deterministic bounded page of Channel Groups in first-seen M3U order.
    pub fn list_groups(&self, request: PageRequest) -> Result<Page<ChannelGroupView>, CoreError> {
        self.query_catalog(|catalog| catalog.groups_page(&request))
    }

    /// Returns a deterministic bounded page of all Channels or one exact group.
    ///
    /// Channels keep M3U source order within each Channel Group. The unfiltered
    /// catalog concatenates those groups in first-seen order rather than sorting
    /// names alphabetically.
    pub fn list_channels(&self, query: ChannelQuery) -> Result<Page<ChannelSummary>, CoreError> {
        self.query_catalog(|catalog| catalog.channels_page(&query))
    }

    /// Resolves a Channel Identifier to its Channel Number in the requested
    /// publication. An absent Channel yields no number.
    pub fn channel_number(
        &self,
        expected: CatalogGeneration,
        id: &ChannelId,
    ) -> Result<Option<u32>, CoreError> {
        self.query_generation(expected, |catalog| catalog.channel_number(id))
    }

    /// Returns all Quality Variants for a Channel Number in the requested
    /// publication. An absent number yields no family. A different published
    /// generation yields [`CoreError::StaleCursor`].
    pub fn channel_family(
        &self,
        expected: CatalogGeneration,
        number: u32,
    ) -> Result<Option<ChannelFamily>, CoreError> {
        self.query_generation(expected, |catalog| catalog.channel_family(number))
    }

    /// Finds one adjacent visible family in whole-catalog order. The starting
    /// family may itself be hidden. At either end there is no neighbor. A
    /// different published generation yields [`CoreError::StaleCursor`].
    pub fn adjacent_channel_family(
        &self,
        expected: CatalogGeneration,
        number: u32,
        direction: ChannelDirection,
        excluded_groups: &HashSet<String>,
    ) -> Result<Option<ChannelFamily>, CoreError> {
        self.query_generation(expected, |catalog| {
            catalog.adjacent_channel_family(number, direction, excluded_groups)
        })
    }

    pub fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        self.query_catalog(|catalog| catalog.channel(id))
    }

    /// Resolves one opaque Channel Identifier to a private, publication-pinned
    /// Playback Source for a privileged Rust adapter.
    ///
    /// The adapter must acquire [`Self::begin_playback_activity`] before
    /// resolution and retain that lease for the upstream connection lifetime.
    pub fn resolve_playback(&self, id: &ChannelId) -> Result<ResolvedPlaybackSource, CoreError> {
        self.query_catalog(|catalog| catalog.resolve_playback(id))
    }

    /// Returns a deterministic bounded Programme page for one Channel.
    pub fn schedule(&self, query: ScheduleQuery) -> Result<Page<ProgrammeSummary>, CoreError> {
        self.query_catalog(|catalog| catalog.schedule(&query))
    }

    /// Returns one bounded Channel page with Programmes overlapping a UTC window.
    pub fn guide_window(
        &self,
        query: GuideWindowQuery,
    ) -> Result<Page<GuideWindowChannel>, CoreError> {
        self.query_catalog(|catalog| catalog.guide_window(&query))
    }

    /// Searches Channels and Programmes without reparsing the active Sources.
    pub fn search(&self, request: SearchRequest) -> Result<SearchResults, CoreError> {
        self.query_catalog(|catalog| catalog.search(&request))
    }

    /// Searches both result lanes while cooperatively observing adapter cancellation.
    pub fn search_with_cancellation(
        &self,
        request: SearchRequest,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<SearchResults, CoreError> {
        self.query_catalog(|catalog| catalog.search_with_cancellation(&request, &is_cancelled))
    }

    /// Searches only Channels without ranking Programme documents.
    pub fn search_channels(
        &self,
        term: SearchTerm,
        page: PageRequest,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        self.query_catalog(|catalog| catalog.search_channels(&term, &page))
    }

    /// Searches only Channels while cooperatively observing adapter cancellation.
    pub fn search_channels_with_cancellation(
        &self,
        term: SearchTerm,
        page: PageRequest,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        self.query_catalog(|catalog| {
            catalog.search_channels_with_cancellation(&term, &page, &is_cancelled)
        })
    }

    /// Searches only Programmes without ranking Channel documents.
    pub fn search_programmes(
        &self,
        term: SearchTerm,
        page: PageRequest,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        self.query_catalog(|catalog| catalog.search_programmes(&term, &page))
    }

    /// Searches only Programmes while cooperatively observing adapter cancellation.
    pub fn search_programmes_with_cancellation(
        &self,
        term: SearchTerm,
        page: PageRequest,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        self.query_catalog(|catalog| {
            catalog.search_programmes_with_cancellation(&term, &page, &is_cancelled)
        })
    }

    /// Refreshes configured Sources independently. Concurrent requests for one
    /// Source share the same in-flight result.
    pub async fn refresh(&self, trigger: RefreshTrigger) -> RefreshReport {
        self.runtime.refresh(trigger).await
    }

    /// Reports lifecycle facts without moving refresh policy into a shell.
    pub fn report_lifecycle(&self, signal: LifecycleSignal) {
        let trigger = match signal {
            LifecycleSignal::Started => Some(RefreshTrigger::Startup),
            LifecycleSignal::Resumed => Some(RefreshTrigger::Resume),
            LifecycleSignal::Suspended => None,
        };
        if let Some(trigger) = trigger {
            self.runtime.spawn_refresh(trigger);
        }
    }

    /// Defers automatic refresh work until the returned non-cloneable lease is dropped.
    pub fn begin_playback_activity(&self) -> PlaybackActivityLease {
        self.runtime.begin_playback();
        PlaybackActivityLease {
            runtime: Arc::clone(&self.runtime),
        }
    }

    /// Subscribes to a bounded safe event feed. Slow consumers skip old events.
    pub fn subscribe(&self) -> CoreEventStream {
        let _publication = self
            .runtime
            .publication
            .lock()
            .expect("publication lock poisoned");
        CoreEventStream {
            receiver: self.runtime.events.subscribe(),
            runtime: Arc::downgrade(&self.runtime),
            initial: Some(CoreEvent::CatalogStatusChanged {
                occurred_at: self.runtime.adapters.clock().now(),
                status: self.runtime.view.load().status.clone(),
            }),
        }
    }

    fn query_generation<T>(
        &self,
        expected: CatalogGeneration,
        query: impl FnOnce(&ChannelCatalog) -> T,
    ) -> Result<T, CoreError> {
        self.query_catalog(|catalog| {
            let current = catalog.generation();
            if current != expected {
                return Err(CoreError::StaleCursor { current });
            }
            Ok(query(catalog))
        })
    }

    fn query_catalog<T>(
        &self,
        query: impl FnOnce(&ChannelCatalog) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let view = self.runtime.view.load_full();
        if !view.status.configuration().is_configured() {
            return Err(CoreError::NotConfigured);
        }
        match &view.catalog {
            Some(catalog) => query(catalog),
            None => Err(CoreError::CatalogUnavailable {
                status: Box::new(view.status.clone()),
            }),
        }
    }
}

pub struct PlaybackActivityLease {
    runtime: Arc<CoreRuntime>,
}

impl Drop for PlaybackActivityLease {
    fn drop(&mut self) {
        self.runtime.end_playback();
    }
}

pub struct CoreEventStream {
    receiver: broadcast::Receiver<CoreEvent>,
    runtime: Weak<CoreRuntime>,
    initial: Option<CoreEvent>,
}

impl CoreEventStream {
    pub async fn recv(&mut self) -> Option<CoreEvent> {
        if let Some(initial) = self.initial.take() {
            return Some(initial);
        }
        match self.receiver.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let runtime = self.runtime.upgrade()?;
                let (occurred_at, status) = {
                    let _publication = runtime
                        .publication
                        .lock()
                        .expect("publication lock poisoned");
                    self.receiver = runtime.events.subscribe();
                    (
                        runtime.adapters.clock().now(),
                        runtime.view.load().status.clone(),
                    )
                };
                Some(CoreEvent::CatalogStatusChanged {
                    occurred_at,
                    status,
                })
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

struct CoreRuntime {
    configuration: RwLock<ConfigurationState>,
    configuration_transition: AsyncMutex<()>,
    // Linearizes epoch invalidation with insertion of a new Source flight.
    configuration_admission: Mutex<()>,
    configuration_changed: watch::Sender<u64>,
    adapters: CoreAdapters,
    view: ArcSwap<CoreView>,
    publication: Mutex<()>,
    // Serializes heavy catalog construction while keeping source fetches concurrent.
    catalog_build: AsyncMutex<()>,
    m3u: SourceRefreshControl,
    epg: SourceRefreshControl,
    activity: Mutex<ActivityAdmission>,
    activity_changed: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    events: broadcast::Sender<CoreEvent>,
    automation_started: AtomicBool,
    deferred_epg_restore: Mutex<Option<ConfigurationContext>>,
    cache_on_start: AtomicBool,
    cache_worker: Mutex<CacheWorkerState>,
    cache_revision: Arc<AtomicU64>,
}

#[derive(Default)]
struct CacheWorkerState {
    dirty: bool,
    running: bool,
}

struct ConfigurationState {
    epoch: u64,
    configuration: Option<Arc<SourceConfiguration>>,
    ready: bool,
}

#[derive(Clone)]
struct ConfigurationContext {
    epoch: u64,
    configuration: Arc<SourceConfiguration>,
}

struct CoreView {
    status: CatalogStatus,
    catalog: Option<Arc<ChannelCatalog>>,
    sources: PublishedSources,
}

#[derive(Default)]
struct BootstrapFailures {
    m3u: Option<SafeFailure>,
    epg: Option<SafeFailure>,
}

impl CoreView {
    fn not_configured() -> Self {
        Self {
            status: CatalogStatus::not_configured(),
            catalog: None,
            sources: PublishedSources::default(),
        }
    }
}

#[derive(Clone, Default)]
struct PublishedSources {
    m3u: Option<SnapshotCandidate>,
    epg: Option<SnapshotCandidate>,
    /// Retained strictly while `CoreView.catalog` is `None` (EPG-first
    /// publication or EPG-only recovery before the first M3U succeeds).
    /// Cleared as soon as a `ChannelCatalog` owns the cold EPG tables.
    pending_epg: Option<Arc<xmltv::ParsedGuide>>,
}

struct SourceRefreshControl {
    flight: Mutex<Option<Arc<RefreshFlight>>>,
    policy: Mutex<RefreshPolicy>,
    reschedule: Arc<Notify>,
}

impl Default for SourceRefreshControl {
    fn default() -> Self {
        Self {
            flight: Mutex::new(None),
            policy: Mutex::new(RefreshPolicy::default()),
            reschedule: Arc::new(Notify::new()),
        }
    }
}

#[derive(Default)]
struct RefreshPolicy {
    consecutive_failures: usize,
    next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
}

struct RefreshFlight {
    context: ConfigurationContext,
    manual: AtomicBool,
    decision: Mutex<FlightDecision>,
    promoted: watch::Sender<bool>,
    result: watch::Sender<Option<RefreshFlightResult>>,
}

#[derive(Clone)]
enum RefreshFlightResult {
    Completed(RefreshOutcome),
    Panicked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlightDecision {
    Pending,
    Admitted,
    Skipped,
}

#[derive(Default)]
struct ActivityAdmission {
    playback_leases: usize,
    automatic_refreshes: usize,
}

struct AutomaticRefreshAdmission<'a> {
    runtime: &'a CoreRuntime,
}

impl Drop for AutomaticRefreshAdmission<'_> {
    fn drop(&mut self) {
        let mut activity = self
            .runtime
            .activity
            .lock()
            .expect("activity admission poisoned");
        activity.automatic_refreshes = activity
            .automatic_refreshes
            .checked_sub(1)
            .expect("automatic refresh admission underflowed");
        drop(activity);
        self.runtime
            .activity_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

impl RefreshFlight {
    fn new(context: ConfigurationContext, manual: bool) -> Self {
        Self {
            context,
            manual: AtomicBool::new(manual),
            decision: Mutex::new(FlightDecision::Pending),
            promoted: watch::channel(manual).0,
            result: watch::channel(None).0,
        }
    }

    fn try_promote(&self) -> bool {
        let decision = self.decision.lock().expect("refresh decision poisoned");
        if *decision == FlightDecision::Skipped {
            return false;
        }
        if !self.manual.swap(true, Ordering::AcqRel) {
            self.promoted.send_replace(true);
        }
        true
    }

    fn try_commit_skip(&self) -> bool {
        let mut decision = self.decision.lock().expect("refresh decision poisoned");
        if self.manual.load(Ordering::Acquire) {
            return false;
        }
        *decision = FlightDecision::Skipped;
        true
    }

    fn admit(&self) {
        let mut decision = self.decision.lock().expect("refresh decision poisoned");
        if *decision == FlightDecision::Pending {
            *decision = FlightDecision::Admitted;
        }
    }

    async fn wait(&self) -> RefreshOutcome {
        let mut result = self.result.subscribe();
        loop {
            if let Some(result) = result.borrow().clone() {
                return match result {
                    RefreshFlightResult::Completed(outcome) => outcome,
                    RefreshFlightResult::Panicked => panic!("the shared refresh task panicked"),
                };
            }
            result
                .changed()
                .await
                .expect("refresh flight remains alive until completion");
        }
    }

    async fn wait_until_finished(&self) {
        let mut result = self.result.subscribe();
        loop {
            if result.borrow().is_some() {
                return;
            }
            if result.changed().await.is_err() {
                return;
            }
        }
    }

    fn complete(&self, result: RefreshOutcome) {
        self.result
            .send_replace(Some(RefreshFlightResult::Completed(result)));
    }

    fn fail_panicked(&self) {
        self.result
            .send_replace(Some(RefreshFlightResult::Panicked));
    }
}

impl CoreRuntime {
    fn new(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
        view: CoreView,
        bootstrap_failures: BootstrapFailures,
    ) -> Self {
        Self::new_shared(
            configuration.map(Arc::new),
            adapters,
            view,
            bootstrap_failures,
        )
    }

    fn new_shared(
        configuration: Option<Arc<SourceConfiguration>>,
        adapters: CoreAdapters,
        view: CoreView,
        bootstrap_failures: BootstrapFailures,
    ) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let runtime = Self {
            configuration: RwLock::new(ConfigurationState {
                epoch: 0,
                configuration,
                ready: true,
            }),
            configuration_transition: AsyncMutex::new(()),
            configuration_admission: Mutex::new(()),
            configuration_changed: watch::channel(0).0,
            adapters,
            view: ArcSwap::from_pointee(view),
            publication: Mutex::new(()),
            catalog_build: AsyncMutex::new(()),
            m3u: SourceRefreshControl::default(),
            epg: SourceRefreshControl::default(),
            activity: Mutex::new(ActivityAdmission::default()),
            activity_changed: watch::channel(0).0,
            shutdown: watch::channel(false).0,
            events,
            automation_started: AtomicBool::new(false),
            deferred_epg_restore: Mutex::new(None),
            cache_on_start: AtomicBool::new(false),
            cache_worker: Mutex::new(CacheWorkerState::default()),
            cache_revision: Arc::new(AtomicU64::new(0)),
        };
        runtime.seed_bootstrap_failures(0, bootstrap_failures);
        runtime
    }

    fn seed_bootstrap_failures(&self, epoch: u64, failures: BootstrapFailures) {
        for (kind, failure) in [
            (SourceKind::M3u, failures.m3u),
            (SourceKind::Epg, failures.epg),
        ] {
            let Some(failure) = failure else {
                continue;
            };
            let _ = self.record_failure(epoch, kind, &failure);
        }
    }

    async fn replace_configuration(
        self: &Arc<Self>,
        configuration: Option<SourceConfiguration>,
    ) -> CatalogStatus {
        let (completed, result) = oneshot::channel();
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let status = runtime.run_configuration_replacement(configuration).await;
            let _ = completed.send(status);
        });
        result
            .await
            .expect("the owned Source Configuration transition completes")
    }

    async fn run_configuration_replacement(
        self: &Arc<Self>,
        configuration: Option<SourceConfiguration>,
    ) -> CatalogStatus {
        let _transition = self.configuration_transition.lock().await;
        let configuration = configuration.map(Arc::new);
        let epoch = self.invalidate_configuration(configuration.clone());
        let ((), ()) = futures_util::future::join(
            self.wait_for_previous_flight(SourceKind::M3u, epoch),
            self.wait_for_previous_flight(SourceKind::Epg, epoch),
        )
        .await;

        let Some(configuration) = configuration else {
            return self.view.load().status.clone();
        };

        let recovered = recover_configuration(
            Arc::clone(&configuration),
            &self.adapters,
            RecoveredFreshness::AgeBased,
        )
        .await;
        if !self.publish_recovered_configuration(epoch, recovered) {
            return self.view.load().status.clone();
        }
        let Some(context) = self.mark_configuration_ready(epoch) else {
            return self.view.load().status.clone();
        };
        self.queue_catalog_cache(false);
        let _ = self.refresh_in(RefreshTrigger::Manual, context).await;
        self.view.load().status.clone()
    }

    fn invalidate_configuration(&self, configuration: Option<Arc<SourceConfiguration>>) -> u64 {
        let status = configuration
            .as_ref()
            .map_or_else(CatalogStatus::not_configured, |configuration| {
                CatalogStatus::unavailable(configuration.redacted(), None)
            });
        let _admission = self
            .configuration_admission
            .lock()
            .expect("Source Configuration admission poisoned");
        let publication = self.publication.lock().expect("publication lock poisoned");
        let epoch = {
            let mut state = self
                .configuration
                .write()
                .expect("Source Configuration state poisoned");
            state.epoch = state
                .epoch
                .checked_add(1)
                .expect("Source Configuration epoch overflowed");
            state.configuration = configuration;
            state.ready = state.configuration.is_none();
            state.epoch
        };
        self.cache_revision.fetch_add(1, Ordering::AcqRel);
        for kind in [SourceKind::M3u, SourceKind::Epg] {
            let control = self.control(kind);
            *control.policy.lock().expect("refresh policy poisoned") = RefreshPolicy::default();
            control.reschedule.notify_one();
        }
        self.view.store(Arc::new(CoreView {
            status: status.clone(),
            catalog: None,
            sources: PublishedSources::default(),
        }));
        let _ = self.events.send(CoreEvent::CatalogStatusChanged {
            occurred_at: self.adapters.clock().now(),
            status,
        });
        drop(publication);
        drop(_admission);
        self.configuration_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        self.activity_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        epoch
    }

    async fn wait_for_previous_flight(&self, kind: SourceKind, epoch: u64) {
        loop {
            let previous = self
                .control(kind)
                .flight
                .lock()
                .expect("refresh flight poisoned")
                .as_ref()
                .filter(|flight| flight.context.epoch != epoch)
                .cloned();
            let Some(previous) = previous else {
                return;
            };
            previous.wait_until_finished().await;
        }
    }

    fn publish_recovered_configuration(
        &self,
        epoch: u64,
        recovered: RecoveredConfiguration,
    ) -> bool {
        let _publication = self.publication.lock().expect("publication lock poisoned");
        let current = self
            .configuration
            .read()
            .expect("Source Configuration state poisoned");
        if current.epoch != epoch || current.ready || current.configuration.is_none() {
            return false;
        }
        let status = recovered.view.status.clone();
        let generation = status.generation();
        self.view.store(Arc::new(recovered.view));
        let occurred_at = self.adapters.clock().now();
        let _ = self.events.send(CoreEvent::CatalogStatusChanged {
            occurred_at,
            status,
        });
        if let Some(generation) = generation {
            let _ = self.events.send(CoreEvent::CatalogPublished {
                occurred_at,
                generation,
            });
        }
        true
    }

    fn mark_configuration_ready(&self, epoch: u64) -> Option<ConfigurationContext> {
        let _admission = self
            .configuration_admission
            .lock()
            .expect("Source Configuration admission poisoned");
        let publication = self.publication.lock().expect("publication lock poisoned");
        let context = {
            let mut state = self
                .configuration
                .write()
                .expect("Source Configuration state poisoned");
            if state.epoch != epoch || state.ready {
                return None;
            }
            let configuration = state.configuration.clone()?;
            state.ready = true;
            ConfigurationContext {
                epoch,
                configuration,
            }
        };
        drop(publication);
        drop(_admission);
        self.configuration_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        Some(context)
    }

    fn control(&self, kind: SourceKind) -> &SourceRefreshControl {
        match kind {
            SourceKind::M3u => &self.m3u,
            SourceKind::Epg => &self.epg,
        }
    }

    fn begin_playback(&self) {
        let mut activity = self.activity.lock().expect("activity admission poisoned");
        activity.playback_leases = activity
            .playback_leases
            .checked_add(1)
            .expect("playback activity lease count overflowed");
        drop(activity);
        self.activity_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn end_playback(&self) {
        let mut activity = self.activity.lock().expect("activity admission poisoned");
        activity.playback_leases = activity
            .playback_leases
            .checked_sub(1)
            .expect("playback activity lease count underflowed");
        drop(activity);
        self.activity_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn try_admit_automatic(&self) -> Option<AutomaticRefreshAdmission<'_>> {
        let mut activity = self.activity.lock().expect("activity admission poisoned");
        if activity.playback_leases != 0 {
            return None;
        }
        activity.automatic_refreshes = activity
            .automatic_refreshes
            .checked_add(1)
            .expect("automatic refresh admission count overflowed");
        Some(AutomaticRefreshAdmission { runtime: self })
    }

    fn ready_configuration(&self, kind: SourceKind) -> Option<ConfigurationContext> {
        let state = self
            .configuration
            .read()
            .expect("Source Configuration state poisoned");
        let configuration = state.ready.then(|| state.configuration.clone()).flatten()?;
        (kind == SourceKind::M3u || configuration.has_epg()).then_some(ConfigurationContext {
            epoch: state.epoch,
            configuration,
        })
    }

    async fn await_ready_configuration(&self, kind: SourceKind) -> Option<ConfigurationContext> {
        loop {
            let mut changed = self.configuration_changed.subscribe();
            let snapshot = {
                let state = self
                    .configuration
                    .read()
                    .expect("Source Configuration state poisoned");
                (state.ready, state.epoch, state.configuration.clone())
            };
            if snapshot.0 {
                let configuration = snapshot.2?;
                return (kind == SourceKind::M3u || configuration.has_epg()).then_some(
                    ConfigurationContext {
                        epoch: snapshot.1,
                        configuration,
                    },
                );
            }
            if changed.changed().await.is_err() {
                return None;
            }
        }
    }

    fn context_is_current(&self, context: &ConfigurationContext) -> bool {
        let state = self
            .configuration
            .read()
            .expect("Source Configuration state poisoned");
        state.ready && state.epoch == context.epoch
    }

    fn start_automation(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.automation_started.swap(true, Ordering::AcqRel) {
            return;
        }
        if self.cache_on_start.swap(false, Ordering::AcqRel) {
            self.queue_catalog_cache(false);
        }
        let pending = self
            .deferred_epg_restore
            .lock()
            .expect("recovery state poisoned")
            .clone();
        if let Some(context) = pending {
            // M3U can refresh independently of the deferred EPG snapshot.
            self.spawn_refresh(RefreshTrigger::Startup);
            let weak = Arc::downgrade(self);
            let adapters = self.adapters.clone();
            // Run deferred EPG recovery on an async task (never holding a
            // blocking worker while awaiting `catalog_build`) so a bounded
            // blocking pool cannot deadlock against a concurrent M3U build.
            tokio::spawn(async move {
                let source =
                    SnapshotSource::epg(&context.configuration).expect("pending recovery has EPG");
                let recovered =
                    recover_source(&adapters, source, EPG_DECODED_LIMIT, xmltv::parse, true).await;
                if let Some(runtime) = weak.upgrade() {
                    runtime.restore_epg_snapshot(&context, recovered).await;
                    *runtime
                        .deferred_epg_restore
                        .lock()
                        .expect("recovery state poisoned") = None;
                    runtime.launch_automation();
                }
            });
        } else {
            self.launch_automation();
        }
    }

    fn launch_automation(self: &Arc<Self>) {
        self.spawn_scheduler(SourceKind::M3u);
        self.spawn_scheduler(SourceKind::Epg);
        if self.ready_configuration(SourceKind::M3u).is_some() {
            self.spawn_refresh(RefreshTrigger::Startup);
        }
    }

    async fn restore_epg_snapshot(
        self: &Arc<Self>,
        context: &ConfigurationContext,
        recovered: RecoveryAttempt<xmltv::ParsedGuide>,
    ) {
        let _build_guard = self.catalog_build.lock().await;
        let current = self.view.load_full();
        if !self.epoch_is_current_ready(context.epoch) || current.sources.epg.is_some() {
            return;
        }
        let epg_diagnostic = recovered.diagnostic;
        let epg_terminal_failure = recovered.terminal_failure;
        let (epg_candidate, epg_validated_at, mut epg_value) = match recovered.loaded {
            Some(loaded) => (
                Some(loaded.candidate),
                Some(loaded.validated_at),
                Some(loaded.value),
            ),
            None => (None, None, None),
        };

        let built = match (&current.sources.m3u, &current.catalog, &epg_candidate) {
            (Some(m3u), Some(existing_catalog), Some(candidate)) => {
                let m3u_checksum = *m3u.metadata().checksum();
                let generation = context
                    .configuration
                    .catalog_generation(&m3u_checksum, Some(candidate.metadata().checksum()));
                let existing_catalog = Arc::clone(existing_catalog);
                // Move the parsed guide into the build job so it drops on the
                // blocking worker instead of under the `publication` mutex.
                let epg_parsed = epg_value.take().expect("loaded EPG has parsed guide");
                let offload = should_offload_guide_update(
                    &existing_catalog,
                    candidate.metadata().decoded_bytes(),
                    &epg_parsed,
                );
                let catalog = run_blocking_if(offload, move || {
                    Arc::new(existing_catalog.with_updated_guide(epg_parsed.as_ref(), generation))
                })
                .await;
                Some((catalog, generation))
            }
            _ => None,
        };

        let (catalog_changed, needs_cache_write) = {
            let _publication = self.publication.lock().expect("publication lock poisoned");
            let current = self.view.load_full();
            // Configuration changes and manual refreshes win over delayed recovery.
            if !self.epoch_is_current_ready(context.epoch) || current.sources.epg.is_some() {
                return;
            }
            let mut sources = current.sources.clone();
            let mut status = current.status.clone();
            let state = match (epg_candidate, epg_validated_at) {
                (Some(candidate), Some(validated_at)) => {
                    let state = recovered_source_state(
                        validated_at,
                        self.adapters.clock().now(),
                        RecoveredFreshness::AgeBased,
                    );
                    sources.epg = Some(candidate);
                    if sources.m3u.is_none() {
                        sources.pending_epg = epg_value.take();
                    } else {
                        sources.pending_epg = None;
                    }
                    state
                }
                _ => SourceState::Unavailable {
                    failure: epg_terminal_failure,
                },
            };
            status.set_source_state(SourceKind::Epg, state);
            status.set_recovery(SourceKind::Epg, epg_diagnostic);
            let (catalog, generation) = match built {
                Some((catalog, generation)) if sources.epg.is_some() => {
                    (Some(catalog), Some(generation))
                }
                _ => (current.catalog.clone(), current.status.generation()),
            };
            status.set_generation(generation);
            let changed = generation != current.status.generation();
            let needs_cache_write = catalog.as_ref().is_some_and(|c| !c.is_mapped());
            self.view.store(Arc::new(CoreView {
                status: status.clone(),
                catalog,
                sources,
            }));
            let occurred_at = self.adapters.clock().now();
            let _ = self.events.send(CoreEvent::CatalogStatusChanged {
                occurred_at,
                status,
            });
            if changed && let Some(generation) = generation {
                let _ = self.events.send(CoreEvent::CatalogPublished {
                    occurred_at,
                    generation,
                });
            }
            (changed, needs_cache_write)
        };
        drop(_build_guard);
        if needs_cache_write {
            self.queue_catalog_cache(catalog_changed);
        }
    }

    fn queue_catalog_cache(self: &Arc<Self>, invalidate_in_flight: bool) {
        if !self.adapters.snapshot_store().catalog_cache_enabled() {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let spawn = {
            let mut state = self.cache_worker.lock().expect("cache queue poisoned");
            // Mark dirty BEFORE bumping revision so an in-flight worker that
            // cancels on `cache_revision` always observes `dirty == true` and
            // seamlessly persists the newer generation without respawning.
            state.dirty = true;
            if invalidate_in_flight {
                self.cache_revision.fetch_add(1, Ordering::AcqRel);
            }
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };
        if !spawn {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::task::spawn_blocking(move || {
            loop {
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                {
                    let mut state = runtime.cache_worker.lock().expect("cache queue poisoned");
                    if !state.dirty {
                        state.running = false;
                        break;
                    }
                    state.dirty = false;
                }
                // Snapshot the latest coherent view under `publication` so a
                // delayed caller cannot overwrite a newer publication's job.
                let job = {
                    let _publication = runtime
                        .publication
                        .lock()
                        .expect("publication lock poisoned");
                    let revision = runtime.cache_revision.load(Ordering::Acquire);
                    let context = runtime.ready_configuration(SourceKind::M3u);
                    let view = runtime.view.load_full();
                    context.and_then(|context| {
                        let catalog = view.catalog.as_ref().filter(|c| !c.is_mapped())?;
                        let m3u = view.sources.m3u.as_ref()?.clone();
                        let epg = view.sources.epg.clone();
                        let key = crate::catalog::cache_key(
                            &context.configuration,
                            m3u.metadata().checksum(),
                            epg.as_ref().map(|epg| epg.metadata().checksum()),
                        );
                        Some((context.epoch, Arc::clone(catalog), m3u, epg, key, revision))
                    })
                };
                let adapters = runtime.adapters.clone();
                let shutdown = runtime.shutdown.subscribe();
                let cache_revision = Arc::clone(&runtime.cache_revision);
                drop(runtime);

                let Some((epoch, submitted_catalog, m3u, epg, key, submitted_revision)) = job
                else {
                    continue;
                };
                let is_cancelled = || {
                    *shutdown.borrow()
                        || cache_revision.load(Ordering::Acquire) != submitted_revision
                };
                let mut candidates = Vec::with_capacity(2);
                candidates.push(&m3u);
                if let Some(epg) = epg.as_ref() {
                    candidates.push(epg);
                }
                let mapping = adapters.snapshot_store().write_mapped_catalog_cache(
                    &key,
                    &candidates,
                    submitted_catalog.as_bytes(),
                    &is_cancelled,
                );
                let Some(mapping) = mapping else {
                    continue;
                };
                if *shutdown.borrow() {
                    continue;
                }
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                let Some(mapped_catalog) = submitted_catalog.promote_to_mapping(mapping) else {
                    continue;
                };
                let _publication = runtime
                    .publication
                    .lock()
                    .expect("publication lock poisoned");
                if !runtime.epoch_is_current_ready(epoch) {
                    continue;
                }
                let current = runtime.view.load_full();
                if current
                    .catalog
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, &submitted_catalog))
                {
                    runtime.view.store(Arc::new(CoreView {
                        status: current.status.clone(),
                        catalog: Some(Arc::new(mapped_catalog)),
                        sources: current.sources.clone(),
                    }));
                }
            }
        });
    }

    fn spawn_refresh(self: &Arc<Self>, trigger: RefreshTrigger) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let runtime = Arc::clone(self);
        handle.spawn(async move {
            // Deferred recovery owns the first EPG refresh so its saved
            // freshness and validators are available. M3U work can proceed.
            let restoring_epg = runtime
                .deferred_epg_restore
                .lock()
                .expect("recovery state poisoned")
                .is_some();
            if restoring_epg {
                let _ = runtime.refresh_source(SourceKind::M3u, trigger).await;
            } else {
                let _ = runtime.refresh(trigger).await;
            }
        });
    }

    fn spawn_scheduler(self: &Arc<Self>, kind: SourceKind) {
        let weak = Arc::downgrade(self);
        let clock = self.adapters.clock_arc();
        let reschedule = Arc::clone(&self.control(kind).reschedule);
        let mut shutdown = self.shutdown.subscribe();
        let mut configuration_changed = self.configuration_changed.subscribe();
        tokio::spawn(async move {
            loop {
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                if runtime.ready_configuration(kind).is_none() {
                    drop(runtime);
                    tokio::select! {
                        result = configuration_changed.changed() => {
                            if result.is_err() {
                                break;
                            }
                        }
                        result = shutdown.changed() => {
                            if result.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                    continue;
                }
                let deadline = runtime.next_automatic_attempt(kind).0;
                drop(runtime);

                tokio::select! {
                    () = clock.wait_until(deadline) => {
                        let Some(runtime) = weak.upgrade() else {
                            break;
                        };
                        let _ = runtime
                            .refresh_source(kind, RefreshTrigger::FreshnessDeadline)
                            .await;
                    }
                    () = reschedule.notified() => {}
                    result = configuration_changed.changed() => {
                        if result.is_err() {
                            break;
                        }
                    }
                    result = shutdown.changed() => {
                        if result.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        });
    }

    async fn refresh(self: &Arc<Self>, trigger: RefreshTrigger) -> RefreshReport {
        let Some(context) = self.await_ready_configuration(SourceKind::M3u).await else {
            return RefreshReport::new(
                trigger,
                RefreshOutcome::NotConfigured,
                None,
                self.view.load().status.clone(),
            );
        };

        self.refresh_in(trigger, context).await
    }

    async fn refresh_in(
        self: &Arc<Self>,
        trigger: RefreshTrigger,
        context: ConfigurationContext,
    ) -> RefreshReport {
        let m3u = self.refresh_source_in(SourceKind::M3u, trigger, context.clone());
        let (m3u, epg) = if context.configuration.has_epg() {
            let epg = self.refresh_source_in(SourceKind::Epg, trigger, context);
            let (m3u, epg) = futures_util::future::join(m3u, epg).await;
            (m3u, Some(epg))
        } else {
            (m3u.await, None)
        };
        RefreshReport::new(trigger, m3u, epg, self.view.load().status.clone())
    }

    async fn refresh_source(
        self: &Arc<Self>,
        kind: SourceKind,
        trigger: RefreshTrigger,
    ) -> RefreshOutcome {
        let Some(context) = self.await_ready_configuration(kind).await else {
            return RefreshOutcome::NotConfigured;
        };
        self.refresh_source_in(kind, trigger, context).await
    }

    async fn refresh_source_in(
        self: &Arc<Self>,
        kind: SourceKind,
        trigger: RefreshTrigger,
        context: ConfigurationContext,
    ) -> RefreshOutcome {
        if !self.context_is_current(&context) {
            return RefreshOutcome::NotConfigured;
        }

        let control = self.control(kind);
        let (flight, leader) = loop {
            let stale = {
                let _admission = self
                    .configuration_admission
                    .lock()
                    .expect("Source Configuration admission poisoned");
                if !self.context_is_current(&context) {
                    return RefreshOutcome::NotConfigured;
                }
                let mut current = control.flight.lock().expect("refresh flight poisoned");
                if let Some(flight) = current.as_ref() {
                    let flight = Arc::clone(flight);
                    if flight.context.epoch != context.epoch
                        || (trigger.is_manual() && !flight.try_promote())
                    {
                        Some(flight)
                    } else {
                        break (flight, false);
                    }
                } else {
                    let flight = Arc::new(RefreshFlight::new(context.clone(), trigger.is_manual()));
                    *current = Some(Arc::clone(&flight));
                    break (flight, true);
                }
            };
            stale
                .expect("a stale or committed-skipped flight is retried")
                .wait_until_finished()
                .await;
            if !self.context_is_current(&context) {
                return RefreshOutcome::NotConfigured;
            }
        };

        if leader {
            let runtime = Arc::clone(self);
            let task_flight = Arc::clone(&flight);
            tokio::spawn(async move {
                let result = AssertUnwindSafe(runtime.run_source_refresh(kind, &task_flight))
                    .catch_unwind()
                    .await;
                let control = runtime.control(kind);
                let mut current = control.flight.lock().expect("refresh flight poisoned");
                if let Ok(outcome) = &result {
                    runtime.publish_refresh_completed(
                        &current,
                        task_flight.context.epoch,
                        kind,
                        outcome,
                    );
                }
                if current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &task_flight))
                {
                    *current = None;
                }
                drop(current);
                control.reschedule.notify_one();
                match result {
                    Ok(outcome) => task_flight.complete(outcome),
                    Err(payload) => {
                        task_flight.fail_panicked();
                        resume_unwind(payload);
                    }
                }
            });
        }

        flight.wait().await
    }

    async fn run_source_refresh(
        self: &Arc<Self>,
        kind: SourceKind,
        flight: &RefreshFlight,
    ) -> RefreshOutcome {
        if !self.context_is_current(&flight.context) {
            return RefreshOutcome::NotConfigured;
        }
        let mut automatic_admission = None;
        if !flight.manual.load(Ordering::Acquire) {
            let (next_attempt_at, reason) = self.next_automatic_attempt(kind);
            if self.adapters.clock().now() < next_attempt_at {
                let skipped = RefreshOutcome::Skipped {
                    reason,
                    next_attempt_at,
                };
                if flight.try_commit_skip() {
                    return skipped;
                }
            }

            let mut reported_deferred = false;
            loop {
                let mut activity_changed = self.activity_changed.subscribe();
                let mut promoted = flight.promoted.subscribe();
                let mut configuration_changed = self.configuration_changed.subscribe();
                if !self.context_is_current(&flight.context) {
                    return RefreshOutcome::NotConfigured;
                }
                if flight.manual.load(Ordering::Acquire) {
                    break;
                }
                if let Some(admission) = self.try_admit_automatic() {
                    automatic_admission = Some(admission);
                    break;
                }
                if !reported_deferred {
                    if !self.set_source_state(
                        flight.context.epoch,
                        kind,
                        SourceState::Deferred {
                            validated_at: self.current_validated_at(kind),
                            deferred_at: self.adapters.clock().now(),
                        },
                    ) {
                        return RefreshOutcome::NotConfigured;
                    }
                    reported_deferred = true;
                }
                tokio::select! {
                    result = activity_changed.changed() => {
                        result.expect("activity admission exists for the runtime");
                    }
                    result = promoted.changed() => {
                        result.expect("refresh flight exists while it is pending");
                    }
                    result = configuration_changed.changed() => {
                        if result.is_err() || !self.context_is_current(&flight.context) {
                            return RefreshOutcome::NotConfigured;
                        }
                    }
                }
            }
        }
        flight.admit();

        if !self.set_source_state(
            flight.context.epoch,
            kind,
            SourceState::Refreshing {
                validated_at: self.current_validated_at(kind),
                started_at: self.adapters.clock().now(),
            },
        ) {
            return RefreshOutcome::NotConfigured;
        }

        let result = match kind {
            SourceKind::M3u => self.refresh_m3u(&flight.context).await,
            SourceKind::Epg => self.refresh_epg(&flight.context).await,
        };
        drop(automatic_admission);
        match result {
            Ok(outcome) => {
                if self.reset_policy(flight.context.epoch, kind) {
                    outcome
                } else {
                    RefreshOutcome::NotConfigured
                }
            }
            Err(failure) => {
                if let Some(next_attempt_at) =
                    self.record_failure(flight.context.epoch, kind, &failure)
                {
                    RefreshOutcome::Failed {
                        failure,
                        next_attempt_at,
                    }
                } else {
                    RefreshOutcome::NotConfigured
                }
            }
        }
    }

    async fn refresh_m3u(
        self: &Arc<Self>,
        context: &ConfigurationContext,
    ) -> Result<RefreshOutcome, SafeFailure> {
        let configuration = context.configuration.as_ref();
        let current = self.view.load().sources.m3u.clone();
        let validators = current
            .as_ref()
            .map(|candidate| candidate.metadata().validators().clone())
            .unwrap_or_default();
        let protected = current;
        let fetched = fetch_source(
            &self.adapters,
            SourceRequest::m3u(configuration, validators),
            SnapshotSource::m3u(configuration),
            protected,
            M3U_DECODED_LIMIT,
            m3u::parse,
        )
        .await?;
        match fetched {
            FetchedSource::Modified(loaded) => {
                let validated_at = loaded.validated_at;
                Ok(if self.publish_m3u(context, loaded).await {
                    RefreshOutcome::Updated { validated_at }
                } else {
                    RefreshOutcome::NotConfigured
                })
            }
            FetchedSource::NotModified {
                candidate,
                validated_at,
            } => Ok(
                if self.publish_revalidation(
                    context.epoch,
                    SourceKind::M3u,
                    candidate,
                    validated_at,
                ) {
                    RefreshOutcome::NotModified { validated_at }
                } else {
                    RefreshOutcome::NotConfigured
                },
            ),
        }
    }

    async fn refresh_epg(
        self: &Arc<Self>,
        context: &ConfigurationContext,
    ) -> Result<RefreshOutcome, SafeFailure> {
        let configuration = context.configuration.as_ref();
        let current = self.view.load().sources.epg.clone();
        let validators = current
            .as_ref()
            .map(|candidate| candidate.metadata().validators().clone())
            .unwrap_or_default();
        let protected = current;
        let request = SourceRequest::epg(configuration, validators)
            .expect("an EPG refresh only runs when EPG is configured");
        let snapshot = SnapshotSource::epg(configuration)
            .expect("an EPG refresh only runs when EPG is configured");
        let fetched = fetch_source(
            &self.adapters,
            request,
            snapshot,
            protected,
            EPG_DECODED_LIMIT,
            xmltv::parse,
        )
        .await?;
        match fetched {
            FetchedSource::Modified(loaded) => {
                let validated_at = loaded.validated_at;
                Ok(if self.publish_epg(context, loaded).await {
                    RefreshOutcome::Updated { validated_at }
                } else {
                    RefreshOutcome::NotConfigured
                })
            }
            FetchedSource::NotModified {
                candidate,
                validated_at,
            } => Ok(
                if self.publish_revalidation(
                    context.epoch,
                    SourceKind::Epg,
                    candidate,
                    validated_at,
                ) {
                    RefreshOutcome::NotModified { validated_at }
                } else {
                    RefreshOutcome::NotConfigured
                },
            ),
        }
    }

    async fn publish_m3u(
        self: &Arc<Self>,
        context: &ConfigurationContext,
        loaded: LoadedSource<Vec<m3u::ParsedChannel>>,
    ) -> bool {
        let _build_guard = self.catalog_build.lock().await;
        if !self.epoch_is_current_ready(context.epoch) {
            return false;
        }
        let current = self.view.load_full();
        let content_changed = current.sources.m3u.as_ref().is_none_or(|candidate| {
            candidate.metadata().checksum() != loaded.candidate.metadata().checksum()
        });
        let current_epg_checksum = current
            .sources
            .epg
            .as_ref()
            .map(|candidate| *candidate.metadata().checksum());
        let mut m3u_value = Some(loaded.value);
        let built = if content_changed {
            let configuration = Arc::clone(&context.configuration);
            let m3u_parsed = m3u_value.take().expect("loaded M3U has parsed channels");
            let existing_catalog = current.catalog.clone();
            let pending_epg = current.sources.pending_epg.clone();
            let generation = configuration.catalog_generation(
                loaded.candidate.metadata().checksum(),
                current_epg_checksum.as_ref(),
            );
            let offload = should_offload_catalog_build(
                &m3u_parsed,
                loaded.candidate.metadata().decoded_bytes(),
                pending_epg.as_deref(),
                current
                    .sources
                    .epg
                    .as_ref()
                    .map_or(0, |candidate| candidate.metadata().decoded_bytes()),
                existing_catalog.as_ref().is_some_and(|c| c.has_epg()),
            );
            let catalog = run_blocking_if(offload, move || {
                let built = if let Some(existing) = existing_catalog.as_deref() {
                    ChannelCatalog::from_parsed_with_existing_guide(
                        configuration.as_ref(),
                        &m3u_parsed,
                        existing,
                        generation,
                    )
                } else {
                    ChannelCatalog::from_parsed(
                        configuration.as_ref(),
                        &m3u_parsed,
                        pending_epg.as_deref(),
                        generation,
                    )
                };
                Arc::new(built)
            })
            .await;
            Some((catalog, generation))
        } else {
            None
        };
        drop(m3u_value);

        let (invalidate_in_flight, needs_cache_write) = {
            let _publication = self.publication.lock().expect("publication lock poisoned");
            if !self.epoch_is_current_ready(context.epoch) {
                return false;
            }
            let current = self.view.load_full();
            let mut sources = current.sources.clone();
            sources.m3u = Some(loaded.candidate);
            sources.pending_epg = None;
            let (catalog, generation) = match built {
                Some((catalog, generation)) => (Some(catalog), Some(generation)),
                None => (current.catalog.clone(), current.status.generation()),
            };
            let needs_cache_write = catalog.as_ref().is_some_and(|c| !c.is_mapped());
            self.commit_published_sources(
                current.as_ref(),
                sources,
                SourceKind::M3u,
                loaded.validated_at,
                content_changed,
                catalog,
                generation,
            );
            (content_changed, needs_cache_write)
        };
        drop(_build_guard);
        if needs_cache_write {
            self.queue_catalog_cache(invalidate_in_flight);
        }
        true
    }

    async fn publish_epg(
        self: &Arc<Self>,
        context: &ConfigurationContext,
        loaded: LoadedSource<xmltv::ParsedGuide>,
    ) -> bool {
        let _build_guard = self.catalog_build.lock().await;
        if !self.epoch_is_current_ready(context.epoch) {
            return false;
        }
        let current = self.view.load_full();
        let content_changed = current.sources.epg.as_ref().is_none_or(|candidate| {
            candidate.metadata().checksum() != loaded.candidate.metadata().checksum()
        });
        let mut epg_value = Some(loaded.value);
        let built = if content_changed
            && let (Some(m3u), Some(existing_catalog)) =
                (current.sources.m3u.as_ref(), current.catalog.as_ref())
        {
            let m3u_checksum = *m3u.metadata().checksum();
            let existing_catalog = Arc::clone(existing_catalog);
            let epg_parsed = epg_value.take().expect("loaded EPG has parsed guide");
            let generation = context
                .configuration
                .catalog_generation(&m3u_checksum, Some(loaded.candidate.metadata().checksum()));
            let offload = should_offload_guide_update(
                &existing_catalog,
                loaded.candidate.metadata().decoded_bytes(),
                &epg_parsed,
            );
            let catalog = run_blocking_if(offload, move || {
                Arc::new(existing_catalog.with_updated_guide(epg_parsed.as_ref(), generation))
            })
            .await;
            Some((catalog, generation))
        } else {
            None
        };

        let (invalidate_in_flight, needs_cache_write) = {
            let _publication = self.publication.lock().expect("publication lock poisoned");
            if !self.epoch_is_current_ready(context.epoch) {
                return false;
            }
            let current = self.view.load_full();
            let mut sources = current.sources.clone();
            sources.epg = Some(loaded.candidate);
            let (catalog, generation, catalog_changed) = if sources.m3u.is_none() {
                if content_changed {
                    sources.pending_epg = epg_value.take();
                }
                (None, None, false)
            } else if let Some((catalog, generation)) = built {
                sources.pending_epg = None;
                (Some(catalog), Some(generation), true)
            } else {
                (current.catalog.clone(), current.status.generation(), false)
            };
            let needs_cache_write = catalog.as_ref().is_some_and(|c| !c.is_mapped());
            self.commit_published_sources(
                current.as_ref(),
                sources,
                SourceKind::Epg,
                loaded.validated_at,
                content_changed,
                catalog,
                generation,
            );
            (catalog_changed, needs_cache_write)
        };
        drop(_build_guard);
        if needs_cache_write {
            self.queue_catalog_cache(invalidate_in_flight);
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_published_sources(
        &self,
        current: &CoreView,
        sources: PublishedSources,
        refreshed: SourceKind,
        validated_at: chrono::DateTime<chrono::Utc>,
        content_changed: bool,
        catalog: Option<Arc<ChannelCatalog>>,
        generation: Option<crate::domain::CatalogGeneration>,
    ) {
        let mut status = current.status.clone();
        status.set_source_state(
            refreshed,
            source_state(validated_at, self.adapters.clock().now()),
        );
        status.set_generation(generation);
        self.view.store(Arc::new(CoreView {
            status: status.clone(),
            catalog,
            sources,
        }));
        let occurred_at = self.adapters.clock().now();
        let _ = self.events.send(CoreEvent::CatalogStatusChanged {
            occurred_at,
            status,
        });
        if content_changed && let Some(generation) = generation {
            let _ = self.events.send(CoreEvent::CatalogPublished {
                occurred_at,
                generation,
            });
        }
    }

    fn publish_revalidation(
        self: &Arc<Self>,
        epoch: u64,
        kind: SourceKind,
        candidate: SnapshotCandidate,
        validated_at: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let needs_cache_write = {
            let _publication = self.publication.lock().expect("publication lock poisoned");
            if !self.epoch_is_current_ready(epoch) {
                return false;
            }
            let current = self.view.load_full();
            let mut sources = current.sources.clone();
            match kind {
                SourceKind::M3u => {
                    debug_assert!(sources.m3u.is_some());
                    sources.m3u = Some(candidate);
                }
                SourceKind::Epg => {
                    debug_assert!(sources.epg.is_some());
                    sources.epg = Some(candidate);
                }
            }
            let mut status = current.status.clone();
            status.set_source_state(
                kind,
                source_state(validated_at, self.adapters.clock().now()),
            );
            let needs_cache_write = current.catalog.as_ref().is_some_and(|c| !c.is_mapped());
            self.view.store(Arc::new(CoreView {
                status: status.clone(),
                catalog: current.catalog.clone(),
                sources,
            }));
            let _ = self.events.send(CoreEvent::CatalogStatusChanged {
                occurred_at: self.adapters.clock().now(),
                status,
            });
            needs_cache_write
        };
        if needs_cache_write {
            self.queue_catalog_cache(false);
        }
        true
    }

    fn set_source_state(&self, epoch: u64, kind: SourceKind, state: SourceState) -> bool {
        let _publication = self.publication.lock().expect("publication lock poisoned");
        if !self.epoch_is_current_ready(epoch) {
            return false;
        }
        let current = self.view.load_full();
        let mut status = current.status.clone();
        status.set_source_state(kind, state);
        self.view.store(Arc::new(CoreView {
            status: status.clone(),
            catalog: current.catalog.clone(),
            sources: current.sources.clone(),
        }));
        let _ = self.events.send(CoreEvent::CatalogStatusChanged {
            occurred_at: self.adapters.clock().now(),
            status,
        });
        true
    }

    /// Publishes while the caller still excludes the completed source flight.
    /// This lock also linearizes the event against subscription snapshots and
    /// lag resynchronization.
    fn publish_refresh_completed(
        &self,
        _flight: &std::sync::MutexGuard<'_, Option<Arc<RefreshFlight>>>,
        epoch: u64,
        kind: SourceKind,
        outcome: &RefreshOutcome,
    ) {
        let _publication = self.publication.lock().expect("publication lock poisoned");
        if !self.epoch_is_current_ready(epoch) {
            return;
        }
        let _ = self.events.send(CoreEvent::RefreshCompleted {
            occurred_at: self.adapters.clock().now(),
            kind,
            outcome: outcome.clone(),
        });
    }

    fn current_validated_at(&self, kind: SourceKind) -> Option<chrono::DateTime<chrono::Utc>> {
        let view = self.view.load();
        match kind {
            SourceKind::M3u => view
                .sources
                .m3u
                .as_ref()
                .map(|candidate| candidate.metadata().validated_at()),
            SourceKind::Epg => view
                .sources
                .epg
                .as_ref()
                .map(|candidate| candidate.metadata().validated_at()),
        }
    }

    fn next_automatic_attempt(
        &self,
        kind: SourceKind,
    ) -> (
        chrono::DateTime<chrono::Utc>,
        crate::domain::RefreshSkipReason,
    ) {
        let now = self.adapters.clock().now();
        let freshness_at = self
            .current_stale_attempt_at(kind)
            .or_else(|| {
                self.current_validated_at(kind)
                    .filter(|validated_at| *validated_at <= now)
                    .and_then(|validated_at| validated_at.checked_add_signed(FRESHNESS))
            })
            .unwrap_or(now);
        let retry_at = self
            .control(kind)
            .policy
            .lock()
            .expect("refresh policy poisoned")
            .next_attempt_at;
        retry_at.map_or(
            (freshness_at, crate::domain::RefreshSkipReason::Fresh),
            |retry_at| (retry_at, crate::domain::RefreshSkipReason::Backoff),
        )
    }

    fn current_stale_attempt_at(&self, kind: SourceKind) -> Option<chrono::DateTime<chrono::Utc>> {
        let view = self.view.load();
        let state = match kind {
            SourceKind::M3u => Some(view.status.m3u()),
            SourceKind::Epg => view.status.epg(),
        };
        match state {
            Some(SourceState::Stale {
                next_attempt_at: Some(next_attempt_at),
                ..
            }) => Some(*next_attempt_at),
            _ => None,
        }
    }

    fn record_failure(
        &self,
        epoch: u64,
        kind: SourceKind,
        failure: &SafeFailure,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let _publication = self.publication.lock().expect("publication lock poisoned");
        if !self.epoch_is_current_ready(epoch) {
            return None;
        }
        let control = self.control(kind);
        let mut policy = control.policy.lock().expect("refresh policy poisoned");
        let index = policy
            .consecutive_failures
            .min(BACKOFF_MINUTES.len().saturating_sub(1));
        let base = std::time::Duration::from_secs(BACKOFF_MINUTES[index] as u64 * 60);
        policy.consecutive_failures = policy.consecutive_failures.saturating_add(1);
        let retry_after = match failure {
            SafeFailure::SourceAccess { retry_after, .. } => *retry_after,
            _ => None,
        };
        let delay = retry_after.map_or(base, |delay| delay.max(base));
        let now = self.adapters.clock().now();
        let next_attempt_at = chrono::Duration::from_std(delay)
            .ok()
            .and_then(|delay| now.checked_add_signed(delay))
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
        policy.next_attempt_at = Some(next_attempt_at);
        drop(policy);
        let current = self.view.load_full();
        let mut status = current.status.clone();
        status.set_source_state(
            kind,
            SourceState::Failed {
                validated_at: match kind {
                    SourceKind::M3u => current
                        .sources
                        .m3u
                        .as_ref()
                        .map(|candidate| candidate.metadata().validated_at()),
                    SourceKind::Epg => current
                        .sources
                        .epg
                        .as_ref()
                        .map(|candidate| candidate.metadata().validated_at()),
                },
                failure: failure.clone(),
                next_attempt_at,
            },
        );
        self.view.store(Arc::new(CoreView {
            status: status.clone(),
            catalog: current.catalog.clone(),
            sources: current.sources.clone(),
        }));
        let _ = self.events.send(CoreEvent::CatalogStatusChanged {
            occurred_at: now,
            status,
        });
        control.reschedule.notify_one();
        Some(next_attempt_at)
    }

    fn reset_policy(&self, epoch: u64, kind: SourceKind) -> bool {
        let _publication = self.publication.lock().expect("publication lock poisoned");
        if !self.epoch_is_current_ready(epoch) {
            return false;
        }
        let control = self.control(kind);
        let mut policy = control.policy.lock().expect("refresh policy poisoned");
        policy.consecutive_failures = 0;
        policy.next_attempt_at = None;
        drop(policy);
        control.reschedule.notify_one();
        true
    }

    fn epoch_is_current_ready(&self, epoch: u64) -> bool {
        let state = self
            .configuration
            .read()
            .expect("Source Configuration state poisoned");
        state.ready && state.epoch == epoch
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

struct RecoveredConfiguration {
    view: CoreView,
}

#[derive(Clone, Copy)]
enum RecoveredFreshness {
    AgeBased,
    PendingRevalidation,
}

async fn recover_catalog_cache(
    configuration: &SourceConfiguration,
    adapters: &CoreAdapters,
    freshness: RecoveredFreshness,
) -> Option<CoreView> {
    let store = adapters.snapshot_store();
    if !store.catalog_cache_enabled() {
        return None;
    }
    let m3u_scan = store
        .scan_candidates(SnapshotSource::m3u(configuration))
        .await
        .ok()?;
    if !m3u_scan.diagnostics().is_empty() {
        return None;
    }
    let m3u = m3u_scan
        .into_candidates()
        .next()
        .filter(|c| !c.requires_adoption())?;
    if m3u.metadata().source() != SnapshotSource::m3u(configuration) {
        return None;
    }
    let epg = match SnapshotSource::epg(configuration) {
        Some(source) => {
            let scan = store.scan_candidates(source).await.ok()?;
            if !scan.diagnostics().is_empty() {
                return None;
            }
            let candidate = scan.into_candidates().next();
            if candidate
                .as_ref()
                .is_some_and(|c| c.metadata().source() != source)
            {
                return None;
            }
            if candidate
                .as_ref()
                .is_some_and(SnapshotCandidate::requires_adoption)
            {
                return None;
            }
            candidate
        }
        None => None,
    };
    let generation = configuration.catalog_generation(
        m3u.metadata().checksum(),
        epg.as_ref().map(|e| e.metadata().checksum()),
    );
    let key = crate::catalog::cache_key(
        configuration,
        m3u.metadata().checksum(),
        epg.as_ref().map(|e| e.metadata().checksum()),
    );
    // The derived copy is independently checksummed and bound to the
    // participating raw candidates' payload stat identity (re-hashing raw
    // payloads if ambiguous); any mismatch falls back to raw recovery.
    let store_arc = adapters.snapshot_store_arc();
    let m3u_for_cache = m3u.clone();
    let epg_for_cache = epg.clone();
    let catalog = run_blocking(move || {
        let mut candidates = Vec::with_capacity(2);
        candidates.push(&m3u_for_cache);
        if let Some(epg) = epg_for_cache.as_ref() {
            candidates.push(epg);
        }
        ChannelCatalog::open_from_cache(store_arc.as_ref(), &key, &candidates, generation)
    })
    .await?;
    if catalog.has_epg() != epg.is_some() {
        return None;
    }
    let now = adapters.clock().now();
    let m3u_state = recovered_source_state(m3u.metadata().validated_at(), now, freshness);
    let epg_state = configuration.has_epg().then(|| {
        epg.as_ref()
            .map_or(SourceState::Unavailable { failure: None }, |e| {
                recovered_source_state(e.metadata().validated_at(), now, freshness)
            })
    });
    let sources = PublishedSources {
        m3u: Some(m3u),
        epg,
        pending_epg: None,
    };
    Some(CoreView {
        status: CatalogStatus::published(
            generation,
            configuration.redacted(),
            m3u_state,
            epg_state,
            None,
            None,
        ),
        catalog: Some(Arc::new(catalog)),
        sources,
    })
}

async fn recover_configuration(
    configuration: Arc<SourceConfiguration>,
    adapters: &CoreAdapters,
    freshness: RecoveredFreshness,
) -> RecoveredConfiguration {
    recover_configuration_parts(configuration, adapters, freshness, true).await
}

async fn recover_configuration_parts(
    configuration: Arc<SourceConfiguration>,
    adapters: &CoreAdapters,
    freshness: RecoveredFreshness,
    include_epg: bool,
) -> RecoveredConfiguration {
    let m3u_recovery = recover_source(
        adapters,
        SnapshotSource::m3u(configuration.as_ref()),
        M3U_DECODED_LIMIT,
        m3u::parse,
        false,
    );
    let epg_recovery = async {
        match SnapshotSource::epg(configuration.as_ref()).filter(|_| include_epg) {
            Some(source) => {
                recover_source(adapters, source, EPG_DECODED_LIMIT, xmltv::parse, false).await
            }
            None => RecoveryAttempt {
                loaded: None,
                diagnostic: None,
                terminal_failure: None,
            },
        }
    };
    let (m3u_recovery, epg_recovery) = futures_util::future::join(m3u_recovery, epg_recovery).await;
    let RecoveryAttempt {
        loaded: m3u,
        diagnostic: m3u_diagnostic,
        terminal_failure: m3u_failure,
    } = m3u_recovery;
    let RecoveryAttempt {
        loaded: epg,
        diagnostic: epg_diagnostic,
        terminal_failure: epg_failure,
    } = epg_recovery;

    let generation = m3u.as_ref().map(|m3u| {
        configuration.catalog_generation(&m3u.checksum, epg.as_ref().map(|epg| &epg.checksum))
    });
    let catalog = match generation {
        Some(generation) => {
            let configuration = Arc::clone(&configuration);
            let m3u_value = Arc::clone(
                &m3u.as_ref()
                    .expect("a recovered generation has an M3U contribution")
                    .value,
            );
            let epg_value = epg.as_ref().map(|epg| Arc::clone(&epg.value));
            let offload = should_offload_catalog_build(
                &m3u_value,
                m3u.as_ref()
                    .map_or(0, |m3u| m3u.candidate.metadata().decoded_bytes()),
                epg_value.as_deref(),
                epg.as_ref()
                    .map_or(0, |epg| epg.candidate.metadata().decoded_bytes()),
                false,
            );
            let catalog = run_blocking_if(offload, move || {
                Arc::new(ChannelCatalog::from_parsed(
                    configuration.as_ref(),
                    &m3u_value,
                    epg_value.as_deref(),
                    generation,
                ))
            })
            .await;
            Some(catalog)
        }
        None => None,
    };
    let m3u_state = m3u
        .as_ref()
        .map(|m3u| recovered_source_state(m3u.validated_at, adapters.clock().now(), freshness));
    let epg_state = configuration.has_epg().then(|| {
        epg.as_ref().map_or_else(
            || SourceState::Unavailable {
                failure: epg_failure.clone(),
            },
            |epg| recovered_source_state(epg.validated_at, adapters.clock().now(), freshness),
        )
    });
    let mut status = match (generation, m3u_state) {
        (Some(generation), Some(m3u)) => CatalogStatus::published(
            generation,
            configuration.redacted(),
            m3u,
            epg_state,
            m3u_diagnostic.clone(),
            epg_diagnostic.clone(),
        ),
        _ => {
            let mut status = CatalogStatus::unavailable(configuration.redacted(), m3u_failure);
            if let Some(epg) = epg_state {
                status.set_source_state(SourceKind::Epg, epg);
            }
            status
        }
    };
    status.set_recovery(SourceKind::M3u, m3u_diagnostic);
    status.set_recovery(SourceKind::Epg, epg_diagnostic);
    let pending_epg = if catalog.is_none() {
        epg.as_ref().map(|loaded| Arc::clone(&loaded.value))
    } else {
        None
    };
    let sources = PublishedSources {
        m3u: m3u.map(|m3u| m3u.candidate),
        epg: epg.map(|epg| epg.candidate),
        pending_epg,
    };
    RecoveredConfiguration {
        view: CoreView {
            status,
            catalog,
            sources,
        },
    }
}

async fn load_catalog(
    configuration: &SourceConfiguration,
    adapters: &CoreAdapters,
) -> Result<LoadedCatalog, CatalogLoadFailure> {
    let m3u_snapshot = SnapshotSource::m3u(configuration);
    let m3u_recovery =
        recover_source(adapters, m3u_snapshot, M3U_DECODED_LIMIT, m3u::parse, false).await;
    let recovered_m3u = m3u_recovery.loaded.is_some();
    let m3u_diagnostic = m3u_recovery.diagnostic;
    let m3u = match m3u_recovery.loaded {
        Some(loaded) => loaded,
        None => load_source(
            adapters,
            SourceRequest::m3u(configuration, PrivateSourceValidators::default()),
            m3u_snapshot,
            M3U_DECODED_LIMIT,
            m3u::parse,
        )
        .await
        .map_err(|failure| CatalogLoadFailure {
            failure,
            m3u_recovery: m3u_diagnostic.clone(),
            epg_recovery: None,
        })?,
    };

    let (guide, epg_checksum, epg, epg_recovery, epg_candidate, epg_bootstrap_failure) = match (
        SourceRequest::epg(configuration, PrivateSourceValidators::default()),
        SnapshotSource::epg(configuration),
    ) {
        (Some(request), Some(snapshot)) => {
            let recovery =
                recover_source(adapters, snapshot, EPG_DECODED_LIMIT, xmltv::parse, false).await;
            let recovery_diagnostic = recovery.diagnostic;
            match recovery.loaded {
                Some(loaded) => {
                    let state = source_state(loaded.validated_at, adapters.clock().now());
                    (
                        Some(loaded.value),
                        Some(loaded.checksum),
                        Some(state),
                        recovery_diagnostic,
                        Some(loaded.candidate),
                        None,
                    )
                }
                None if recovered_m3u => (
                    None,
                    None,
                    Some(SourceState::Unavailable {
                        failure: recovery.terminal_failure,
                    }),
                    recovery_diagnostic,
                    None,
                    None,
                ),
                None => {
                    match load_source(adapters, request, snapshot, EPG_DECODED_LIMIT, xmltv::parse)
                        .await
                    {
                        Ok(loaded) => (
                            Some(loaded.value),
                            Some(loaded.checksum),
                            Some(source_state(loaded.validated_at, adapters.clock().now())),
                            recovery_diagnostic,
                            Some(loaded.candidate),
                            None,
                        ),
                        Err(failure) => {
                            let bootstrap_failure = failure.clone();
                            (
                                None,
                                None,
                                Some(SourceState::Unavailable {
                                    failure: Some(failure),
                                }),
                                recovery_diagnostic,
                                None,
                                Some(bootstrap_failure),
                            )
                        }
                    }
                }
            }
        }
        (None, None) => (None, None, None, None, None, None),
        _ => unreachable!("EPG request and snapshot identity are derived together"),
    };

    let generation = configuration.catalog_generation(&m3u.checksum, epg_checksum.as_ref());
    let catalog =
        ChannelCatalog::from_parsed(configuration, &m3u.value, guide.as_deref(), generation);
    let sources = PublishedSources {
        m3u: Some(m3u.candidate),
        epg: epg_candidate,
        pending_epg: None,
    };
    Ok(LoadedCatalog {
        catalog,
        m3u: source_state(m3u.validated_at, adapters.clock().now()),
        epg,
        m3u_recovery: m3u_diagnostic,
        epg_recovery,
        sources,
        bootstrap_failures: BootstrapFailures {
            m3u: None,
            epg: epg_bootstrap_failure,
        },
    })
}

struct LoadedCatalog {
    catalog: ChannelCatalog,
    m3u: SourceState,
    epg: Option<SourceState>,
    m3u_recovery: Option<SnapshotRecoveryDiagnostic>,
    epg_recovery: Option<SnapshotRecoveryDiagnostic>,
    sources: PublishedSources,
    bootstrap_failures: BootstrapFailures,
}

struct CatalogLoadFailure {
    failure: SafeFailure,
    m3u_recovery: Option<SnapshotRecoveryDiagnostic>,
    epg_recovery: Option<SnapshotRecoveryDiagnostic>,
}

struct LoadedSource<T> {
    value: Arc<T>,
    checksum: [u8; 32],
    validated_at: chrono::DateTime<chrono::Utc>,
    candidate: SnapshotCandidate,
}

type SourceParser<T> = fn(&mut dyn BufRead) -> Result<T, SafeFailure>;

struct RecoveryAttempt<T> {
    loaded: Option<LoadedSource<T>>,
    diagnostic: Option<SnapshotRecoveryDiagnostic>,
    terminal_failure: Option<SafeFailure>,
}

async fn recover_source<T: Send + 'static>(
    adapters: &CoreAdapters,
    source: SnapshotSource,
    decoded_limit: u64,
    parse: SourceParser<T>,
    force_blocking: bool,
) -> RecoveryAttempt<T> {
    let store = adapters.snapshot_store();
    let scan = match store.scan_candidates(source).await {
        Ok(scan) => scan,
        Err(reason) => {
            let failure = SafeFailure::Snapshot {
                kind: source.kind(),
                operation: SnapshotOperation::ScanCandidates,
                reason,
            };
            return RecoveryAttempt {
                loaded: None,
                diagnostic: SnapshotRecoveryDiagnostic::new(vec![failure.clone()], false),
                terminal_failure: Some(failure),
            };
        }
    };
    let mut failures = scan
        .diagnostics()
        .iter()
        .copied()
        .map(|reason| SafeFailure::SnapshotRecovery {
            kind: source.kind(),
            reason,
        })
        .collect::<Vec<_>>();

    for candidate in scan.into_candidates() {
        match recover_candidate(
            store,
            source,
            &candidate,
            decoded_limit,
            parse,
            force_blocking,
        )
        .await
        {
            Ok(mut loaded) => {
                let fallback_adopted = if candidate.requires_adoption() {
                    match store.adopt_candidate(&candidate).await {
                        Ok(adopted) if valid_adoption(&candidate, &adopted) => {
                            loaded.candidate = adopted;
                            true
                        }
                        Ok(_) => {
                            failures.push(SafeFailure::Snapshot {
                                kind: source.kind(),
                                operation: SnapshotOperation::AdoptCandidate,
                                reason: crate::domain::StoreError::Corrupt,
                            });
                            false
                        }
                        Err(reason) => {
                            failures.push(SafeFailure::Snapshot {
                                kind: source.kind(),
                                operation: SnapshotOperation::AdoptCandidate,
                                reason,
                            });
                            false
                        }
                    }
                } else {
                    false
                };
                return RecoveryAttempt {
                    loaded: Some(loaded),
                    diagnostic: SnapshotRecoveryDiagnostic::new(failures, fallback_adopted),
                    terminal_failure: None,
                };
            }
            Err(failure) => failures.push(failure),
        }
    }

    let terminal_failure = failures.last().cloned();
    RecoveryAttempt {
        loaded: None,
        diagnostic: SnapshotRecoveryDiagnostic::new(failures, false),
        terminal_failure,
    }
}

async fn recover_candidate<T: Send + 'static>(
    store: &dyn crate::ports::SnapshotStore,
    expected: SnapshotSource,
    candidate: &SnapshotCandidate,
    decoded_limit: u64,
    parse: SourceParser<T>,
    force_blocking: bool,
) -> Result<LoadedSource<T>, SafeFailure> {
    let metadata = candidate.metadata();
    if metadata.source() != expected {
        return Err(SafeFailure::SnapshotRecovery {
            kind: expected.kind(),
            reason: SnapshotRecoveryReason::SourceMismatch,
        });
    }
    if metadata.decoded_bytes() > decoded_limit {
        return Err(SafeFailure::DecodedLimitExceeded {
            kind: expected.kind(),
            limit_bytes: decoded_limit,
        });
    }

    let reader = store
        .open_candidate(candidate)
        .await
        .map_err(|reason| SafeFailure::Snapshot {
            kind: expected.kind(),
            operation: SnapshotOperation::OpenCandidate,
            reason,
        })?;
    let expected_kind = expected.kind();
    let expected_bytes = metadata.decoded_bytes();
    let expected_checksum = *metadata.checksum();
    let offload = force_blocking || expected_bytes >= BLOCKING_OFFLOAD_BYTES;
    let value = run_blocking_if(offload, move || {
        let mut reader = CandidateReader::new(reader, decoded_limit);
        let parsed = parse(&mut reader);
        let drain = io::copy(&mut reader, &mut io::sink());
        if reader.exceeded_limit {
            return Err(SafeFailure::DecodedLimitExceeded {
                kind: expected_kind,
                limit_bytes: decoded_limit,
            });
        }
        if reader.read_failed || drain.is_err() {
            return Err(SafeFailure::Snapshot {
                kind: expected_kind,
                operation: SnapshotOperation::OpenCandidate,
                reason: crate::domain::StoreError::Unavailable,
            });
        }
        if reader.decoded_bytes != expected_bytes {
            return Err(SafeFailure::SnapshotRecovery {
                kind: expected_kind,
                reason: SnapshotRecoveryReason::LengthMismatch,
            });
        }
        if reader.checksum.finalize().as_bytes() != &expected_checksum {
            return Err(SafeFailure::SnapshotRecovery {
                kind: expected_kind,
                reason: SnapshotRecoveryReason::ChecksumMismatch,
            });
        }
        parsed
    })
    .await?;

    Ok(LoadedSource {
        value: Arc::new(value),
        checksum: expected_checksum,
        validated_at: metadata.validated_at(),
        candidate: candidate.clone(),
    })
}

struct CandidateReader<R> {
    inner: R,
    checksum: blake3::Hasher,
    decoded_bytes: u64,
    decoded_limit: u64,
    exceeded_limit: bool,
    read_failed: bool,
}

impl<R> CandidateReader<R> {
    fn new(inner: R, decoded_limit: u64) -> Self {
        Self {
            inner,
            checksum: blake3::Hasher::new(),
            decoded_bytes: 0,
            decoded_limit,
            exceeded_limit: false,
            read_failed: false,
        }
    }

    fn record(&mut self, bytes: &[u8]) {
        self.decoded_bytes = self.decoded_bytes.saturating_add(bytes.len() as u64);
        self.exceeded_limit |= self.decoded_bytes > self.decoded_limit;
        self.checksum.update(bytes);
    }

    fn limit_error() -> io::Error {
        io::Error::other("snapshot candidate exceeds its decoded limit")
    }
}

impl<R: BufRead> Read for CandidateReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.exceeded_limit {
            return Err(Self::limit_error());
        }
        match self.inner.read(buffer) {
            Ok(read) => {
                self.record(&buffer[..read]);
                Ok(read)
            }
            Err(error) => {
                self.read_failed = true;
                Err(error)
            }
        }
    }
}

impl<R: BufRead> BufRead for CandidateReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.exceeded_limit {
            return Err(Self::limit_error());
        }
        match self.inner.fill_buf() {
            Ok(buffer) => Ok(buffer),
            Err(error) => {
                self.read_failed = true;
                Err(error)
            }
        }
    }

    fn consume(&mut self, amount: usize) {
        match self.inner.fill_buf() {
            Ok(buffer) => {
                let amount = amount.min(buffer.len());
                self.decoded_bytes = self.decoded_bytes.saturating_add(amount as u64);
                self.exceeded_limit |= self.decoded_bytes > self.decoded_limit;
                self.checksum.update(&buffer[..amount]);
                self.inner.consume(amount);
            }
            Err(_) => self.read_failed = true,
        }
    }
}

fn source_state(
    validated_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> SourceState {
    let fresh = validated_at <= now
        && validated_at
            .checked_add_signed(chrono::Duration::hours(6))
            .is_some_and(|deadline| now < deadline);
    if fresh {
        SourceState::Fresh { validated_at }
    } else {
        SourceState::Stale {
            validated_at,
            next_attempt_at: Some(now),
        }
    }
}

fn recovered_source_state(
    validated_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    freshness: RecoveredFreshness,
) -> SourceState {
    match freshness {
        RecoveredFreshness::AgeBased => source_state(validated_at, now),
        RecoveredFreshness::PendingRevalidation => SourceState::Stale {
            validated_at,
            next_attempt_at: Some(now),
        },
    }
}

async fn load_source<T: Send + 'static>(
    adapters: &CoreAdapters,
    request: SourceRequest,
    snapshot: SnapshotSource,
    decoded_limit: u64,
    parse: SourceParser<T>,
) -> Result<LoadedSource<T>, SafeFailure> {
    match fetch_source(adapters, request, snapshot, None, decoded_limit, parse).await? {
        FetchedSource::Modified(loaded) => Ok(loaded),
        FetchedSource::NotModified { .. } => Err(SafeFailure::SourceAccess {
            kind: snapshot.kind(),
            reason: SourceAccessError::InvalidResponse,
            retry_after: None,
        }),
    }
}

enum FetchedSource<T> {
    Modified(LoadedSource<T>),
    NotModified {
        candidate: SnapshotCandidate,
        validated_at: chrono::DateTime<chrono::Utc>,
    },
}

async fn fetch_source<T: Send + 'static>(
    adapters: &CoreAdapters,
    request: SourceRequest,
    snapshot: SnapshotSource,
    protected: Option<SnapshotCandidate>,
    decoded_limit: u64,
    parse: SourceParser<T>,
) -> Result<FetchedSource<T>, SafeFailure> {
    let source = request.kind();
    debug_assert_eq!(source, snapshot.kind());
    let response = adapters
        .source_access()
        .open(request)
        .await
        .map_err(|failure| SafeFailure::SourceAccess {
            kind: source,
            reason: failure.reason(),
            retry_after: failure.retry_after(),
        })?;
    let (declared_length, mut body, validators) = match response.into_inner() {
        SourceResponseInner::Modified {
            declared_decoded_length,
            decoded_body,
            validators,
        } => (declared_decoded_length, decoded_body, validators),
        SourceResponseInner::NotModified { validators } => {
            let Some(current) = protected else {
                return Err(SafeFailure::SourceAccess {
                    kind: source,
                    reason: SourceAccessError::InvalidResponse,
                    retry_after: None,
                });
            };
            let validated_at = adapters.clock().now();
            let validators = current.metadata().validators().merged_with(&validators);
            let metadata = SnapshotMetadata::new(
                snapshot,
                current.metadata().decoded_bytes(),
                *current.metadata().checksum(),
                validated_at,
                validators.clone(),
            );
            let expected =
                SnapshotCandidate::new(current.token(), metadata, current.requires_adoption());
            let revalidation = SnapshotRevalidation::new(validated_at, validators);
            let candidate = adapters
                .snapshot_store()
                .revalidate_candidate(&current, &revalidation)
                .await
                .map_err(|reason| SafeFailure::Snapshot {
                    kind: source,
                    operation: SnapshotOperation::RevalidateCandidate,
                    reason,
                })?;
            if candidate != expected {
                return Err(SafeFailure::Snapshot {
                    kind: source,
                    operation: SnapshotOperation::RevalidateCandidate,
                    reason: crate::domain::StoreError::Corrupt,
                });
            }
            return Ok(FetchedSource::NotModified {
                candidate,
                validated_at,
            });
        }
    };

    if declared_length.is_some_and(|length| length > decoded_limit) {
        return Err(SafeFailure::DecodedLimitExceeded {
            kind: source,
            limit_bytes: decoded_limit,
        });
    }

    let store = adapters.snapshot_store();
    let stage = store
        .begin_stage(SnapshotStageRequest::new(snapshot, protected))
        .map_err(|reason| SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::BeginStage,
            reason,
        })?;
    let staged = StagedCandidate::new(store, stage);
    let mut decoded_bytes = 0_u64;
    let mut checksum = blake3::Hasher::new();

    while let Some(next) = body.next().await {
        let chunk = match next {
            Ok(chunk) => chunk,
            Err(reason) => {
                return Err(staged.reject(SafeFailure::SourceRead {
                    kind: source,
                    reason,
                }));
            }
        };
        decoded_bytes = match decoded_bytes.checked_add(chunk.len() as u64) {
            Some(length) if length <= decoded_limit => length,
            _ => {
                return Err(staged.reject(SafeFailure::DecodedLimitExceeded {
                    kind: source,
                    limit_bytes: decoded_limit,
                }));
            }
        };
        checksum.update(&chunk);
        if let Err(reason) = store.append(staged.stage(), chunk).await {
            return Err(staged.reject(SafeFailure::Snapshot {
                kind: source,
                operation: SnapshotOperation::WriteStage,
                reason,
            }));
        }
    }

    let mut reader = match store.open_staged(staged.stage()).await {
        Ok(reader) => reader,
        Err(reason) => {
            return Err(staged.reject(SafeFailure::Snapshot {
                kind: source,
                operation: SnapshotOperation::ReadStage,
                reason,
            }));
        }
    };
    let parsed = run_blocking_if(decoded_bytes >= BLOCKING_OFFLOAD_BYTES, move || {
        let result = parse(reader.as_mut());
        drop(reader);
        result
    })
    .await;
    let value = match parsed {
        Ok(value) => value,
        Err(failure) => {
            return Err(staged.reject(failure));
        }
    };

    let checksum = *checksum.finalize().as_bytes();
    let validated_at = adapters.clock().now();
    let validated = staged.validate(decoded_bytes, checksum, validated_at, validators);
    if let Err(reason) = store.prepare_activation(validated.value()).await {
        return Err(validated.reject(SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::PrepareActivation,
            reason,
        }));
    }
    let expected_metadata = validated.value().metadata();
    let candidate = match store.activate(validated.value()) {
        Ok(candidate) => candidate,
        Err(reason) => {
            return Err(validated.reject(SafeFailure::Snapshot {
                kind: source,
                operation: SnapshotOperation::Activate,
                reason,
            }));
        }
    };
    if candidate.metadata() != &expected_metadata || candidate.requires_adoption() {
        return Err(validated.reject(SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::Activate,
            reason: crate::domain::StoreError::Corrupt,
        }));
    }
    validated.commit();

    Ok(FetchedSource::Modified(LoadedSource {
        value: Arc::new(value),
        checksum,
        validated_at,
        candidate,
    }))
}

fn should_offload_catalog_build(
    channels: &[m3u::ParsedChannel],
    m3u_decoded_bytes: u64,
    guide: Option<&xmltv::ParsedGuide>,
    epg_decoded_bytes: u64,
    existing_has_epg: bool,
) -> bool {
    channels.len() >= BLOCKING_OFFLOAD_CHANNELS
        || m3u_decoded_bytes >= BLOCKING_OFFLOAD_BYTES
        || existing_has_epg
        || epg_decoded_bytes >= BLOCKING_OFFLOAD_BYTES
        || guide.is_some_and(|guide| guide.programmes.len() >= BLOCKING_OFFLOAD_CHANNELS)
}

fn should_offload_guide_update(
    existing_catalog: &ChannelCatalog,
    epg_decoded_bytes: u64,
    guide: &xmltv::ParsedGuide,
) -> bool {
    existing_catalog.as_bytes().len() >= BLOCKING_OFFLOAD_BYTES as usize
        || epg_decoded_bytes >= BLOCKING_OFFLOAD_BYTES
        || guide.programmes.len() >= BLOCKING_OFFLOAD_CHANNELS
}

async fn run_blocking_if<F, R>(offload: bool, job: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if offload {
        run_blocking(job).await
    } else {
        job()
    }
}

async fn run_blocking<F, R>(job: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle
            .spawn_blocking(job)
            .await
            .expect("blocking core task did not panic"),
        Err(_) => job(),
    }
}

fn valid_adoption(before: &SnapshotCandidate, after: &SnapshotCandidate) -> bool {
    before.token() == after.token()
        && before.metadata() == after.metadata()
        && !after.requires_adoption()
}

fn discard_after(
    store: &dyn crate::ports::SnapshotStore,
    stage: SnapshotStage,
    original: SafeFailure,
) -> SafeFailure {
    let source = stage.source().kind();
    match store.discard(stage) {
        Ok(()) => original,
        Err(reason) => SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::Discard,
            reason,
        },
    }
}

struct StagedCandidate<'a> {
    store: &'a dyn crate::ports::SnapshotStore,
    stage: Option<SnapshotStage>,
}

impl<'a> StagedCandidate<'a> {
    fn new(store: &'a dyn crate::ports::SnapshotStore, stage: SnapshotStage) -> Self {
        Self {
            store,
            stage: Some(stage),
        }
    }

    fn stage(&self) -> &SnapshotStage {
        self.stage.as_ref().expect("staged candidate is armed")
    }

    fn validate(
        mut self,
        decoded_bytes: u64,
        checksum: [u8; 32],
        validated_at: chrono::DateTime<chrono::Utc>,
        validators: crate::ports::PrivateSourceValidators,
    ) -> ValidatedCandidate<'a> {
        let stage = self.stage.take().expect("staged candidate is armed");
        ValidatedCandidate {
            store: self.store,
            validated: Some(ValidatedStage::new(
                stage,
                decoded_bytes,
                checksum,
                validated_at,
                validators,
            )),
        }
    }

    fn reject(mut self, original: SafeFailure) -> SafeFailure {
        let stage = self.stage.take().expect("staged candidate is armed");
        discard_after(self.store, stage, original)
    }
}

impl Drop for StagedCandidate<'_> {
    fn drop(&mut self) {
        if let Some(stage) = self.stage.take() {
            let _ = self.store.discard(stage);
        }
    }
}

struct ValidatedCandidate<'a> {
    store: &'a dyn crate::ports::SnapshotStore,
    validated: Option<ValidatedStage>,
}

impl ValidatedCandidate<'_> {
    fn value(&self) -> &ValidatedStage {
        self.validated
            .as_ref()
            .expect("validated candidate is armed")
    }

    fn reject(mut self, original: SafeFailure) -> SafeFailure {
        let stage = self
            .validated
            .take()
            .expect("validated candidate is armed")
            .into_stage();
        discard_after(self.store, stage, original)
    }

    fn commit(mut self) {
        self.validated.take();
    }
}

impl Drop for ValidatedCandidate<'_> {
    fn drop(&mut self) {
        if let Some(validated) = self.validated.take() {
            let _ = self.store.discard(validated.into_stage());
        }
    }
}

#[cfg(test)]
mod refresh_concurrency_tests {
    use std::{
        io::BufRead,
        sync::{Arc, Condvar, Mutex, atomic::Ordering, mpsc},
        thread,
        time::Duration,
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use chrono::{DateTime, Utc};

    use crate::{
        domain::{
            RefreshOutcome, SourceAccessFailure, SourceConfiguration, SourceConfigurationInput,
            SourceKind, StoreError,
        },
        ports::{
            Clock, CoreAdapters, SnapshotCandidate, SnapshotStage, SnapshotStageRequest,
            SnapshotStore, SourceAccess, SourceRequest, SourceResponse, ValidatedStage,
        },
    };

    use super::{
        BootstrapFailures, ConfigurationContext, CoreRuntime, CoreView, FlightDecision,
        RefreshFlight, SparrowCore,
    };

    #[test]
    fn manual_promotion_and_automatic_skip_have_one_linearized_winner() {
        let promoted_first = RefreshFlight::new(configuration_context(), false);
        assert!(promoted_first.try_promote());
        assert!(!promoted_first.try_commit_skip());
        assert!(promoted_first.manual.load(Ordering::Acquire));

        let skipped_first = RefreshFlight::new(configuration_context(), false);
        assert!(skipped_first.try_commit_skip());
        assert!(!skipped_first.try_promote());
        assert_eq!(
            *skipped_first
                .decision
                .lock()
                .expect("refresh decision remains available"),
            FlightDecision::Skipped
        );
    }

    #[tokio::test]
    #[should_panic(expected = "the shared refresh task panicked")]
    async fn a_panicked_shared_task_wakes_waiters_instead_of_hanging() {
        let flight = RefreshFlight::new(configuration_context(), false);
        flight.fail_panicked();
        let _ = flight.wait().await;
    }

    #[test]
    fn refresh_completion_excludes_subscription_snapshot_until_the_event_is_published() {
        let clock = Arc::new(CompletionBlockingClock::at("2026-08-30T12:00:00Z"));
        let adapters = CoreAdapters::new(
            Arc::new(UnusedAdapter),
            Arc::new(UnusedAdapter),
            Arc::clone(&clock) as Arc<_>,
        );
        let core = SparrowCore::from_runtime(CoreRuntime::new(
            None,
            adapters,
            CoreView::not_configured(),
            BootstrapFailures::default(),
        ));

        let publishing_runtime = Arc::clone(&core.runtime);
        let publisher = thread::spawn(move || {
            let flight = publishing_runtime
                .control(SourceKind::M3u)
                .flight
                .lock()
                .expect("refresh flight is not poisoned");
            publishing_runtime.publish_refresh_completed(
                &flight,
                0,
                SourceKind::M3u,
                &RefreshOutcome::NotConfigured,
            );
        });
        clock.wait_until_blocked();

        let completion_holds_publication = core.runtime.publication.try_lock().is_err();
        let subscribing_core = core.clone();
        let (subscription_tx, subscription_rx) = mpsc::sync_channel(1);
        let subscriber = thread::spawn(move || {
            subscription_tx
                .send(subscribing_core.subscribe())
                .expect("test retains the subscription receiver");
        });

        clock.release();
        publisher
            .join()
            .expect("completion publisher does not panic");
        let mut events = subscription_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("subscription completes after publication");
        subscriber.join().expect("subscriber does not panic");

        assert!(
            completion_holds_publication,
            "completion must exclude subscribe and lag-resync snapshot synthesis"
        );
        assert!(matches!(
            events.initial.take(),
            Some(crate::domain::CoreEvent::CatalogStatusChanged { .. })
        ));
        assert!(matches!(
            events.receiver.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    fn configuration_context() -> ConfigurationContext {
        let configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/channels.m3u",
            None::<String>,
        ))
        .expect("fixture Source Configuration is valid");
        ConfigurationContext {
            epoch: 0,
            configuration: Arc::new(configuration),
        }
    }

    struct CompletionBlockingClock {
        now: DateTime<Utc>,
        state: Mutex<CompletionBlockingClockState>,
        changed: Condvar,
    }

    struct CompletionBlockingClockState {
        block_next: bool,
        blocked: bool,
        released: bool,
    }

    impl CompletionBlockingClock {
        fn at(value: &str) -> Self {
            Self {
                now: DateTime::parse_from_rfc3339(value)
                    .expect("fixture instant is valid")
                    .with_timezone(&Utc),
                state: Mutex::new(CompletionBlockingClockState {
                    block_next: true,
                    blocked: false,
                    released: false,
                }),
                changed: Condvar::new(),
            }
        }

        fn wait_until_blocked(&self) {
            let state = self.state.lock().expect("test clock is not poisoned");
            let (state, result) = self
                .changed
                .wait_timeout_while(state, Duration::from_secs(2), |state| !state.blocked)
                .expect("test clock is not poisoned");
            assert!(
                !result.timed_out(),
                "completion reaches the controlled clock"
            );
            assert!(state.blocked);
        }

        fn release(&self) {
            let mut state = self.state.lock().expect("test clock is not poisoned");
            state.released = true;
            self.changed.notify_all();
        }
    }

    #[async_trait]
    impl Clock for CompletionBlockingClock {
        fn now(&self) -> DateTime<Utc> {
            let mut state = self.state.lock().expect("test clock is not poisoned");
            if state.block_next {
                state.block_next = false;
                state.blocked = true;
                self.changed.notify_all();
                while !state.released {
                    state = self
                        .changed
                        .wait(state)
                        .expect("test clock is not poisoned");
                }
            }
            self.now
        }

        async fn wait_until(&self, _deadline: DateTime<Utc>) {
            std::future::pending().await
        }
    }

    struct UnusedAdapter;

    #[async_trait]
    impl SourceAccess for UnusedAdapter {
        async fn open(
            &self,
            _request: SourceRequest,
        ) -> Result<SourceResponse, SourceAccessFailure> {
            unreachable!("the event fixture never opens a Source")
        }
    }

    #[async_trait]
    impl SnapshotStore for UnusedAdapter {
        fn begin_stage(&self, _request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
            unreachable!("the event fixture never stages a Snapshot")
        }

        async fn append(&self, _stage: &SnapshotStage, _chunk: Bytes) -> Result<(), StoreError> {
            unreachable!("the event fixture never writes a Snapshot")
        }

        async fn open_staged(
            &self,
            _stage: &SnapshotStage,
        ) -> Result<Box<dyn BufRead + Send>, StoreError> {
            unreachable!("the event fixture never reads a Snapshot")
        }

        async fn prepare_activation(&self, _validated: &ValidatedStage) -> Result<(), StoreError> {
            unreachable!("the event fixture never prepares a Snapshot")
        }

        fn activate(&self, _validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
            unreachable!("the event fixture never activates a Snapshot")
        }

        fn discard(&self, _stage: SnapshotStage) -> Result<(), StoreError> {
            unreachable!("the event fixture never discards a Snapshot")
        }
    }
}
