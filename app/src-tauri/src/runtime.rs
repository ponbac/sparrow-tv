use std::{
    collections::{HashMap, VecDeque},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use sparrow_bounded_blocking::{BlockingTaskCancellation, BoundedBlocking};
use sparrow_core::{
    ChannelSummary, CoreAdapters, CoreError, Page, PageRequest, ProgrammeSummary, RefreshReport,
    RefreshTrigger, SearchRequest, SearchResults, SearchTerm, SparrowCore, SystemClock,
};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use sparrow_source_http::HttpPlaybackAccess;
use sparrow_source_http::HttpSourceAccess;
use tokio::sync::{Mutex, Notify, watch};

use crate::{
    android_playback,
    audio_preferences::AudioPreferenceStore,
    config_store::{
        ConfigurationStoreError, SourceConfigurationStore, StoredSourceConfiguration,
        ensure_private_directory,
    },
    instance_lock::{InstanceLock, InstanceLockError},
    ipc::{
        dto::{CatalogStatusDto, ClientErrorDto, CoreEventDto},
        input::{SearchRequestId, SourceConfigurationInputDto},
        subscriptions::SubscriptionRegistry,
    },
    playback::{
        InstalledPlaybackStart, NativeStreamHandle, PlaybackManager, PlaybackManagerError,
        PlaybackRestartIntent, PlaybackSessionId,
    },
    screen_wake::ScreenWake,
};

#[cfg(test)]
use crate::screen_wake::noop_screen_wake;

const PRIVATE_DIRECTORY: &str = "private-v1";
const SNAPSHOT_DIRECTORY: &str = "snapshots-v1";
const MAX_TRACKED_SEARCH_REQUESTS: usize = 64;

/// Process-lifetime rendezvous between early WebView invokes and runtime startup.
#[derive(Clone)]
pub(crate) struct InstalledRuntimeSlot {
    inner: Arc<InstalledRuntimeSlotInner>,
}

struct InstalledRuntimeSlotInner {
    runtime: OnceLock<Arc<InstalledRuntime>>,
    ready: watch::Sender<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InstalledRuntimeAlreadyReady;

impl InstalledRuntimeSlot {
    pub(crate) fn new() -> Self {
        let (ready, _) = watch::channel(false);
        Self {
            inner: Arc::new(InstalledRuntimeSlotInner {
                runtime: OnceLock::new(),
                ready,
            }),
        }
    }

    pub(crate) fn fill(
        &self,
        runtime: Arc<InstalledRuntime>,
    ) -> Result<(), InstalledRuntimeAlreadyReady> {
        self.inner
            .runtime
            .set(runtime)
            .map_err(|_| InstalledRuntimeAlreadyReady)?;
        self.inner.ready.send_replace(true);
        Ok(())
    }

    pub(crate) async fn wait(&self) -> Arc<InstalledRuntime> {
        let mut ready = self.inner.ready.subscribe();
        loop {
            if let Some(runtime) = self.ready() {
                return runtime;
            }
            ready
                .changed()
                .await
                .expect("the process-lifetime runtime slot remains open");
        }
    }

    pub(crate) fn ready(&self) -> Option<Arc<InstalledRuntime>> {
        self.inner.runtime.get().map(Arc::clone)
    }
}

/// The complete on-device catalog composition managed by Tauri.
pub(crate) struct InstalledRuntime {
    playback: Arc<PlaybackManager>,
    core: Arc<SparrowCore>,
    configuration_store: SourceConfigurationStore,
    configuration_mutation: Mutex<()>,
    searches: BoundedBlocking,
    search_cancellations: SearchCancellationRegistry,
    subscriptions: SubscriptionRegistry,
    android_presentation: AndroidPresentationGate,
    lifecycle_order: LifecycleOrder,
    lifecycle_revision: AtomicU64,
    _instance_lock: InstanceLock,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledLifecycleEvent {
    revision: u64,
    state: &'static str,
}

impl InstalledRuntime {
    #[cfg(test)]
    pub(crate) async fn open(app_data: PathBuf) -> Result<Self, InstalledStartupError> {
        Self::open_with_screen_wake(app_data, noop_screen_wake()).await
    }

    pub(crate) async fn open_with_screen_wake(
        app_data: PathBuf,
        screen_wake: Arc<dyn ScreenWake>,
    ) -> Result<Self, InstalledStartupError> {
        prepare_app_data(&app_data)?;
        let private_root = app_data.join(PRIVATE_DIRECTORY);
        ensure_private_directory(&private_root).map_err(InstalledStartupError::from)?;
        let instance_lock =
            InstanceLock::acquire(&private_root).map_err(InstalledStartupError::from)?;
        let configuration_store =
            SourceConfigurationStore::open(&private_root).map_err(InstalledStartupError::from)?;
        let configuration = load_persisted_configuration(&configuration_store)?;

        let source =
            Arc::new(HttpSourceAccess::new().map_err(|_| InstalledStartupError::SourceAdapter)?);
        let snapshots = Arc::new(
            AtomicFileSnapshotStore::open(private_root.join(SNAPSHOT_DIRECTORY))
                .map_err(|_| InstalledStartupError::SnapshotAdapter)?,
        );
        let core = Arc::new(
            SparrowCore::bootstrap_from_snapshots(
                configuration,
                CoreAdapters::new(source, snapshots, Arc::new(SystemClock)),
            )
            .await
            .map_err(|_| InstalledStartupError::Core)?,
        );
        let playback_access =
            HttpPlaybackAccess::new().map_err(|_| InstalledStartupError::PlaybackAdapter)?;
        let audio_preferences = AudioPreferenceStore::open(&private_root);
        let playback = Arc::new(PlaybackManager::new_with_screen_wake(
            Arc::clone(&core),
            playback_access,
            audio_preferences,
            private_root.clone(),
            screen_wake,
        ));

        Ok(Self {
            playback,
            core,
            configuration_store,
            configuration_mutation: Mutex::new(()),
            searches: BoundedBlocking::serial(),
            search_cancellations: SearchCancellationRegistry::default(),
            subscriptions: SubscriptionRegistry::default(),
            android_presentation: AndroidPresentationGate::default(),
            lifecycle_order: LifecycleOrder::default(),
            lifecycle_revision: AtomicU64::new(0),
            _instance_lock: instance_lock,
        })
    }

    pub(crate) fn core(&self) -> &SparrowCore {
        &self.core
    }

    pub(crate) fn status(&self) -> sparrow_core::CatalogStatus {
        let status = self.core.status();
        self.core.activate_automation();
        status
    }

    pub(crate) async fn replace_configuration(
        &self,
        input: SourceConfigurationInputDto,
    ) -> Result<CatalogStatusDto, ClientErrorDto> {
        let (stored, configuration) = input.validate()?;
        let _mutation = self.configuration_mutation.lock().await;
        self.persist_configuration(stored).await?;
        let status = self
            .core
            .replace_source_configuration(Some(configuration))
            .await;
        Ok(CatalogStatusDto::from(status))
    }

    pub(crate) async fn refresh(&self) -> RefreshReport {
        self.core.refresh(RefreshTrigger::Manual).await
    }

    pub(crate) async fn search(
        &self,
        request_id: SearchRequestId,
        request: SearchRequest,
    ) -> Result<SearchResults, ClientErrorDto> {
        self.run_search(request_id, move |core, cancellation| {
            core.search_with_cancellation(request, || cancellation.is_cancelled())
        })
        .await
    }

    pub(crate) async fn search_channels(
        &self,
        request_id: SearchRequestId,
        term: SearchTerm,
        page: PageRequest,
    ) -> Result<Page<ChannelSummary>, ClientErrorDto> {
        self.run_search(request_id, move |core, cancellation| {
            core.search_channels_with_cancellation(term, page, || cancellation.is_cancelled())
        })
        .await
    }

    pub(crate) async fn search_programmes(
        &self,
        request_id: SearchRequestId,
        term: SearchTerm,
        page: PageRequest,
    ) -> Result<Page<ProgrammeSummary>, ClientErrorDto> {
        self.run_search(request_id, move |core, cancellation| {
            core.search_programmes_with_cancellation(term, page, || cancellation.is_cancelled())
        })
        .await
    }

    pub(crate) fn cancel_search(&self, request_id: SearchRequestId) {
        self.search_cancellations.cancel(request_id);
    }

    async fn run_search<Output, Job>(
        &self,
        request_id: SearchRequestId,
        job: Job,
    ) -> Result<Output, ClientErrorDto>
    where
        Job: FnOnce(Arc<SparrowCore>, BlockingTaskCancellation) -> Result<Output, CoreError>
            + Send
            + 'static,
        Output: Send + 'static,
    {
        let registration = self
            .search_cancellations
            .register(request_id)
            .map_err(|_| ClientErrorDto::service_unavailable())?;
        if registration.is_cancelled() {
            return Err(ClientErrorDto::service_unavailable());
        }

        let core = Arc::clone(&self.core);
        let result = self
            .searches
            .run_with_cancellation(registration.cancellation(), move |cancellation| {
                job(core, cancellation)
            })
            .await
            .map_err(|_| ClientErrorDto::service_unavailable())?;
        drop(registration);
        result.map_err(ClientErrorDto::from)
    }

    async fn persist_configuration(
        &self,
        configuration: StoredSourceConfiguration,
    ) -> Result<(), ClientErrorDto> {
        let store = self.configuration_store.clone();
        tokio::task::spawn_blocking(move || store.save(&configuration))
            .await
            .map_err(|_| ClientErrorDto::service_unavailable())?
            .map_err(|_| ClientErrorDto::service_unavailable())
    }

    pub(crate) fn subscribe(
        &self,
        events: tauri::ipc::Channel<CoreEventDto>,
    ) -> Result<String, ClientErrorDto> {
        self.subscriptions.subscribe(Arc::clone(&self.core), events)
    }

    pub(crate) fn unsubscribe(&self, subscription_id: &str) {
        self.subscriptions.unsubscribe(subscription_id);
    }

    pub(crate) async fn start_playback(
        &self,
        session_id: PlaybackSessionId,
        channel_id: sparrow_core::ChannelId,
    ) -> Result<InstalledPlaybackStart, PlaybackManagerError> {
        #[cfg(target_os = "linux")]
        {
            self.playback
                .start_mpv_primary(session_id, channel_id)
                .await
                .map(InstalledPlaybackStart::LinuxMpv)
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.playback
                .start(session_id, channel_id)
                .await
                .map(InstalledPlaybackStart::NativeStream)
        }
    }

    pub(crate) async fn read_playback(
        &self,
        session_id: PlaybackSessionId,
        stream_handle: NativeStreamHandle,
    ) -> Result<Vec<u8>, PlaybackManagerError> {
        self.playback.read(session_id, stream_handle).await
    }

    pub(crate) async fn suspend_playback(
        &self,
        session_id: PlaybackSessionId,
    ) -> Result<(), PlaybackManagerError> {
        let stop_id = session_id.clone();
        let playback = Arc::clone(&self.playback);
        self.android_presentation
            .stop_after_transport(
                async move { playback.suspend(session_id).await },
                move || android_playback::suspend_session(&stop_id),
            )
            .await
    }

    pub(crate) async fn reopen_playback(
        &self,
        session_id: PlaybackSessionId,
    ) -> Result<InstalledPlaybackStart, PlaybackManagerError> {
        #[cfg(target_os = "linux")]
        {
            self.playback
                .reopen_mpv(session_id)
                .await
                .map(InstalledPlaybackStart::LinuxMpv)
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.android_presentation
                .transport_transition(self.playback.reopen(session_id))
                .await
                .map(InstalledPlaybackStart::NativeStream)
        }
    }

    pub(crate) async fn restart_playback(
        &self,
        session_id: PlaybackSessionId,
        expected_stream_handle: NativeStreamHandle,
        intent: PlaybackRestartIntent,
    ) -> Result<InstalledPlaybackStart, PlaybackManagerError> {
        let presentation_identity = android_playback::AndroidPlaybackIdentity::new(
            session_id.clone(),
            expected_stream_handle.clone(),
        );
        let playback = Arc::clone(&self.playback);
        let stop_session_id = session_id.clone();
        let stop_stream_handle = expected_stream_handle.clone();
        self.android_presentation
            .stop_before_transport_replace(
                async move {
                    playback
                        .suspend_generation(stop_session_id, stop_stream_handle)
                        .await
                },
                move || android_playback::stop(&presentation_identity),
                self.playback
                    .restart(session_id, expected_stream_handle, intent),
            )
            .await
            .map(InstalledPlaybackStart::NativeStream)
    }

    pub(crate) async fn stop_playback(
        &self,
        session_id: PlaybackSessionId,
        stream_handle: Option<NativeStreamHandle>,
    ) -> Result<(), PlaybackManagerError> {
        let stop_id = session_id.clone();
        let playback = Arc::clone(&self.playback);
        self.android_presentation
            .stop_after_transport(
                async move { playback.stop(session_id, stream_handle).await },
                move || android_playback::stop_session(&stop_id),
            )
            .await
    }

    pub(crate) async fn start_android_playback(
        &self,
        identity: android_playback::AndroidPlaybackIdentity,
        viewport: android_playback::AndroidPlaybackViewport,
        controls: android_playback::AndroidPlaybackControls,
    ) -> Result<(), PlaybackManagerError> {
        let session_id = identity.session_id().clone();
        let stream_handle = identity.stream_handle().clone();
        self.android_presentation
            .start_if_active(
                self.playback
                    .validate_active_generation(session_id, stream_handle),
                move || android_playback::start(&identity, viewport, controls),
            )
            .await
    }

    pub(crate) async fn android_playback_status(
        &self,
        identity: android_playback::AndroidPlaybackIdentity,
    ) -> Result<android_playback::AndroidPlaybackStatus, PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || android_playback::status(&identity))
            .await
    }

    pub(crate) async fn set_android_playback_controls(
        &self,
        identity: android_playback::AndroidPlaybackIdentity,
        controls: android_playback::AndroidPlaybackControls,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || android_playback::set_controls(&identity, controls))
            .await
    }

    pub(crate) async fn set_android_playback_viewport(
        &self,
        identity: android_playback::AndroidPlaybackIdentity,
        viewport: android_playback::AndroidPlaybackViewport,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || android_playback::set_viewport(&identity, viewport))
            .await
    }

    pub(crate) async fn stop_android_playback(
        &self,
        identity: android_playback::AndroidPlaybackIdentity,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || android_playback::stop(&identity))
            .await
    }

    pub(crate) async fn set_playback_activity(
        &self,
        session_id: PlaybackSessionId,
        active: bool,
    ) -> Result<(), PlaybackManagerError> {
        self.playback.set_activity(session_id, active).await
    }

    pub(crate) fn dispatch_lifecycle<Publish>(
        self: Arc<Self>,
        signal: sparrow_core::LifecycleSignal,
        publish: Publish,
    ) -> impl std::future::Future<Output = Result<(), PlaybackManagerError>> + Send + 'static
    where
        Publish: FnOnce(InstalledLifecycleEvent) + Send + 'static,
    {
        let sequence = self.lifecycle_order.reserve();
        async move {
            let sequence = sequence?;
            let _turn = self.lifecycle_order.wait(sequence).await?;
            if let Some(event) = self.apply_lifecycle(signal).await? {
                publish(event);
            }
            Ok(())
        }
    }

    async fn apply_lifecycle(
        &self,
        signal: sparrow_core::LifecycleSignal,
    ) -> Result<Option<InstalledLifecycleEvent>, PlaybackManagerError> {
        match signal {
            sparrow_core::LifecycleSignal::Started => {
                self.core.report_lifecycle(signal);
                Ok(None)
            }
            sparrow_core::LifecycleSignal::Suspended => {
                let playback = Arc::clone(&self.playback);
                let cleanup = self
                    .android_presentation
                    .stop_after_transport(
                        async move { playback.suspend_for_lifecycle().await },
                        android_playback::suspend_all,
                    )
                    .await;
                self.core.report_lifecycle(signal);
                cleanup?;
                self.lifecycle_event("suspended").map(Some)
            }
            sparrow_core::LifecycleSignal::Resumed => {
                self.android_presentation
                    .transport_transition(self.playback.resume_for_lifecycle())
                    .await?;
                self.core.report_lifecycle(signal);
                self.lifecycle_event("resumed").map(Some)
            }
        }
    }

    fn lifecycle_event(
        &self,
        state: &'static str,
    ) -> Result<InstalledLifecycleEvent, PlaybackManagerError> {
        let revision = self
            .lifecycle_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .map_err(|_| PlaybackManagerError::Unavailable)?
            + 1;
        Ok(InstalledLifecycleEvent { revision, state })
    }

    pub(crate) async fn control_mpv(
        &self,
        session_id: PlaybackSessionId,
        control: crate::playback::MpvPlaybackControl,
    ) -> Result<(), PlaybackManagerError> {
        self.playback.control_mpv(session_id, control).await
    }

    pub(crate) async fn shutdown_playback(&self) -> Result<(), PlaybackManagerError> {
        let playback = Arc::clone(&self.playback);
        self.android_presentation
            .stop_after_transport(
                async move { playback.shutdown().await },
                android_playback::stop_all,
            )
            .await
    }
}

#[derive(Default)]
struct LifecycleOrder {
    submitted: AtomicU64,
    turn: Mutex<u64>,
    changed: Notify,
}

impl LifecycleOrder {
    fn reserve(&self) -> Result<u64, PlaybackManagerError> {
        self.submitted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |sequence| {
                sequence.checked_add(1)
            })
            .map_err(|_| PlaybackManagerError::Unavailable)
    }

    async fn wait(&self, sequence: u64) -> Result<LifecycleTurn<'_>, PlaybackManagerError> {
        loop {
            let changed = self.changed.notified();
            let turn = self.turn.lock().await;
            if *turn == sequence {
                return Ok(LifecycleTurn {
                    turn,
                    changed: &self.changed,
                });
            }
            if *turn > sequence {
                return Err(PlaybackManagerError::Unavailable);
            }
            drop(turn);
            changed.await;
        }
    }
}

struct LifecycleTurn<'a> {
    turn: tokio::sync::MutexGuard<'a, u64>,
    changed: &'a Notify,
}

impl Drop for LifecycleTurn<'_> {
    fn drop(&mut self) {
        *self.turn = self.turn.saturating_add(1);
        self.changed.notify_waiters();
    }
}

/// Serializes the Rust transport and Android presentation boundary.
///
/// The permit spans both sides of each transition. A delayed JNI start must
/// therefore either publish first and be followed by cleanup, or observe the
/// transport cleanup through `validate_active_generation` and fail closed.
#[derive(Default)]
struct AndroidPresentationGate {
    operation: Arc<Mutex<()>>,
}

impl AndroidPresentationGate {
    async fn start_if_active(
        &self,
        validate_transport: impl std::future::Future<Output = Result<(), PlaybackManagerError>>,
        start_presentation: impl FnOnce() -> Result<(), android_playback::AndroidPlaybackError>
        + Send
        + 'static,
    ) -> Result<(), PlaybackManagerError> {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        validate_transport.await?;
        run_android_presentation(permit, start_presentation)
            .await
            .map(|_| ())
    }

    async fn presentation_call<Output: Send + 'static>(
        &self,
        operation: impl FnOnce() -> Result<Output, android_playback::AndroidPlaybackError>
        + Send
        + 'static,
    ) -> Result<Output, PlaybackManagerError> {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        run_android_presentation(permit, operation)
            .await
            .map(|(_, output)| output)
    }

    async fn transport_transition<Output>(
        &self,
        transition: impl std::future::Future<Output = Result<Output, PlaybackManagerError>>,
    ) -> Result<Output, PlaybackManagerError> {
        let _permit = Arc::clone(&self.operation).lock_owned().await;
        transition.await
    }

    async fn stop_after_transport(
        &self,
        stop_transport: impl std::future::Future<Output = Result<(), PlaybackManagerError>>
        + Send
        + 'static,
        stop_presentation: impl FnOnce() -> Result<(), android_playback::AndroidPlaybackError>
        + Send
        + 'static,
    ) -> Result<(), PlaybackManagerError> {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        run_android_cleanup(permit, stop_transport, stop_presentation)
            .await
            .map(drop)
    }

    async fn stop_before_transport_replace<Output>(
        &self,
        stop_transport: impl std::future::Future<Output = Result<(), PlaybackManagerError>>
        + Send
        + 'static,
        stop_presentation: impl FnOnce() -> Result<(), android_playback::AndroidPlaybackError>
        + Send
        + 'static,
        replace_transport: impl std::future::Future<Output = Result<Output, PlaybackManagerError>>,
    ) -> Result<Output, PlaybackManagerError> {
        let permit = Arc::clone(&self.operation).lock_owned().await;
        let permit = run_android_cleanup(permit, stop_transport, stop_presentation).await?;
        let _permit = permit;
        replace_transport.await
    }
}

/// Owns the complete bounded cleanup phase independently of its requesting task.
///
/// Dropping the returned join handle detaches this task, so an aborted invoke
/// cannot release the gate after stopping transport but before stopping the
/// Android presentation. The permit is returned only when both cleanup steps
/// succeed; restart callers then retain it while polling the replacement. The
/// caller acquires the permit before spawning, which bounds independent cleanup
/// work to one task; cancelled gate waiters never create another task.
async fn run_android_cleanup(
    permit: tokio::sync::OwnedMutexGuard<()>,
    stop_transport: impl std::future::Future<Output = Result<(), PlaybackManagerError>> + Send + 'static,
    stop_presentation: impl FnOnce() -> Result<(), android_playback::AndroidPlaybackError>
    + Send
    + 'static,
) -> Result<tokio::sync::OwnedMutexGuard<()>, PlaybackManagerError> {
    tokio::spawn(async move {
        let transport_result = stop_transport.await;
        let presentation_result = run_android_presentation(permit, stop_presentation).await;

        // Presentation cleanup is always attempted, while the transport error
        // remains the primary failure when both sides fail.
        transport_result?;
        presentation_result.map(|(permit, ())| permit)
    })
    .await
    .map_err(|_| PlaybackManagerError::Unavailable)?
}

async fn run_android_presentation<Output: Send + 'static>(
    permit: tokio::sync::OwnedMutexGuard<()>,
    operation: impl FnOnce() -> Result<Output, android_playback::AndroidPlaybackError> + Send + 'static,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, Output), PlaybackManagerError> {
    let (permit, result) = tokio::task::spawn_blocking(move || (permit, operation()))
        .await
        .map_err(|_| PlaybackManagerError::Unavailable)?;
    result
        .map(|output| (permit, output))
        .map_err(|_| PlaybackManagerError::Unavailable)
}

#[derive(Default)]
struct SearchCancellationRegistry {
    state: StdMutex<SearchCancellationState>,
}

#[derive(Default)]
struct SearchCancellationState {
    entries: HashMap<SearchRequestId, SearchCancellationEntry>,
    pending_cancellations: VecDeque<SearchRequestId>,
}

struct SearchCancellationEntry {
    cancellation: BlockingTaskCancellation,
    claimed: bool,
}

struct SearchRegistration<'a> {
    registry: &'a SearchCancellationRegistry,
    request_id: SearchRequestId,
    cancellation: BlockingTaskCancellation,
}

impl SearchCancellationRegistry {
    fn register(&self, request_id: SearchRequestId) -> Result<SearchRegistration<'_>, ()> {
        let mut state = self
            .state
            .lock()
            .expect("search cancellation state poisoned");
        let cancellation = match state.entries.get_mut(&request_id) {
            Some(entry) if entry.claimed => return Err(()),
            Some(entry) => {
                entry.claimed = true;
                entry.cancellation.clone()
            }
            None => {
                if !state.make_room() {
                    return Err(());
                }
                let cancellation = BlockingTaskCancellation::new();
                state.entries.insert(
                    request_id.clone(),
                    SearchCancellationEntry {
                        cancellation: cancellation.clone(),
                        claimed: true,
                    },
                );
                cancellation
            }
        };
        drop(state);
        Ok(SearchRegistration {
            registry: self,
            request_id,
            cancellation,
        })
    }

    fn cancel(&self, request_id: SearchRequestId) {
        let mut state = self
            .state
            .lock()
            .expect("search cancellation state poisoned");
        if let Some(entry) = state.entries.get(&request_id) {
            entry.cancellation.cancel();
            return;
        }
        if !state.make_room() {
            return;
        }
        let cancellation = BlockingTaskCancellation::new();
        cancellation.cancel();
        state.entries.insert(
            request_id.clone(),
            SearchCancellationEntry {
                cancellation,
                claimed: false,
            },
        );
        state.pending_cancellations.push_back(request_id);
    }

    fn remove(&self, request_id: &SearchRequestId, cancellation: &BlockingTaskCancellation) {
        let mut state = self
            .state
            .lock()
            .expect("search cancellation state poisoned");
        if state
            .entries
            .get(request_id)
            .is_some_and(|entry| entry.cancellation.same_request(cancellation))
        {
            state.entries.remove(request_id);
        }
    }
}

impl SearchCancellationState {
    fn make_room(&mut self) -> bool {
        while self.entries.len() >= MAX_TRACKED_SEARCH_REQUESTS {
            let Some(request_id) = self.pending_cancellations.pop_front() else {
                return false;
            };
            if self
                .entries
                .get(&request_id)
                .is_some_and(|entry| !entry.claimed)
            {
                self.entries.remove(&request_id);
            }
        }
        true
    }
}

impl SearchRegistration<'_> {
    fn cancellation(&self) -> BlockingTaskCancellation {
        self.cancellation.clone()
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

impl Drop for SearchRegistration<'_> {
    fn drop(&mut self) {
        self.registry.remove(&self.request_id, &self.cancellation);
    }
}

fn load_persisted_configuration(
    store: &SourceConfigurationStore,
) -> Result<Option<sparrow_core::SourceConfiguration>, InstalledStartupError> {
    let stored = match store.load() {
        Ok(stored) => stored,
        Err(ConfigurationStoreError::Corrupt) => return Ok(None),
        Err(error) => return Err(InstalledStartupError::from(error)),
    };
    Ok(stored
        .and_then(|stored| SparrowCore::parse_source_configuration(stored.source_input()).ok()))
}

fn prepare_app_data(path: &Path) -> Result<(), InstalledStartupError> {
    fs::create_dir_all(path).map_err(|_| InstalledStartupError::AppData)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| InstalledStartupError::AppData)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstalledStartupError::AppData);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| InstalledStartupError::AppData)?;
    Ok(())
}

/// Safe startup failures deliberately discard filesystem and provider context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InstalledStartupError {
    #[error("the private app-data directory is unavailable")]
    AppData,
    #[error("another Sparrow instance is already running")]
    AlreadyRunning,
    #[error("the source configuration is unavailable")]
    Configuration,
    #[error("the source access adapter could not be initialized")]
    SourceAdapter,
    #[error("the snapshot adapter could not be initialized")]
    SnapshotAdapter,
    #[error("the playback adapter could not be initialized")]
    PlaybackAdapter,
    #[error("the catalog core could not be initialized")]
    Core,
}

impl From<ConfigurationStoreError> for InstalledStartupError {
    fn from(_error: ConfigurationStoreError) -> Self {
        Self::Configuration
    }
}

impl From<InstanceLockError> for InstalledStartupError {
    fn from(error: InstanceLockError) -> Self {
        match error {
            InstanceLockError::AlreadyRunning => Self::AlreadyRunning,
            InstanceLockError::Unavailable => Self::AppData,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read as _, Write as _},
        net::{TcpListener, TcpStream},
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc as std_mpsc,
        },
        thread,
    };

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn android_transport_and_presentation_stop_before_a_generation_is_replaced() {
        let gate = AndroidPresentationGate::default();
        let order = Arc::new(StdMutex::new(Vec::new()));
        let transport_stop_order = Arc::clone(&order);
        let stop_order = Arc::clone(&order);
        let replace_order = Arc::clone(&order);
        let replaced = gate
            .stop_before_transport_replace(
                async move {
                    transport_stop_order
                        .lock()
                        .expect("ordering fixture is readable")
                        .push("transport-stop");
                    Ok(())
                },
                move || {
                    stop_order
                        .lock()
                        .expect("ordering fixture is readable")
                        .push("presentation-stop");
                    Ok(())
                },
                async move {
                    replace_order
                        .lock()
                        .expect("ordering fixture is readable")
                        .push("transport-replace");
                    Ok::<_, PlaybackManagerError>(7)
                },
            )
            .await
            .expect("replacement follows presentation stop");

        assert_eq!(replaced, 7);
        assert_eq!(
            *order.lock().expect("ordering fixture is readable"),
            ["transport-stop", "presentation-stop", "transport-replace"]
        );

        let presentation_called = Arc::new(AtomicBool::new(false));
        let presentation_flag = Arc::clone(&presentation_called);
        let replacement_polled = Arc::new(AtomicBool::new(false));
        let replacement_flag = Arc::clone(&replacement_polled);
        let transport_failed = gate
            .stop_before_transport_replace(
                async { Err(PlaybackManagerError::Cancelled) },
                move || {
                    presentation_flag.store(true, Ordering::SeqCst);
                    Ok(())
                },
                async move {
                    replacement_flag.store(true, Ordering::SeqCst);
                    Ok::<_, PlaybackManagerError>(())
                },
            )
            .await;
        assert!(matches!(
            transport_failed,
            Err(PlaybackManagerError::Cancelled)
        ));
        assert!(presentation_called.load(Ordering::SeqCst));
        assert!(!replacement_polled.load(Ordering::SeqCst));

        let presentation_called = Arc::new(AtomicBool::new(false));
        let presentation_flag = Arc::clone(&presentation_called);
        let replacement_polled = Arc::new(AtomicBool::new(false));
        let replacement_flag = Arc::clone(&replacement_polled);
        let both_failed = gate
            .stop_before_transport_replace(
                async { Err(PlaybackManagerError::Cancelled) },
                move || {
                    presentation_flag.store(true, Ordering::SeqCst);
                    Err(android_playback::AndroidPlaybackError)
                },
                async move {
                    replacement_flag.store(true, Ordering::SeqCst);
                    Ok::<_, PlaybackManagerError>(())
                },
            )
            .await;
        assert!(matches!(both_failed, Err(PlaybackManagerError::Cancelled)));
        assert!(presentation_called.load(Ordering::SeqCst));
        assert!(!replacement_polled.load(Ordering::SeqCst));

        let replacement_polled = Arc::new(AtomicBool::new(false));
        let replacement_flag = Arc::clone(&replacement_polled);
        let failed = gate
            .stop_before_transport_replace(
                async { Ok(()) },
                || Err(android_playback::AndroidPlaybackError),
                async move {
                    replacement_flag.store(true, Ordering::SeqCst);
                    Ok::<_, PlaybackManagerError>(())
                },
            )
            .await;
        assert!(matches!(failed, Err(PlaybackManagerError::Unavailable)));
        assert!(!replacement_polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cleanup_attempts_android_release_after_transport_failure() {
        let gate = AndroidPresentationGate::default();
        let order = Arc::new(StdMutex::new(Vec::new()));
        let transport_order = Arc::clone(&order);
        let presentation_order = Arc::clone(&order);
        let result = gate
            .stop_after_transport(
                async move {
                    transport_order
                        .lock()
                        .expect("cleanup order is readable")
                        .push("transport");
                    Err(PlaybackManagerError::Cancelled)
                },
                move || {
                    presentation_order
                        .lock()
                        .expect("cleanup order is readable")
                        .push("presentation");
                    Ok(())
                },
            )
            .await;

        assert!(matches!(result, Err(PlaybackManagerError::Cancelled)));
        assert_eq!(
            *order.lock().expect("cleanup order is readable"),
            ["transport", "presentation"]
        );
        assert!(matches!(
            gate.stop_after_transport(async { Ok(()) }, || {
                Err(android_playback::AndroidPlaybackError)
            })
            .await,
            Err(PlaybackManagerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn aborted_stop_still_finishes_presentation_cleanup_before_the_gate_reopens() {
        let gate = Arc::new(AndroidPresentationGate::default());
        let presentation_stopped = Arc::new(AtomicBool::new(false));
        let following_call_started = Arc::new(AtomicBool::new(false));
        let (transport_entered, transport_entered_rx) = tokio::sync::oneshot::channel();
        let (release_transport, release_transport_rx) = tokio::sync::oneshot::channel();
        let (presentation_done, presentation_done_rx) = tokio::sync::oneshot::channel();
        let (following_requested, following_requested_rx) = tokio::sync::oneshot::channel();

        let cleanup_gate = Arc::clone(&gate);
        let presentation_flag = Arc::clone(&presentation_stopped);
        let cleanup = tokio::spawn(async move {
            cleanup_gate
                .stop_after_transport(
                    async move {
                        let _ = transport_entered.send(());
                        let _ = release_transport_rx.await;
                        Ok(())
                    },
                    move || {
                        presentation_flag.store(true, Ordering::SeqCst);
                        let _ = presentation_done.send(());
                        Ok(())
                    },
                )
                .await
        });
        transport_entered_rx
            .await
            .expect("detached cleanup begins transport stop");
        cleanup.abort();
        assert!(
            cleanup
                .await
                .expect_err("requesting task is aborted")
                .is_cancelled()
        );

        let following_gate = Arc::clone(&gate);
        let following_flag = Arc::clone(&following_call_started);
        let following = tokio::spawn(async move {
            let _ = following_requested.send(());
            following_gate
                .presentation_call(move || {
                    following_flag.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
        });
        following_requested_rx
            .await
            .expect("following call reaches the held gate");
        tokio::task::yield_now().await;
        assert!(!following_call_started.load(Ordering::SeqCst));

        let _ = release_transport.send(());
        presentation_done_rx
            .await
            .expect("presentation cleanup survives requester cancellation");
        following
            .await
            .expect("following task joins")
            .expect("gate reopens after detached cleanup");
        assert!(presentation_stopped.load(Ordering::SeqCst));
        assert!(following_call_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn aborted_restart_finishes_cleanup_without_polling_the_replacement() {
        let gate = Arc::new(AndroidPresentationGate::default());
        let presentation_stopped = Arc::new(AtomicBool::new(false));
        let replacement_polled = Arc::new(AtomicBool::new(false));
        let (transport_entered, transport_entered_rx) = tokio::sync::oneshot::channel();
        let (release_transport, release_transport_rx) = tokio::sync::oneshot::channel();
        let (presentation_done, presentation_done_rx) = tokio::sync::oneshot::channel();

        let restart_gate = Arc::clone(&gate);
        let presentation_flag = Arc::clone(&presentation_stopped);
        let replacement_flag = Arc::clone(&replacement_polled);
        let restart = tokio::spawn(async move {
            restart_gate
                .stop_before_transport_replace(
                    async move {
                        let _ = transport_entered.send(());
                        let _ = release_transport_rx.await;
                        Ok(())
                    },
                    move || {
                        presentation_flag.store(true, Ordering::SeqCst);
                        let _ = presentation_done.send(());
                        Ok(())
                    },
                    async move {
                        replacement_flag.store(true, Ordering::SeqCst);
                        Ok::<_, PlaybackManagerError>(())
                    },
                )
                .await
        });
        transport_entered_rx
            .await
            .expect("restart begins transport stop");
        restart.abort();
        assert!(
            restart
                .await
                .expect_err("requesting restart task is aborted")
                .is_cancelled()
        );

        let _ = release_transport.send(());
        presentation_done_rx
            .await
            .expect("restart presentation cleanup survives cancellation");
        gate.presentation_call(|| Ok(()))
            .await
            .expect("gate reopens after restart cleanup");
        assert!(presentation_stopped.load(Ordering::SeqCst));
        assert!(!replacement_polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cleanup_first_makes_a_delayed_android_start_fail_before_presentation() {
        let gate = Arc::new(AndroidPresentationGate::default());
        let transport_active = Arc::new(AtomicBool::new(true));
        let presentation_started = Arc::new(AtomicBool::new(false));
        let (cleanup_entered, cleanup_entered_rx) = tokio::sync::oneshot::channel();
        let (release_cleanup, release_cleanup_rx) = tokio::sync::oneshot::channel();

        let cleanup_gate = Arc::clone(&gate);
        let cleanup_active = Arc::clone(&transport_active);
        let cleanup = tokio::spawn(async move {
            cleanup_gate
                .stop_after_transport(
                    async move {
                        cleanup_active.store(false, Ordering::SeqCst);
                        let _ = cleanup_entered.send(());
                        let _ = release_cleanup_rx.await;
                        Ok(())
                    },
                    || Ok(()),
                )
                .await
        });
        cleanup_entered_rx
            .await
            .expect("cleanup invalidates transport while holding the gate");

        let start_gate = Arc::clone(&gate);
        let start_active = Arc::clone(&transport_active);
        let start_flag = Arc::clone(&presentation_started);
        let delayed_start = tokio::spawn(async move {
            start_gate
                .start_if_active(
                    async move {
                        start_active
                            .load(Ordering::SeqCst)
                            .then_some(())
                            .ok_or(PlaybackManagerError::Cancelled)
                    },
                    move || {
                        start_flag.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
        });
        let _ = release_cleanup.send(());

        cleanup
            .await
            .expect("cleanup task joins")
            .expect("cleanup succeeds");
        assert!(matches!(
            delayed_start.await.expect("start task joins"),
            Err(PlaybackManagerError::Cancelled)
        ));
        assert!(!presentation_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn start_first_is_followed_by_android_cleanup_before_the_gate_reopens() {
        let gate = Arc::new(AndroidPresentationGate::default());
        let transport_active = Arc::new(AtomicBool::new(true));
        let order = Arc::new(StdMutex::new(Vec::new()));
        let (start_entered, start_entered_rx) = std_mpsc::channel();
        let (release_start, release_start_rx) = std_mpsc::channel();

        let start_gate = Arc::clone(&gate);
        let start_active = Arc::clone(&transport_active);
        let start_order = Arc::clone(&order);
        let start = tokio::spawn(async move {
            start_gate
                .start_if_active(
                    async move {
                        start_active
                            .load(Ordering::SeqCst)
                            .then_some(())
                            .ok_or(PlaybackManagerError::Cancelled)
                    },
                    move || {
                        start_order
                            .lock()
                            .expect("presentation order is readable")
                            .push("presentation-start");
                        start_entered
                            .send(())
                            .expect("test observes presentation start");
                        release_start_rx
                            .recv()
                            .expect("test releases presentation start");
                        Ok(())
                    },
                )
                .await
        });
        tokio::task::spawn_blocking(move || start_entered_rx.recv())
            .await
            .expect("start observation joins")
            .expect("start owns the gate");

        let cleanup_gate = Arc::clone(&gate);
        let cleanup_active = Arc::clone(&transport_active);
        let cleanup_transport_order = Arc::clone(&order);
        let cleanup_presentation_order = Arc::clone(&order);
        let cleanup = tokio::spawn(async move {
            cleanup_gate
                .stop_after_transport(
                    async move {
                        cleanup_active.store(false, Ordering::SeqCst);
                        cleanup_transport_order
                            .lock()
                            .expect("presentation order is readable")
                            .push("transport-stop");
                        Ok(())
                    },
                    move || {
                        cleanup_presentation_order
                            .lock()
                            .expect("presentation order is readable")
                            .push("presentation-stop");
                        Ok(())
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            transport_active.load(Ordering::SeqCst),
            "cleanup cannot invalidate transport until the in-flight JNI start returns"
        );
        release_start
            .send(())
            .expect("presentation start is released");

        start
            .await
            .expect("start task joins")
            .expect("start succeeds");
        cleanup
            .await
            .expect("cleanup task joins")
            .expect("cleanup succeeds");
        assert_eq!(
            *order.lock().expect("presentation order is readable"),
            ["presentation-start", "transport-stop", "presentation-stop"]
        );
        assert!(!transport_active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn lifecycle_order_follows_signal_receipt_when_resume_runs_first() {
        let order = Arc::new(LifecycleOrder::default());
        let suspended_sequence = order.reserve().expect("suspend sequence reserves");
        let resumed_sequence = order.reserve().expect("resume sequence reserves");
        let events = Arc::new(StdMutex::new(Vec::new()));
        let resumed_order = Arc::clone(&order);
        let resumed_events = Arc::clone(&events);
        let resumed = tokio::spawn(async move {
            let _turn = resumed_order
                .wait(resumed_sequence)
                .await
                .expect("resume waits for its turn");
            resumed_events
                .lock()
                .expect("lifecycle events are readable")
                .push("resumed");
        });
        tokio::task::yield_now().await;
        assert!(
            events
                .lock()
                .expect("lifecycle events are readable")
                .is_empty()
        );

        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let suspended_order = Arc::clone(&order);
        let suspended_events = Arc::clone(&events);
        let suspended = tokio::spawn(async move {
            let _turn = suspended_order
                .wait(suspended_sequence)
                .await
                .expect("suspend owns the first turn");
            suspended_events
                .lock()
                .expect("lifecycle events are readable")
                .push("suspend-start");
            let _ = entered.send(());
            let _ = release_rx.await;
            suspended_events
                .lock()
                .expect("lifecycle events are readable")
                .push("suspended");
        });
        entered_rx.await.expect("slow suspend starts");
        assert_eq!(
            *events.lock().expect("lifecycle events are readable"),
            ["suspend-start"]
        );
        let _ = release.send(());
        suspended.await.expect("suspend task completes");
        resumed.await.expect("resume task completes");
        assert_eq!(
            *events.lock().expect("lifecycle events are readable"),
            ["suspend-start", "suspended", "resumed"]
        );
    }

    #[tokio::test]
    async fn dispatched_lifecycle_revisions_follow_source_order() {
        let directory = TempDir::new().expect("temporary directory");
        let runtime = Arc::new(
            InstalledRuntime::open(directory.path().join("app-data"))
                .await
                .expect("runtime opens"),
        );
        let events = Arc::new(StdMutex::new(Vec::new()));
        let suspended_events = Arc::clone(&events);
        let suspended = Arc::clone(&runtime).dispatch_lifecycle(
            sparrow_core::LifecycleSignal::Suspended,
            move |event| {
                suspended_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );
        let resumed_events = Arc::clone(&events);
        let resumed = Arc::clone(&runtime).dispatch_lifecycle(
            sparrow_core::LifecycleSignal::Resumed,
            move |event| {
                resumed_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );

        let resumed_task = tokio::spawn(resumed);
        tokio::task::yield_now().await;
        assert!(
            events
                .lock()
                .expect("lifecycle events are readable")
                .is_empty()
        );
        let suspended_task = tokio::spawn(suspended);
        suspended_task
            .await
            .expect("suspend task joins")
            .expect("suspend dispatches");
        resumed_task
            .await
            .expect("resume task joins")
            .expect("resume dispatches");

        let events = events.lock().expect("lifecycle events are readable");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].revision, 1);
        assert_eq!(events[0].state, "suspended");
        assert_eq!(events[1].revision, 2);
        assert_eq!(events[1].state, "resumed");
    }

    #[tokio::test]
    async fn opens_an_unconfigured_local_composition_and_holds_the_instance_lock() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = InstalledRuntime::open(app_data.clone())
            .await
            .expect("runtime opens");
        assert!(!runtime.core().status().configuration().is_configured());
        assert_eq!(
            fs::metadata(&app_data)
                .expect("app-data metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            InstalledRuntime::open(app_data)
                .await
                .err()
                .expect("second instance is excluded"),
            InstalledStartupError::AlreadyRunning
        );
        drop(runtime);
    }

    #[tokio::test]
    async fn runtime_slot_waits_for_and_then_reuses_the_exact_startup_runtime() {
        let slot = InstalledRuntimeSlot::new();
        assert!(slot.ready().is_none());

        let first_waiting = slot.wait();
        let second_waiting = slot.wait();
        tokio::pin!(first_waiting);
        tokio::pin!(second_waiting);
        let first_completed_before_fill = tokio::select! {
            biased;
            runtime = &mut first_waiting => Some(runtime),
            () = async {} => None,
        };
        let second_completed_before_fill = tokio::select! {
            biased;
            runtime = &mut second_waiting => Some(runtime),
            () = async {} => None,
        };
        assert!(first_completed_before_fill.is_none());
        assert!(second_completed_before_fill.is_none());

        let directory = TempDir::new().expect("temporary directory");
        let runtime = Arc::new(
            InstalledRuntime::open(directory.path().join("app-data"))
                .await
                .expect("runtime opens"),
        );
        slot.fill(Arc::clone(&runtime))
            .expect("the startup runtime fills the slot once");

        let first_released =
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut first_waiting)
                .await
                .expect("the first pending command is released");
        let second_released =
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut second_waiting)
                .await
                .expect("the second pending command is released");
        assert!(Arc::ptr_eq(&first_released, &runtime));
        assert!(Arc::ptr_eq(&second_released, &runtime));
        assert!(
            Arc::ptr_eq(&slot.wait().await, &runtime),
            "commands arriving after startup reuse the same runtime"
        );
        assert_eq!(
            slot.fill(Arc::clone(&runtime)),
            Err(InstalledRuntimeAlreadyReady),
        );
    }

    #[test]
    fn search_cancellation_registry_handles_reordering_and_bounds_tombstones() {
        let registry = SearchCancellationRegistry::default();
        let cancelled_before_registration = search_request_id(1);
        registry.cancel(cancelled_before_registration.clone());
        let registration = registry
            .register(cancelled_before_registration)
            .expect("the matching search claims its cancellation");
        assert!(registration.is_cancelled());
        drop(registration);

        let active_request = search_request_id(2);
        let registration = registry
            .register(active_request.clone())
            .expect("the active search registers");
        registry.cancel(active_request);
        assert!(registration.is_cancelled());
        drop(registration);

        for sequence in 0..(MAX_TRACKED_SEARCH_REQUESTS + 8) {
            registry.cancel(search_request_id(sequence + 100));
        }
        let state = registry
            .state
            .lock()
            .expect("search cancellation state is readable");
        assert_eq!(state.entries.len(), MAX_TRACKED_SEARCH_REQUESTS);
        assert!(state.entries.values().all(|entry| !entry.claimed));
    }

    #[tokio::test]
    async fn cancel_command_before_search_prevents_work_without_poisoning_the_next_search() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = InstalledRuntime::open(app_data)
            .await
            .expect("unconfigured runtime opens");
        let (source_location, source_server) = one_shot_m3u_server();
        runtime
            .replace_configuration(
                serde_json::from_value(json!({
                    "m3uLocation": source_location,
                    "epgLocation": null
                }))
                .expect("source input parses"),
            )
            .await
            .expect("configuration loads");
        source_server.join().expect("source server exits");

        crate::ipc::cancel_search(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_30"
            }))
            .expect("cancellation input parses"),
        )
        .expect("cancellation is accepted");
        let cancelled = crate::ipc::search(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_30",
                "term": "world",
                "channelLimit": 20,
                "programmeLimit": 20
            }))
            .expect("cancelled search input parses"),
        )
        .await;
        assert!(matches!(cancelled, Err(ClientErrorDto::ServiceUnavailable)));

        let recovered = crate::ipc::search(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_31",
                "term": "world",
                "channelLimit": 20,
                "programmeLimit": 20
            }))
            .expect("replacement search input parses"),
        )
        .await
        .expect("the next search uses the available permit");
        let recovered_json = serde_json::to_value(recovered).expect("search serializes");
        assert_eq!(
            recovered_json["channels"]["items"].as_array().map(Vec::len),
            Some(1)
        );
        assert_safe_routine_json(&recovered_json);
    }

    #[tokio::test]
    async fn replaces_persists_browses_and_reopens_from_an_offline_snapshot() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = InstalledRuntime::open(app_data.clone())
            .await
            .expect("unconfigured runtime opens");
        let (source_location, source_server) = one_shot_m3u_server();
        let input = serde_json::from_value(json!({
            "m3uLocation": source_location,
            "epgLocation": null
        }))
        .expect("source input parses");

        let status = runtime
            .replace_configuration(input)
            .await
            .expect("configuration replacement completes");
        source_server.join().expect("source server exits");
        let status_json = serde_json::to_value(status).expect("status serializes");
        assert_eq!(status_json["configuration"]["configured"], true);
        assert_eq!(status_json["configuration"]["epgConfigured"], false);
        assert!(status_json["generation"].is_number());
        assert_safe_routine_json(&status_json);

        let groups = crate::ipc::list_groups(
            &runtime,
            serde_json::from_value(json!({ "limit": 20 })).expect("group input parses"),
        )
        .expect("groups browse locally");
        let channels = crate::ipc::list_channels(
            &runtime,
            serde_json::from_value(json!({ "limit": 20, "group": "News" }))
                .expect("channel input parses"),
        )
        .expect("channels browse locally");
        let groups_json = serde_json::to_value(groups).expect("groups serialize");
        let channels_json = serde_json::to_value(channels).expect("channels serialize");
        assert_eq!(groups_json["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(channels_json["items"].as_array().map(Vec::len), Some(1));
        assert_safe_routine_json(&groups_json);
        assert_safe_routine_json(&channels_json);

        let channel_id = channels_json["items"][0]["id"]
            .as_str()
            .expect("channel id is present")
            .to_owned();
        let channel = crate::ipc::channel(
            &runtime,
            serde_json::from_value(json!({ "id": channel_id.clone() }))
                .expect("channel input parses"),
        )
        .expect("channel resolves locally");
        assert_safe_routine_json(&serde_json::to_value(channel).expect("channel serializes"));

        let schedule = crate::ipc::schedule(
            &runtime,
            serde_json::from_value(json!({ "id": channel_id, "limit": 20 }))
                .expect("schedule input parses"),
        )
        .expect("channel-only schedule resolves");
        let search = crate::ipc::search(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_10",
                "term": "world",
                "channelLimit": 20,
                "programmeLimit": 20
            }))
            .expect("search input parses"),
        )
        .await
        .expect("channel-only search resolves");
        let programmes = crate::ipc::search_programmes(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_11",
                "term": "world",
                "limit": 20
            }))
            .expect("Programme lane input parses"),
        )
        .await
        .expect("channel-only Programme search resolves");
        let schedule_json = serde_json::to_value(schedule).expect("schedule serializes");
        let search_json = serde_json::to_value(search).expect("search serializes");
        let programmes_json = serde_json::to_value(programmes).expect("Programme lane serializes");
        assert_eq!(schedule_json["items"].as_array().map(Vec::len), Some(0));
        assert_eq!(
            search_json["channels"]["items"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            search_json["programmes"]["items"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(programmes_json["items"].as_array().map(Vec::len), Some(0));
        assert_eq!(search_json["generation"], schedule_json["generation"]);
        assert_safe_routine_json(&schedule_json);
        assert_safe_routine_json(&search_json);
        assert_safe_routine_json(&programmes_json);

        let configuration_path = app_data
            .join(PRIVATE_DIRECTORY)
            .join("source-configuration.json");
        assert_eq!(
            fs::metadata(configuration_path)
                .expect("configuration is persisted")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(runtime);

        let offline = InstalledRuntime::open(app_data)
            .await
            .expect("offline snapshot reopens without the source server");
        let offline_status = serde_json::to_value(CatalogStatusDto::from(offline.core().status()))
            .expect("offline status serializes");
        assert_eq!(offline_status["m3u"]["_tag"], "stale");
        assert!(offline_status["m3u"]["nextAttemptAt"].is_string());
        assert_safe_routine_json(&offline_status);
        let offline_channels = crate::ipc::list_channels(
            &offline,
            serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses"),
        )
        .expect("offline catalog browses");
        let offline_json =
            serde_json::to_value(offline_channels).expect("offline channels serialize");
        assert_eq!(offline_json["items"].as_array().map(Vec::len), Some(1));
        assert_safe_routine_json(&offline_json);
    }

    #[tokio::test]
    async fn corrupt_or_invalid_persisted_configuration_boots_usable_and_unconfigured() {
        for bytes in [
            b"not-json".as_slice(),
            br#"{"version":1,"m3uLocation":"file:///private/source.m3u","epgLocation":null}"#,
        ] {
            let directory = TempDir::new().expect("temporary directory");
            let app_data = directory.path().join("app-data");
            let private_root = app_data.join(PRIVATE_DIRECTORY);
            prepare_app_data(&app_data).expect("app-data directory opens");
            ensure_private_directory(&private_root).expect("private directory opens");
            let configuration_path = private_root.join("source-configuration.json");
            fs::write(&configuration_path, bytes).expect("configuration fixture writes");
            fs::set_permissions(&configuration_path, fs::Permissions::from_mode(0o600))
                .expect("fixture permissions");

            let runtime = InstalledRuntime::open(app_data)
                .await
                .expect("runtime degrades safely");
            assert!(!runtime.core().status().configuration().is_configured());
        }
    }

    #[tokio::test]
    async fn manual_source_failure_retains_the_eligible_guide_and_generation() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = InstalledRuntime::open(app_data)
            .await
            .expect("unconfigured runtime opens");
        let (m3u_location, m3u_server) = two_request_source_server(
            "/channels.m3u",
            GUIDE_M3U,
            "m3u-v1",
            FixtureSecondResponse::NotModified,
        );
        let (epg_location, epg_server) = two_request_source_server(
            "/guide.xml",
            GUIDE_EPG,
            "epg-v1",
            FixtureSecondResponse::ServiceUnavailable,
        );
        let input = serde_json::from_value(json!({
            "m3uLocation": m3u_location,
            "epgLocation": epg_location
        }))
        .expect("source input parses");
        let initial_status = runtime
            .replace_configuration(input)
            .await
            .expect("configuration replacement completes");
        let initial_status_json =
            serde_json::to_value(initial_status).expect("initial status serializes");
        let generation = initial_status_json["generation"]
            .as_u64()
            .expect("initial catalog has a generation");

        let channels = crate::ipc::list_channels(
            &runtime,
            serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses"),
        )
        .expect("channels browse locally");
        let channels_json = serde_json::to_value(channels).expect("channels serialize");
        let channel_id = channels_json["items"][0]["id"]
            .as_str()
            .expect("channel id exists")
            .to_owned();
        let schedule_before = crate::ipc::schedule(
            &runtime,
            serde_json::from_value(json!({ "id": channel_id.clone(), "limit": 20 }))
                .expect("schedule input parses"),
        )
        .expect("guide schedule resolves");
        let search_before = crate::ipc::search(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_20",
                "term": "morning",
                "channelLimit": 20,
                "programmeLimit": 20
            }))
            .expect("search input parses"),
        )
        .await
        .expect("guide search resolves");
        let schedule_before_json =
            serde_json::to_value(schedule_before).expect("schedule serializes");
        let search_before_json = serde_json::to_value(search_before).expect("search serializes");
        assert_eq!(
            schedule_before_json["items"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            search_before_json["programmes"]["items"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            schedule_before_json["items"][0]["title"],
            "Morning Bulletin"
        );
        let expected_programme = json!({
            "channelId": channel_id.clone(),
            "title": "Morning Bulletin",
            "description": "Daily roundup",
            "startsAt": "2026-08-30T12:00:00Z",
            "endsAt": "2026-08-30T13:00:00Z"
        });
        let expected_search_hit = json!({
            "channel": channels_json["items"][0].clone(),
            "title": "Morning Bulletin",
            "titleTruncated": false,
            "startsAt": "2026-08-30T12:00:00Z",
            "endsAt": "2026-08-30T13:00:00Z"
        });
        assert_eq!(
            schedule_before_json,
            json!({
                "generation": generation,
                "items": [expected_programme.clone()],
                "next": null
            })
        );
        assert_eq!(
            search_before_json,
            json!({
                "generation": generation,
                "channels": {
                    "generation": generation,
                    "items": [],
                    "next": null
                },
                "programmes": {
                    "generation": generation,
                    "items": [expected_search_hit],
                    "next": null
                }
            })
        );

        let report = runtime.refresh().await;
        m3u_server.join().expect("M3U server exits");
        epg_server.join().expect("EPG server exits");
        let report_json = serde_json::to_value(crate::ipc::dto::RefreshReportDto::from(report))
            .expect("refresh report serializes");
        assert_eq!(report_json["trigger"], "manual");
        assert_eq!(report_json["m3u"]["_tag"], "not-modified");
        assert_eq!(report_json["epg"]["_tag"], "failed");
        assert_eq!(report_json["status"]["generation"], generation);
        assert!(report_json["status"]["epg"]["validatedAt"].is_string());
        assert_safe_routine_json(&report_json);

        let schedule_after = crate::ipc::schedule(
            &runtime,
            serde_json::from_value(json!({ "id": channel_id, "limit": 20 }))
                .expect("schedule input parses"),
        )
        .expect("retained guide schedule resolves");
        let search_after = crate::ipc::search_programmes(
            &runtime,
            serde_json::from_value(json!({
                "requestId": "srch1_0123456789abcdef0123456789abcdef_21",
                "term": "morning",
                "limit": 20
            }))
            .expect("Programme lane input parses"),
        )
        .await
        .expect("retained guide search resolves");
        let schedule_after_json =
            serde_json::to_value(schedule_after).expect("schedule serializes");
        let search_after_json =
            serde_json::to_value(search_after).expect("Programme lane serializes");
        assert_eq!(schedule_after_json["generation"], generation);
        assert_eq!(
            schedule_after_json["items"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(search_after_json["generation"], generation);
        assert_eq!(search_after_json["items"].as_array().map(Vec::len), Some(1));
        assert_safe_routine_json(&schedule_after_json);
        assert_safe_routine_json(&search_after_json);
    }

    #[tokio::test]
    async fn replacing_configuration_immediately_excludes_prior_catalog_and_snapshots() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = Arc::new(
            InstalledRuntime::open(app_data.clone())
                .await
                .expect("unconfigured runtime opens"),
        );
        let (initial_location, initial_server) = one_shot_m3u_server();
        runtime
            .replace_configuration(
                serde_json::from_value(json!({
                    "m3uLocation": initial_location,
                    "epgLocation": null
                }))
                .expect("initial source input parses"),
            )
            .await
            .expect("initial configuration loads");
        initial_server.join().expect("initial source server exits");
        let prior_channels = crate::ipc::list_channels(
            runtime.as_ref(),
            serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses"),
        )
        .expect("prior catalog browses");
        let prior_json = serde_json::to_value(prior_channels).expect("channels serialize");
        let prior_channel_id = prior_json["items"][0]["id"]
            .as_str()
            .expect("prior channel id exists")
            .to_owned();
        let (replacement_location, request_started, release, replacement_server) =
            blocked_failure_source_server();
        let replacement = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            async move {
                runtime
                    .replace_configuration(
                        serde_json::from_value(json!({
                            "m3uLocation": replacement_location,
                            "epgLocation": null
                        }))
                        .expect("replacement source input parses"),
                    )
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), request_started)
            .await
            .expect("replacement request starts")
            .expect("replacement server reports request");

        let replacing_status =
            serde_json::to_value(CatalogStatusDto::from(runtime.core().status()))
                .expect("replacement status serializes");
        assert_eq!(replacing_status["generation"], serde_json::Value::Null);
        assert!(matches!(
            crate::ipc::list_channels(
                runtime.as_ref(),
                serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses")
            ),
            Err(ClientErrorDto::CatalogUnavailable { .. })
        ));
        assert!(matches!(
            crate::ipc::channel(
                runtime.as_ref(),
                serde_json::from_value(json!({ "id": prior_channel_id }))
                    .expect("channel input parses")
            ),
            Err(ClientErrorDto::CatalogUnavailable { .. })
        ));
        assert_safe_routine_json(&replacing_status);

        release.store(true, Ordering::Release);
        let replaced_status = replacement
            .await
            .expect("replacement task completes")
            .expect("replacement command returns a safe status");
        replacement_server
            .join()
            .expect("replacement source server exits");
        let replaced_json =
            serde_json::to_value(replaced_status).expect("replacement status serializes");
        assert_eq!(replaced_json["generation"], serde_json::Value::Null);
        assert_eq!(replaced_json["m3u"]["_tag"], "failed");
        assert_safe_routine_json(&replaced_json);
        drop(runtime);

        let reopened = InstalledRuntime::open(app_data)
            .await
            .expect("replacement configuration reopens");
        let reopened_status =
            serde_json::to_value(CatalogStatusDto::from(reopened.core().status()))
                .expect("reopened status serializes");
        assert_eq!(reopened_status["generation"], serde_json::Value::Null);
        assert!(matches!(
            crate::ipc::list_channels(
                &reopened,
                serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses")
            ),
            Err(ClientErrorDto::CatalogUnavailable { .. })
        ));
        assert_safe_routine_json(&reopened_status);
    }

    #[tokio::test]
    async fn persisted_configuration_never_blocks_startup_on_provider_io() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let private_root = app_data.join(PRIVATE_DIRECTORY);
        prepare_app_data(&app_data).expect("app-data directory opens");
        let store = SourceConfigurationStore::open(&private_root).expect("store opens");
        let provider = TcpListener::bind("127.0.0.1:0").expect("black-hole provider binds");
        let location = format!(
            "http://{}/channels.m3u",
            provider.local_addr().expect("provider address exists")
        );
        store
            .save(&StoredSourceConfiguration::normalized(location, None))
            .expect("configuration persists");

        let runtime = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            InstalledRuntime::open(app_data),
        )
        .await
        .expect("startup never awaits the provider")
        .expect("runtime opens");
        assert!(runtime.core().status().configuration().is_configured());
    }

    #[test]
    fn shell_manifest_and_capability_expose_no_hosted_or_frontend_io_adapter() {
        let manifest = include_str!("../Cargo.toml");
        let capability = include_str!("../capabilities/installed.json");
        assert!(!manifest.contains("sparrow-server"));
        assert!(!manifest.contains("axum"));
        for forbidden in ["fs:", "http:", "shell:"] {
            assert!(!capability.contains(forbidden));
        }
    }

    #[test]
    fn startup_diagnostics_never_expose_private_context() {
        let private_canary = "https://user:secret@provider.invalid/list.m3u";
        for error in [
            InstalledStartupError::AppData,
            InstalledStartupError::AlreadyRunning,
            InstalledStartupError::Configuration,
            InstalledStartupError::SourceAdapter,
            InstalledStartupError::SnapshotAdapter,
            InstalledStartupError::PlaybackAdapter,
            InstalledStartupError::Core,
        ] {
            assert!(!format!("{error:?} {error}").contains(private_canary));
        }
    }

    fn one_shot_m3u_server() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture address exists");
        let body = b"#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" group-title=\"News\",World News\nhttp://127.0.0.1:9/live\n";
        let task = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("fixture request arrives");
            let mut request = [0_u8; 2048];
            let bytes = stream.read(&mut request).expect("fixture request reads");
            assert!(request[..bytes].starts_with(b"GET /channels.m3u HTTP/1.1\r\n"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("fixture response header writes");
            stream.write_all(body).expect("fixture body writes");
            stream.flush().expect("fixture response flushes");
        });
        (format!("http://{address}/channels.m3u"), task)
    }

    const GUIDE_M3U: &[u8] = b"#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" group-title=\"News\",World News\nhttp://127.0.0.1:9/live\n#EXTINF:-1 tvg-id=\"fixture-two\" group-title=\"Sport\",Sports Desk\nhttp://127.0.0.1:9/sport\n";
    const GUIDE_EPG: &[u8] = b"<tv><channel id=\"fixture-one\"><display-name>World News</display-name></channel><programme start=\"20260830120000 +0000\" stop=\"20260830130000 +0000\" channel=\"fixture-one\"><title>Morning Bulletin</title><desc>Daily roundup</desc></programme></tv>";

    #[derive(Clone, Copy)]
    enum FixtureSecondResponse {
        NotModified,
        ServiceUnavailable,
    }

    fn two_request_source_server(
        path: &'static str,
        body: &'static [u8],
        validator: &'static str,
        second: FixtureSecondResponse,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture address exists");
        let task = thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().expect("fixture request arrives");
                let request = read_http_request(&mut stream);
                assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
                if request_index == 0 {
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"{}\"\r\nConnection: close\r\n\r\n",
                        body.len(),
                        validator
                    )
                    .expect("fixture response header writes");
                    stream.write_all(body).expect("fixture body writes");
                } else {
                    assert!(request.to_ascii_lowercase().contains("if-none-match:"));
                    match second {
                        FixtureSecondResponse::NotModified => write!(
                            stream,
                            "HTTP/1.1 304 Not Modified\r\nETag: \"{validator}\"\r\nConnection: close\r\n\r\n"
                        )
                        .expect("not-modified response writes"),
                        FixtureSecondResponse::ServiceUnavailable => write!(
                            stream,
                            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .expect("failure response writes"),
                    }
                }
                stream.flush().expect("fixture response flushes");
            }
        });
        (format!("http://{address}{path}"), task)
    }

    fn blocked_failure_source_server() -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        Arc<AtomicBool>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture address exists");
        let (request_started, request_started_rx) = tokio::sync::oneshot::channel();
        let release = Arc::new(AtomicBool::new(false));
        let release_for_server = Arc::clone(&release);
        let task = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("fixture request arrives");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("GET /replacement.m3u HTTP/1.1\r\n"));
            request_started
                .send(())
                .expect("test receives request-started signal");
            while !release_for_server.load(Ordering::Acquire) {
                thread::yield_now();
            }
            write!(
                stream,
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("failure response writes");
            stream.flush().expect("failure response flushes");
        });
        (
            format!("http://{address}/replacement.m3u"),
            request_started_rx,
            release,
            task,
        )
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).expect("fixture request reads");
            assert!(read > 0, "fixture request contains complete headers");
            request.extend_from_slice(&chunk[..read]);
            assert!(request.len() <= 16 * 1024, "fixture request is bounded");
        }
        String::from_utf8(request).expect("fixture request headers are UTF-8")
    }

    fn assert_safe_routine_json(value: &serde_json::Value) {
        let serialized = value.to_string();
        for forbidden in ["http://", "https://", "m3uLocation", "epgLocation"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    fn search_request_id(sequence: usize) -> SearchRequestId {
        let input: crate::ipc::input::SearchCancellationInput = serde_json::from_value(json!({
            "requestId": format!("srch1_0123456789abcdef0123456789abcdef_{sequence:x}")
        }))
        .expect("cancellation input parses");
        input
            .into_request_id()
            .expect("fixture request identifier is valid")
    }
}
