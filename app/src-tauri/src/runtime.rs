use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use sparrow_bounded_blocking::{BlockingTaskCancellation, BoundedBlocking};
use sparrow_core::{
    CatalogAccess, ChannelSummary, CoreError, Page, PageRequest, ProgrammeSummary, RefreshReport,
    RefreshTrigger, SearchRequest, SearchResults, SearchTerm,
};
use sparrow_installed_catalog::{CatalogStartupError, InstalledCatalog, PreparedInstalledCatalog};
use sparrow_installed_playback::{
    InstalledPlaybackStart, LifecycleOrder, NativeStreamHandle, PlaybackManager,
    PlaybackManagerError, PlaybackRestartIntent, PlaybackSessionId, ScreenWake,
    android::{
        AndroidPlaybackControls, AndroidPlaybackIdentity, AndroidPlaybackStatus,
        AndroidPlaybackViewport, AndroidPresentationGate,
    },
};
use sparrow_source_http::HttpPlaybackAccess;
use tokio::{runtime::Handle, sync::watch};

use crate::{
    android_playback,
    ipc::{
        dto::{CatalogStatusDto, ClientErrorDto, CoreEventDto},
        input::{PlaybackEngineInput, SearchRequestId, SourceConfigurationInputDto},
        subscriptions::SubscriptionRegistry,
    },
};

#[cfg(test)]
use sparrow_installed_playback::noop_screen_wake;

const MAX_TRACKED_SEARCH_REQUESTS: usize = 64;

/// Process-lifetime rendezvous between early WebView invokes and runtime startup.
#[derive(Clone)]
pub(crate) struct InstalledRuntimeSlot {
    inner: Arc<InstalledRuntimeSlotInner>,
}

struct InstalledRuntimeSlotInner {
    runtime: OnceLock<Arc<InstalledRuntime>>,
    ready: watch::Sender<bool>,
    lifecycle_order: LifecycleOrder,
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
                lifecycle_order: LifecycleOrder::default(),
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
        if let Some(runtime) = self.ready() {
            return runtime;
        }
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

    pub(crate) fn dispatch_lifecycle<Publish>(
        &self,
        runtime: &Handle,
        signal: sparrow_core::LifecycleSignal,
        publish: Publish,
    ) -> impl std::future::Future<Output = Result<(), PlaybackManagerError>>
    + Send
    + 'static
    + use<Publish>
    where
        Publish: FnOnce(InstalledLifecycleEvent) + Send + 'static,
    {
        let slot = self.clone();
        self.inner.lifecycle_order.dispatch(runtime, async move {
            let runtime = slot.wait().await;
            if let Some(event) = runtime.apply_lifecycle(signal).await? {
                publish(event);
            }
            Ok(())
        })
    }
}

/// The installed Tauri application and its platform-specific services.
pub(crate) struct InstalledRuntime {
    #[cfg(target_os = "linux")]
    linux_playback: LinuxPlaybackEngine,
    playback: Arc<PlaybackManager>,
    searches: BoundedBlocking,
    search_cancellations: SearchCancellationRegistry,
    subscriptions: SubscriptionRegistry,
    android_presentation: AndroidPresentationGate,
    lifecycle_revision: AtomicU64,
    catalog: Arc<InstalledCatalog>,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxPlaybackEngine {
    Mpv,
    WebViewMse,
}

#[cfg(target_os = "linux")]
impl LinuxPlaybackEngine {
    fn configured() -> Result<Self, InstalledStartupError> {
        #[cfg(feature = "linux-playback-lab")]
        {
            let value = match std::env::var("SPARROW_LINUX_PLAYBACK_ENGINE") {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(InstalledStartupError::PlaybackAdapter);
                }
            };
            Self::parse(value.as_deref())
        }
        #[cfg(not(feature = "linux-playback-lab"))]
        Ok(Self::WebViewMse)
    }

    #[cfg(feature = "linux-playback-lab")]
    fn parse(value: Option<&str>) -> Result<Self, InstalledStartupError> {
        match value {
            Some("mpv") => Ok(Self::Mpv),
            None | Some("mse") => Ok(Self::WebViewMse),
            Some(_) => Err(InstalledStartupError::PlaybackAdapter),
        }
    }
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledLifecycleEvent {
    revision: u64,
    state: &'static str,
}

/// Fast synchronous startup prerequisites checked before spawning async bootstrap.
pub(crate) struct PreparedInstalledRuntime {
    #[cfg(target_os = "linux")]
    linux_playback: LinuxPlaybackEngine,
    catalog: PreparedInstalledCatalog,
}

impl InstalledRuntime {
    #[cfg(test)]
    pub(crate) async fn open(app_data: PathBuf) -> Result<Self, InstalledStartupError> {
        Self::open_with_screen_wake(app_data, noop_screen_wake()).await
    }

    pub(crate) fn prepare(
        app_data: PathBuf,
    ) -> Result<PreparedInstalledRuntime, InstalledStartupError> {
        #[cfg(target_os = "linux")]
        let linux_playback = LinuxPlaybackEngine::configured()?;
        Ok(PreparedInstalledRuntime {
            #[cfg(target_os = "linux")]
            linux_playback,
            catalog: InstalledCatalog::prepare(&app_data)?,
        })
    }

    pub(crate) async fn finish_open(
        prepared: PreparedInstalledRuntime,
        screen_wake: Arc<dyn ScreenWake>,
    ) -> Result<Self, InstalledStartupError> {
        let PreparedInstalledRuntime {
            #[cfg(target_os = "linux")]
            linux_playback,
            catalog,
        } = prepared;
        let catalog = catalog.finish_open().await?;
        let private_root = catalog.private_root().to_path_buf();
        let playback_access =
            HttpPlaybackAccess::new().map_err(|_| InstalledStartupError::PlaybackAdapter)?;
        let playback = Arc::new(PlaybackManager::new(
            catalog.core().clone(),
            playback_access,
            private_root,
            screen_wake,
        ));

        Ok(Self {
            #[cfg(target_os = "linux")]
            linux_playback,
            playback,
            searches: BoundedBlocking::serial(),
            search_cancellations: SearchCancellationRegistry::default(),
            subscriptions: SubscriptionRegistry::default(),
            android_presentation: AndroidPresentationGate::default(),
            lifecycle_revision: AtomicU64::new(0),
            catalog,
        })
    }

    #[cfg(test)]
    pub(crate) async fn open_with_screen_wake(
        app_data: PathBuf,
        screen_wake: Arc<dyn ScreenWake>,
    ) -> Result<Self, InstalledStartupError> {
        let prepared = Self::prepare(app_data)?;
        Self::finish_open(prepared, screen_wake).await
    }

    pub(crate) fn core(&self) -> &CatalogAccess {
        self.catalog.core()
    }

    pub(crate) fn status(&self) -> sparrow_core::CatalogStatus {
        self.catalog.status()
    }

    pub(crate) async fn replace_configuration(
        &self,
        input: SourceConfigurationInputDto,
    ) -> Result<CatalogStatusDto, ClientErrorDto> {
        let configuration = input.validate()?;
        self.catalog
            .replace_configuration(configuration)
            .await
            .map(CatalogStatusDto::from)
            .map_err(|_| ClientErrorDto::service_unavailable())
    }

    pub(crate) async fn refresh(&self) -> RefreshReport {
        self.core().refresh(RefreshTrigger::Manual).await
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
        Job: FnOnce(CatalogAccess, BlockingTaskCancellation) -> Result<Output, CoreError>
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

        let core = self.catalog.core().clone();
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

    pub(crate) fn subscribe(
        &self,
        events: tauri::ipc::Channel<CoreEventDto>,
    ) -> Result<String, ClientErrorDto> {
        self.subscriptions
            .subscribe(self.catalog.core().clone(), events)
    }

    pub(crate) fn unsubscribe(&self, subscription_id: &str) {
        self.subscriptions.unsubscribe(subscription_id);
    }

    pub(crate) async fn start_playback(
        &self,
        session_id: PlaybackSessionId,
        channel_id: sparrow_core::ChannelId,
        engine: Option<PlaybackEngineInput>,
    ) -> Result<InstalledPlaybackStart, PlaybackManagerError> {
        #[cfg(target_os = "linux")]
        {
            let engine = match engine {
                Some(PlaybackEngineInput::InApp) => LinuxPlaybackEngine::WebViewMse,
                Some(PlaybackEngineInput::Mpv) => LinuxPlaybackEngine::Mpv,
                None => self.linux_playback,
            };
            match engine {
                LinuxPlaybackEngine::Mpv => self
                    .playback
                    .start_mpv_primary(session_id, channel_id)
                    .await
                    .map(InstalledPlaybackStart::LinuxMpv),
                LinuxPlaybackEngine::WebViewMse => self
                    .playback
                    .start(session_id, channel_id)
                    .await
                    .map(InstalledPlaybackStart::NativeStream),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = engine;
            self.playback
                .start(session_id, channel_id)
                .await
                .map(InstalledPlaybackStart::NativeStream)
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn playback_manager(&self) -> &Arc<PlaybackManager> {
        &self.playback
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
                move || async move { playback.suspend(session_id).await },
                move || {
                    android_playback::blocking(move || android_playback::suspend_session(&stop_id))
                },
            )
            .await
    }

    pub(crate) async fn reopen_playback(
        &self,
        session_id: PlaybackSessionId,
        engine: Option<PlaybackEngineInput>,
    ) -> Result<InstalledPlaybackStart, PlaybackManagerError> {
        #[cfg(target_os = "linux")]
        {
            let engine = match engine {
                Some(PlaybackEngineInput::InApp) => LinuxPlaybackEngine::WebViewMse,
                Some(PlaybackEngineInput::Mpv) => LinuxPlaybackEngine::Mpv,
                None => self.linux_playback,
            };
            match engine {
                LinuxPlaybackEngine::Mpv => self
                    .playback
                    .reopen_mpv(session_id)
                    .await
                    .map(InstalledPlaybackStart::LinuxMpv),
                LinuxPlaybackEngine::WebViewMse => self
                    .playback
                    .reopen(session_id)
                    .await
                    .map(InstalledPlaybackStart::NativeStream),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = engine;
            self.android_presentation
                .transport_transition(|| self.playback.reopen(session_id))
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
        self.android_presentation
            .restart_generation(
                &self.playback,
                AndroidPlaybackIdentity::new(session_id, expected_stream_handle),
                intent,
                |identity| android_playback::blocking(move || android_playback::stop(&identity)),
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
                move || async move { playback.stop(session_id, stream_handle).await },
                move || {
                    android_playback::blocking(move || android_playback::stop_session(&stop_id))
                },
            )
            .await
    }

    pub(crate) async fn start_android_playback(
        &self,
        identity: AndroidPlaybackIdentity,
        viewport: AndroidPlaybackViewport,
        controls: AndroidPlaybackControls,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .start_if_active(&self.playback, identity, move |identity| {
                android_playback::blocking(move || {
                    android_playback::start(&identity, viewport, controls)
                })
            })
            .await
    }

    pub(crate) async fn android_playback_status(
        &self,
        identity: AndroidPlaybackIdentity,
    ) -> Result<AndroidPlaybackStatus, PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || {
                android_playback::blocking(move || android_playback::status(&identity))
            })
            .await
    }

    pub(crate) async fn set_android_playback_controls(
        &self,
        identity: AndroidPlaybackIdentity,
        controls: AndroidPlaybackControls,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || {
                android_playback::blocking(move || {
                    android_playback::set_controls(&identity, controls)
                })
            })
            .await
    }

    pub(crate) async fn set_android_playback_viewport(
        &self,
        identity: AndroidPlaybackIdentity,
        viewport: AndroidPlaybackViewport,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || {
                android_playback::blocking(move || {
                    android_playback::set_viewport(&identity, viewport)
                })
            })
            .await
    }

    pub(crate) async fn stop_android_playback(
        &self,
        identity: AndroidPlaybackIdentity,
    ) -> Result<(), PlaybackManagerError> {
        self.android_presentation
            .presentation_call(move || {
                android_playback::blocking(move || android_playback::stop(&identity))
            })
            .await
    }

    pub(crate) async fn set_playback_activity(
        &self,
        session_id: PlaybackSessionId,
        active: bool,
    ) -> Result<(), PlaybackManagerError> {
        self.playback.set_activity(session_id, active).await
    }

    async fn apply_lifecycle(
        &self,
        signal: sparrow_core::LifecycleSignal,
    ) -> Result<Option<InstalledLifecycleEvent>, PlaybackManagerError> {
        match signal {
            sparrow_core::LifecycleSignal::Started => {
                self.core().report_lifecycle(signal);
                Ok(None)
            }
            sparrow_core::LifecycleSignal::Suspended => {
                let playback = Arc::clone(&self.playback);
                let cleanup = self
                    .android_presentation
                    .stop_after_transport(
                        move || async move { playback.suspend_for_lifecycle().await },
                        || android_playback::blocking(android_playback::suspend_all),
                    )
                    .await;
                self.core().report_lifecycle(signal);
                cleanup?;
                self.lifecycle_event("suspended").map(Some)
            }
            sparrow_core::LifecycleSignal::Resumed => {
                self.android_presentation
                    .transport_transition(|| self.playback.resume_for_lifecycle())
                    .await?;
                self.core().report_lifecycle(signal);
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
        control: sparrow_installed_playback::MpvPlaybackControl,
    ) -> Result<(), PlaybackManagerError> {
        self.playback.control_mpv(session_id, control).await
    }

    pub(crate) async fn shutdown_playback(&self) -> Result<(), PlaybackManagerError> {
        let playback = Arc::clone(&self.playback);
        self.android_presentation
            .stop_after_transport(
                move || async move { playback.shutdown().await },
                || android_playback::blocking(android_playback::stop_all),
            )
            .await
    }
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

/// Safe startup failures deliberately discard filesystem and provider context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InstalledStartupError {
    #[error(transparent)]
    Catalog(#[from] CatalogStartupError),
    #[error("the playback adapter could not be initialized")]
    PlaybackAdapter,
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
        },
        thread,
    };

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    const PRIVATE_DIRECTORY: &str = "private-v1";

    #[cfg(all(target_os = "linux", not(feature = "linux-playback-lab")))]
    #[test]
    fn normal_linux_build_defaults_to_in_app() {
        assert_eq!(
            LinuxPlaybackEngine::configured().unwrap(),
            LinuxPlaybackEngine::WebViewMse
        );
    }

    #[cfg(all(target_os = "linux", feature = "linux-playback-lab"))]
    #[test]
    fn playback_lab_defaults_to_in_app_and_validates_overrides() {
        assert_eq!(
            LinuxPlaybackEngine::parse(None).unwrap(),
            LinuxPlaybackEngine::WebViewMse
        );
        assert_eq!(
            LinuxPlaybackEngine::parse(Some("mpv")).unwrap(),
            LinuxPlaybackEngine::Mpv
        );
        assert_eq!(
            LinuxPlaybackEngine::parse(Some("mse")).unwrap(),
            LinuxPlaybackEngine::WebViewMse
        );
        assert!(LinuxPlaybackEngine::parse(Some("unknown")).is_err());
        assert!(LinuxPlaybackEngine::parse(Some("")).is_err());
    }

    #[tokio::test]
    async fn dispatched_lifecycle_revisions_follow_source_order() {
        let directory = TempDir::new().expect("temporary directory");
        let runtime = Arc::new(
            InstalledRuntime::open(directory.path().join("app-data"))
                .await
                .expect("runtime opens"),
        );
        let slot = InstalledRuntimeSlot::new();
        slot.fill(runtime).expect("slot fills once");
        let events = Arc::new(StdMutex::new(Vec::new()));
        let suspended_events = Arc::clone(&events);
        let suspended = slot.dispatch_lifecycle(
            &Handle::current(),
            sparrow_core::LifecycleSignal::Suspended,
            move |event| {
                suspended_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );
        let resumed_events = Arc::clone(&events);
        let resumed = slot.dispatch_lifecycle(
            &Handle::current(),
            sparrow_core::LifecycleSignal::Resumed,
            move |event| {
                resumed_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );

        // Wait for the later signal first. Dispatch order follows submission.
        tokio::time::timeout(std::time::Duration::from_secs(2), resumed)
            .await
            .expect("resume completes before awaiting suspend")
            .expect("resume dispatches");
        suspended.await.expect("suspend dispatches");

        let events = events.lock().expect("lifecycle events are readable");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].revision, 1);
        assert_eq!(events[0].state, "suspended");
        assert_eq!(events[1].revision, 2);
        assert_eq!(events[1].state, "resumed");
    }

    #[tokio::test]
    async fn runtime_slot_preserves_lifecycle_order_across_startup_and_a_dropped_waiter() {
        let directory = TempDir::new().expect("temporary directory");
        let slot = InstalledRuntimeSlot::new();
        let events = Arc::new(StdMutex::new(Vec::new()));

        let first_events = Arc::clone(&events);
        let first_suspended = slot.dispatch_lifecycle(
            &Handle::current(),
            sparrow_core::LifecycleSignal::Suspended,
            move |event| {
                first_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );
        let second_events = Arc::clone(&events);
        let second_resumed = slot.dispatch_lifecycle(
            &Handle::current(),
            sparrow_core::LifecycleSignal::Resumed,
            move |event| {
                second_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );

        // The older signal was received even though nobody awaits its completion.
        drop(first_suspended);
        let second_task = tokio::spawn(second_resumed);
        tokio::task::yield_now().await;
        assert!(
            events
                .lock()
                .expect("lifecycle events are readable")
                .is_empty()
        );

        let runtime = Arc::new(
            InstalledRuntime::open(directory.path().join("app-data"))
                .await
                .expect("runtime opens"),
        );
        slot.fill(runtime).expect("slot fills once");

        // Dispatch a third signal immediately after fill; it must still wait behind pre-fill signals.
        let third_events = Arc::clone(&events);
        let third_suspended = slot.dispatch_lifecycle(
            &Handle::current(),
            sparrow_core::LifecycleSignal::Suspended,
            move |event| {
                third_events
                    .lock()
                    .expect("lifecycle events are readable")
                    .push(event);
            },
        );
        let third_task = tokio::spawn(third_suspended);

        tokio::time::timeout(std::time::Duration::from_secs(1), second_task)
            .await
            .expect("dropping the first waiter does not stall lifecycle")
            .expect("second task joins")
            .expect("second signal dispatches");
        third_task
            .await
            .expect("third task joins")
            .expect("third signal dispatches");

        let events = events.lock().expect("lifecycle events are readable");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].revision, 1);
        assert_eq!(events[0].state, "suspended");
        assert_eq!(events[1].revision, 2);
        assert_eq!(events[1].state, "resumed");
        assert_eq!(events[2].revision, 3);
        assert_eq!(events[2].state, "suspended");
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
            InstalledStartupError::Catalog(CatalogStartupError::AlreadyRunning)
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
        assert_eq!(channels_json["items"][0]["icon"], FIXTURE_ICON);

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
        let channel_json = serde_json::to_value(channel).expect("channel serializes");
        assert_eq!(channel_json["icon"], FIXTURE_ICON);
        assert_safe_routine_json(&channel_json);

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
        assert_eq!(search_json["channels"]["items"][0]["icon"], FIXTURE_ICON);
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

        let offline = reopen_after_background_work(app_data)
            .await
            .expect("offline snapshot reopens without the source server");
        let offline_status = serde_json::to_value(CatalogStatusDto::from(offline.core().status()))
            .expect("offline status serializes");
        assert_eq!(offline_status["m3u"]["_tag"], "fresh");
        assert!(offline_status["m3u"]["validatedAt"].is_string());
        assert_safe_routine_json(&offline_status);
        let offline_channels = crate::ipc::list_channels(
            &offline,
            serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses"),
        )
        .expect("offline catalog browses");
        let offline_json =
            serde_json::to_value(offline_channels).expect("offline channels serialize");
        assert_eq!(offline_json["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(offline_json["items"][0]["icon"], FIXTURE_ICON);
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
            InstalledCatalog::prepare(&app_data).expect("private directory opens");
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

        let reopened = reopen_after_background_work(app_data)
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
        InstalledCatalog::prepare(&app_data).expect("private directory opens");
        let provider = TcpListener::bind("127.0.0.1:0").expect("black-hole provider binds");
        let location = format!(
            "http://{}/channels.m3u",
            provider.local_addr().expect("provider address exists")
        );
        let path = private_root.join("source-configuration.json");
        let record = json!({"version": 1, "m3uLocation": location, "epgLocation": null});
        fs::write(
            &path,
            serde_json::to_vec(&record).expect("configuration encodes"),
        )
        .expect("configuration persists");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("configuration is private");

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
        let catalog_errors = [
            CatalogStartupError::AppData,
            CatalogStartupError::AlreadyRunning,
            CatalogStartupError::Configuration,
            CatalogStartupError::SourceAdapter,
            CatalogStartupError::SnapshotAdapter,
            CatalogStartupError::Core,
        ];
        for error in catalog_errors
            .into_iter()
            .map(InstalledStartupError::from)
            .chain([InstalledStartupError::PlaybackAdapter])
        {
            assert!(!format!("{error:?} {error}").contains(private_canary));
        }
    }

    async fn reopen_after_background_work(
        app_data: PathBuf,
    ) -> Result<InstalledRuntime, InstalledStartupError> {
        use std::time::Duration;

        // These restart tests share a process. Let any previous snapshot write release its lock.
        let prepared = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match InstalledRuntime::prepare(app_data.clone()) {
                    Err(InstalledStartupError::Catalog(CatalogStartupError::AlreadyRunning)) => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    result => break result,
                }
            }
        })
        .await
        .expect("previous catalog work releases the instance lock")?;
        InstalledRuntime::finish_open(prepared, noop_screen_wake()).await
    }

    fn one_shot_m3u_server() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture address exists");
        let body = b"#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" tvg-logo=\"https://icons.example/world-news.png\" group-title=\"News\",World News\nhttp://127.0.0.1:9/live\n";
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

    /// The Channel Icon the one-shot M3U fixture names.
    const FIXTURE_ICON: &str = "https://icons.example/world-news.png";

    /// Checks that routine replies carry no private source location. A Channel
    /// object's `icon` is the one URL a reply may carry: it must be `null` or
    /// the fixture icon, and is removed before the remainder is scanned.
    fn assert_safe_routine_json(value: &serde_json::Value) {
        fn strip_channel_icons(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(object) => {
                    if object.contains_key("id")
                        && object.contains_key("name")
                        && let Some(icon) = object.remove("icon")
                    {
                        assert!(
                            icon.is_null() || icon == FIXTURE_ICON,
                            "unexpected Channel icon"
                        );
                    }
                    object.values_mut().for_each(strip_channel_icons);
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(strip_channel_icons),
                _ => {}
            }
        }
        let mut remainder = value.clone();
        strip_channel_icons(&mut remainder);
        let serialized = remainder.to_string();
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
