use std::{
    collections::VecDeque,
    future::Future,
    io::{Read as _, Write as _},
    net::TcpListener,
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::Stream;
use sparrow_core::{
    ChannelId, ChannelQuery, CoreAdapters, PageLimit, PageRequest, RefreshTrigger,
    ResolvedPlaybackSource, SourceConfigurationInput, SparrowCore, SystemClock,
};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use sparrow_source_http::{
    HttpSourceAccess, PlaybackAccessError, PlaybackByteStream, PlaybackReadError,
};
use tempfile::TempDir;

use super::*;

const PRIVATE_PLAYBACK_CANARY: &str =
    "http://subscriber:secret@provider.invalid/live.ts?token=private";

#[tokio::test]
async fn pulls_are_bounded_lossless_and_distinguish_empty_chunks_from_eof() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let large = (0..(MAX_NATIVE_PULL_BYTES + 13))
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let access = FakeAccess::new([OpenPlan::Stream(vec![
        StreamStep::Chunk(Bytes::new()),
        StreamStep::Chunk(Bytes::copy_from_slice(&large)),
        StreamStep::Chunk(Bytes::new()),
        StreamStep::Chunk(Bytes::from_static(b"tail")),
    ])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(1), fixture.channel.clone())
        .await
        .expect("playback starts");

    let first = read(&manager, &started).await.expect("first pull succeeds");
    let second = read(&manager, &started)
        .await
        .expect("remainder pull succeeds");
    let third = read(&manager, &started)
        .await
        .expect("empty provider chunk is skipped");
    let eof = read(&manager, &started).await.expect("EOF is projected");
    let repeated_eof = read(&manager, &started)
        .await
        .expect("EOF remains idempotent");

    assert_eq!(first, large[..MAX_NATIVE_PULL_BYTES]);
    assert_eq!(second, large[MAX_NATIVE_PULL_BYTES..]);
    assert_eq!(third, b"tail");
    assert!(eof.is_empty());
    assert!(repeated_eof.is_empty());
    assert_eq!(access.tracker.active(), 0, "EOF releases the body");
}

#[tokio::test]
async fn header_and_body_failures_are_typed_and_release_provider_resources() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let header_access = FakeAccess::new([OpenPlan::Error(PlaybackAccessError::InvalidResponse)]);
    let header_manager =
        PlaybackManager::with_access(Arc::clone(&fixture.core), header_access.clone());
    assert!(matches!(
        header_manager
            .start(session(2), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Access(
            PlaybackAccessError::InvalidResponse
        ))
    ));
    assert_eq!(header_access.tracker.active(), 0);

    let body_access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Error(
        PlaybackReadError::Interrupted,
    )])]);
    let body_manager = PlaybackManager::with_access(Arc::clone(&fixture.core), body_access.clone());
    let started = body_manager
        .start(session(3), fixture.channel.clone())
        .await
        .expect("headers succeed");
    assert!(matches!(
        read(&body_manager, &started).await,
        Err(PlaybackManagerError::Read(PlaybackReadError::Interrupted))
    ));
    assert_eq!(body_access.tracker.active(), 0);
    assert!(matches!(
        read(&body_manager, &started).await,
        Err(PlaybackManagerError::Read(PlaybackReadError::Interrupted))
    ));
}

#[tokio::test]
async fn matching_stop_cancels_pending_headers_before_acknowledging() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::PendingHeaders]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let session_id = session(4);
    let mut start = Box::pin(manager.start(session_id.clone(), fixture.channel.clone()));

    tokio::select! {
        result = &mut start => panic!("pending headers completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.active() == 1) => {}
    }
    manager
        .stop(session_id)
        .await
        .expect("matching stop is acknowledged");

    assert!(matches!(start.await, Err(PlaybackManagerError::Cancelled)));
    assert_eq!(
        access.tracker.active(),
        0,
        "stop waits until the pending request is dropped"
    );
}

#[tokio::test]
async fn stop_before_start_is_bounded_and_does_not_poison_later_sessions() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());

    for sequence in 0..=MAX_STOP_TOMBSTONES {
        manager
            .stop(session(sequence))
            .await
            .expect("unknown stop remains idempotent");
    }

    let oldest = manager
        .start(session(0), fixture.channel.clone())
        .await
        .expect("the bounded registry evicts its oldest tombstone");
    assert!(matches!(
        manager
            .start(session(MAX_STOP_TOMBSTONES), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.open_count(), 1);
    manager
        .stop(oldest.session_id().clone())
        .await
        .expect("active playback stops");
}

#[tokio::test]
async fn matching_stop_cancels_a_pending_read_and_releases_the_body() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(70), fixture.channel.clone())
        .await
        .expect("playback starts");
    let mut pending_read = Box::pin(read(&manager, &started));

    tokio::select! {
        result = &mut pending_read => panic!("pending read completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.read_polls() > 0) => {}
    }
    manager
        .stop(started.session_id().clone())
        .await
        .expect("stop cancels the read");

    assert!(matches!(
        pending_read.await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.tracker.active(), 0);
}

#[tokio::test]
async fn replacement_and_stale_commands_never_overlap_or_retarget_streams() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"new"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let first = manager
        .start(session(80), fixture.channel.clone())
        .await
        .expect("first stream starts");
    let second = manager
        .start(session(81), fixture.channel.clone())
        .await
        .expect("replacement stream starts");

    assert_eq!(access.tracker.active(), 1);
    assert_eq!(
        access.tracker.max_active(),
        1,
        "old provider resources are dropped before replacement open"
    );
    assert!(matches!(
        read(&manager, &first).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert!(matches!(
        manager
            .start(second.session_id().clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    manager
        .stop(first.session_id().clone())
        .await
        .expect("stale stop is harmless");
    assert!(matches!(
        manager
            .start(first.session_id().clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.tracker.active(), 1);
    assert_eq!(
        read(&manager, &second).await.expect("current handle reads"),
        b"new"
    );
    manager
        .stop(second.session_id().clone())
        .await
        .expect("current stream stops");
    assert_eq!(access.tracker.active(), 0);
}

#[tokio::test]
async fn dropped_start_callers_cannot_leave_pending_or_queued_opens() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"current"))]),
        OpenPlan::PendingHeaders,
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let current = manager
        .start(session(85), fixture.channel.clone())
        .await
        .expect("current stream starts");

    let (reply, response) = oneshot::channel();
    drop(response);
    manager
        .controls
        .send(ControlCommand::Start {
            session_id: session(86),
            channel_id: fixture.channel.clone(),
            reply,
        })
        .await
        .expect("closed start command queues");
    tokio::task::yield_now().await;
    assert_eq!(access.open_count(), 1);
    assert_eq!(
        read(&manager, &current)
            .await
            .expect("current stream remains"),
        b"current"
    );

    let mut abandoned = Box::pin(manager.start(session(87), fixture.channel.clone()));
    tokio::select! {
        result = &mut abandoned => panic!("pending headers completed unexpectedly: {result:?}"),
        () = wait_until(|| access.open_count() == 2) => {}
    }
    drop(abandoned);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(access.source(1).upgrade().is_none());
}

#[tokio::test]
async fn dropping_a_pending_read_cancels_the_body_without_shifting_bytes() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(88), fixture.channel.clone())
        .await
        .expect("stream starts");
    let mut abandoned = Box::pin(read(&manager, &started));
    tokio::select! {
        result = &mut abandoned => panic!("pending read completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.read_polls() > 0) => {}
    }

    drop(abandoned);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(matches!(
        read(&manager, &started).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    manager
        .stop(started.session_id().clone())
        .await
        .expect("failed Session stops");
}

#[tokio::test]
async fn dropping_the_manager_tears_down_the_actor_and_provider_body() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    manager
        .start(session(90), fixture.channel.clone())
        .await
        .expect("playback starts");
    let pinned_source = access.source(0);
    assert_eq!(access.tracker.active(), 1);
    assert!(pinned_source.upgrade().is_some());

    drop(manager);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(pinned_source.upgrade().is_none());
}

#[tokio::test]
async fn ended_session_keeps_its_pinned_source_across_catalog_refresh() {
    let (source_location, source_server) = m3u_server([
        PRIVATE_PLAYBACK_CANARY,
        "http://replacement.invalid/new.ts?token=rotated",
    ]);
    let fixture = CoreFixture::from_source(&source_location).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"replacement"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let first = manager
        .start(session(100), fixture.channel.clone())
        .await
        .expect("first playback starts");
    assert!(
        read(&manager, &first)
            .await
            .expect("first stream reaches EOF")
            .is_empty()
    );
    let pinned_source = access.source(0);
    assert_eq!(
        pinned_source
            .upgrade()
            .expect("the ended session retains its source")
            .location_for_adapter()
            .as_str(),
        PRIVATE_PLAYBACK_CANARY
    );

    let report = fixture.core.refresh(RefreshTrigger::Manual).await;
    assert!(
        report.status().generation().is_some(),
        "catalog refresh publishes"
    );
    let current_channel = only_channel(&fixture.core);
    assert_eq!(current_channel, fixture.channel);
    assert_eq!(
        pinned_source
            .upgrade()
            .expect("refresh cannot retarget the pinned Session source")
            .location_for_adapter()
            .as_str(),
        PRIVATE_PLAYBACK_CANARY
    );

    let second = manager
        .start(session(101), current_channel)
        .await
        .expect("a new session resolves the refreshed catalog");
    assert_eq!(
        access.opened_location(1),
        "http://replacement.invalid/new.ts?token=rotated"
    );
    assert!(pinned_source.upgrade().is_none());
    manager
        .stop(second.session_id().clone())
        .await
        .expect("replacement stops");
    source_server.join().expect("source fixture exits");
}

#[tokio::test]
async fn diagnostics_never_expose_provider_or_opaque_identifier_values() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Error(PlaybackAccessError::Rejected)]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access);
    let private_session = session(0xfeed);
    let session_value = private_session.as_str().to_owned();
    let error = manager
        .start(private_session.clone(), fixture.channel.clone())
        .await
        .expect_err("provider rejects playback");
    let diagnostics = format!("{error:?} {error} {private_session:?}");

    assert!(!diagnostics.contains(PRIVATE_PLAYBACK_CANARY));
    assert!(!diagnostics.contains("subscriber"));
    assert!(!diagnostics.contains("secret"));
    assert!(!diagnostics.contains(&session_value));
}

async fn read(
    manager: &PlaybackManager,
    started: &StartedPlayback,
) -> Result<Vec<u8>, PlaybackManagerError> {
    manager
        .read(
            started.session_id().clone(),
            started.stream_handle().clone(),
        )
        .await
}

fn session(sequence: usize) -> PlaybackSessionId {
    PlaybackSessionId::parse(format!(
        "play1_0123456789abcdef0123456789abcdef_{sequence:x}"
    ))
    .expect("fixture session id is valid")
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actor reaches the expected state");
}

struct CoreFixture {
    core: Arc<SparrowCore>,
    channel: ChannelId,
    _directory: TempDir,
}

impl CoreFixture {
    async fn one(playback_location: &str) -> Self {
        let (source_location, server) = m3u_server([playback_location]);
        let fixture = Self::from_source(&source_location).await;
        server.join().expect("source fixture exits");
        fixture
    }

    async fn from_source(source_location: &str) -> Self {
        let directory = TempDir::new().expect("temporary snapshot directory");
        let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
            source_location,
            None::<String>,
        ))
        .expect("fixture Source Configuration parses");
        let source = Arc::new(HttpSourceAccess::new().expect("source adapter opens"));
        let snapshots = Arc::new(
            AtomicFileSnapshotStore::open(directory.path()).expect("snapshot store opens"),
        );
        let core = Arc::new(
            SparrowCore::bootstrap(
                Some(configuration),
                CoreAdapters::new(source, snapshots, Arc::new(SystemClock)),
            )
            .await
            .expect("configured core bootstraps"),
        );
        let channel = only_channel(&core);
        Self {
            core,
            channel,
            _directory: directory,
        }
    }
}

fn only_channel(core: &SparrowCore) -> ChannelId {
    core.list_channels(ChannelQuery::all(PageRequest::new(
        None,
        PageLimit::new(10).expect("valid page limit"),
    )))
    .expect("fixture catalog browses")
    .items()
    .first()
    .expect("fixture has one channel")
    .id()
    .clone()
}

fn m3u_server<'a>(
    playback_locations: impl IntoIterator<Item = &'a str>,
) -> (String, thread::JoinHandle<()>) {
    let playback_locations = playback_locations
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
    let address = listener.local_addr().expect("fixture address exists");
    listener
        .set_nonblocking(true)
        .expect("fixture listener becomes nonblocking");
    let task = thread::spawn(move || {
        for playback_location in playback_locations {
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "source request arrives");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("source fixture accepts: {error}"),
                }
            };
            let mut request = [0_u8; 2048];
            let bytes = stream.read(&mut request).expect("fixture request reads");
            assert!(request[..bytes].starts_with(b"GET /channels.m3u HTTP/1.1\r\n"));
            let body = format!(
                "#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" group-title=\"News\",World News\n{playback_location}\n"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("fixture response header writes");
            stream
                .write_all(body.as_bytes())
                .expect("fixture body writes");
            stream.flush().expect("fixture response flushes");
        }
    });
    (format!("http://{address}/channels.m3u"), task)
}

#[derive(Clone)]
struct FakeAccess {
    plans: Arc<Mutex<VecDeque<OpenPlan>>>,
    locations: Arc<Mutex<Vec<String>>>,
    sources: Arc<Mutex<Vec<Weak<ResolvedPlaybackSource>>>>,
    tracker: Arc<ConnectionTracker>,
}

impl FakeAccess {
    fn new(plans: impl IntoIterator<Item = OpenPlan>) -> Arc<Self> {
        Arc::new(Self {
            plans: Arc::new(Mutex::new(plans.into_iter().collect())),
            locations: Arc::new(Mutex::new(Vec::new())),
            sources: Arc::new(Mutex::new(Vec::new())),
            tracker: Arc::new(ConnectionTracker::default()),
        })
    }

    fn open_count(&self) -> usize {
        self.locations.lock().expect("locations lock").len()
    }

    fn opened_location(&self, index: usize) -> String {
        self.locations.lock().expect("locations lock")[index].clone()
    }

    fn source(&self, index: usize) -> Weak<ResolvedPlaybackSource> {
        self.sources.lock().expect("sources lock")[index].clone()
    }
}

impl NativePlaybackAccess for FakeAccess {
    fn open(&self, source: Arc<ResolvedPlaybackSource>) -> AccessOpenFuture {
        self.locations
            .lock()
            .expect("locations lock")
            .push(source.location_for_adapter().as_str().to_owned());
        self.sources
            .lock()
            .expect("sources lock")
            .push(Arc::downgrade(&source));
        let plan = self
            .plans
            .lock()
            .expect("plans lock")
            .pop_front()
            .expect("a fake open plan exists");
        let guard = self.tracker.acquire();
        match plan {
            OpenPlan::PendingHeaders => Box::pin(PendingHeaders { _guard: guard }),
            OpenPlan::Error(error) => Box::pin(async move {
                drop(guard);
                Err(error)
            }),
            OpenPlan::Stream(steps) => {
                let stream: PlaybackByteStream = Box::pin(FakeStream {
                    steps: steps.into_iter().collect(),
                    guard: Some(guard),
                });
                Box::pin(async move { Ok(stream) })
            }
        }
    }
}

enum OpenPlan {
    PendingHeaders,
    Error(PlaybackAccessError),
    Stream(Vec<StreamStep>),
}

enum StreamStep {
    Chunk(Bytes),
    Error(PlaybackReadError),
    Pending,
}

struct PendingHeaders {
    _guard: ConnectionGuard,
}

impl Future for PendingHeaders {
    type Output = Result<PlaybackByteStream, PlaybackAccessError>;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

struct FakeStream {
    steps: VecDeque<StreamStep>,
    guard: Option<ConnectionGuard>,
}

impl Stream for FakeStream {
    type Item = Result<Bytes, PlaybackReadError>;

    fn poll_next(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if matches!(self.steps.front(), Some(StreamStep::Pending)) {
            self.guard
                .as_ref()
                .expect("live stream owns a guard")
                .tracker
                .read_polls
                .fetch_add(1, Ordering::AcqRel);
            return Poll::Pending;
        }
        Poll::Ready(match self.steps.pop_front() {
            Some(StreamStep::Chunk(bytes)) => Some(Ok(bytes)),
            Some(StreamStep::Error(error)) => Some(Err(error)),
            Some(StreamStep::Pending) => unreachable!("pending step is retained"),
            None => None,
        })
    }
}

#[derive(Default)]
struct ConnectionTracker {
    active: AtomicUsize,
    max_active: AtomicUsize,
    read_polls: AtomicUsize,
}

impl ConnectionTracker {
    fn acquire(self: &Arc<Self>) -> ConnectionGuard {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        ConnectionGuard {
            tracker: Arc::clone(self),
        }
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::Acquire)
    }

    fn read_polls(&self) -> usize {
        self.read_polls.load(Ordering::Acquire)
    }
}

struct ConnectionGuard {
    tracker: Arc<ConnectionTracker>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.tracker.active.fetch_sub(1, Ordering::AcqRel);
    }
}
