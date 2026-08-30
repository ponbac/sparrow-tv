mod support;

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream;
use sparrow_core::{
    ChannelQuery, Clock, CoreAdapters, CoreError, CoreEvent, LifecycleSignal, PageLimit,
    PageRequest, PrivateSourceValidators, RefreshOutcome, RefreshSkipReason, RefreshTrigger,
    SafeFailure, ScheduleQuery, SourceAccess, SourceAccessError, SourceAccessFailure,
    SourceByteStream, SourceConfiguration, SourceConfigurationInput, SourceKind, SourceRequest,
    SourceResponse, SourceState, SparrowCore,
};
use tokio::sync::{Semaphore, watch};

use support::MemorySnapshotStore;

const INITIAL_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="alpha" group-title="News",Alpha
https://media.fixture.invalid/alpha
"#;
const UPDATED_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="beta" group-title="News",Beta
https://media.fixture.invalid/beta
"#;
const EXPANDED_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="alpha" group-title="News",Alpha
https://media.fixture.invalid/alpha
#EXTINF:-1 tvg-id="beta" group-title="News",Beta
https://media.fixture.invalid/beta
"#;
const INITIAL_EPG: &[u8] = br#"<tv>
<channel id="alpha"><display-name>Alpha</display-name></channel>
<programme start="20260829120000 +0000" stop="20260829130000 +0000" channel="alpha"><title>Old News</title></programme>
</tv>"#;
const UPDATED_EPG: &[u8] = br#"<tv>
<channel id="beta"><display-name>Beta</display-name></channel>
<programme start="20260829130000 +0000" stop="20260829140000 +0000" channel="beta"><title>New News</title></programme>
</tv>"#;

#[derive(Clone)]
struct ControlledClock {
    state: Arc<ClockState>,
}

struct ClockState {
    now: Mutex<DateTime<Utc>>,
    changed: watch::Sender<u64>,
}

impl ControlledClock {
    fn at(value: &str) -> Self {
        Self {
            state: Arc::new(ClockState {
                now: Mutex::new(utc(value)),
                changed: watch::channel(0).0,
            }),
        }
    }

    fn set(&self, value: &str) {
        *self.state.now.lock().expect("test clock poisoned") = utc(value);
        self.state
            .changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

#[async_trait]
impl Clock for ControlledClock {
    fn now(&self) -> DateTime<Utc> {
        *self.state.now.lock().expect("test clock poisoned")
    }

    async fn wait_until(&self, deadline: DateTime<Utc>) {
        let mut changed = self.state.changed.subscribe();
        loop {
            if self.now() >= deadline {
                return;
            }
            changed
                .changed()
                .await
                .expect("controlled clock remains alive while it is awaited");
        }
    }
}

struct NeverClock {
    now: DateTime<Utc>,
    dropped: Arc<AtomicBool>,
}

impl Drop for NeverClock {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[async_trait]
impl Clock for NeverClock {
    fn now(&self) -> DateTime<Utc> {
        self.now
    }

    async fn wait_until(&self, _deadline: DateTime<Utc>) {
        std::future::pending().await
    }
}

#[derive(Clone, Default)]
struct RefreshSource {
    state: Arc<Mutex<RefreshSourceState>>,
}

#[derive(Default)]
struct RefreshSourceState {
    actions: HashMap<SourceKind, VecDeque<SourceAction>>,
    opens: HashMap<SourceKind, usize>,
    in_flight: HashMap<SourceKind, usize>,
    max_in_flight: HashMap<SourceKind, usize>,
    requests: HashMap<SourceKind, Vec<PrivateSourceValidators>>,
}

enum SourceAction {
    Modified {
        bytes: Bytes,
        validators: PrivateSourceValidators,
        gate: Option<Arc<Semaphore>>,
    },
    NotModified(PrivateSourceValidators),
    Failed(SourceAccessFailure),
}

impl RefreshSource {
    fn push_modified(&self, kind: SourceKind, bytes: &'static [u8]) {
        self.push_modified_with(kind, bytes, PrivateSourceValidators::default(), None);
    }

    fn push_modified_with(
        &self,
        kind: SourceKind,
        bytes: &'static [u8],
        validators: PrivateSourceValidators,
        gate: Option<Arc<Semaphore>>,
    ) {
        self.push(
            kind,
            SourceAction::Modified {
                bytes: Bytes::from_static(bytes),
                validators,
                gate,
            },
        );
    }

    fn push(&self, kind: SourceKind, action: SourceAction) {
        self.state
            .lock()
            .expect("refresh source poisoned")
            .actions
            .entry(kind)
            .or_default()
            .push_back(action);
    }

    fn opens(&self, kind: SourceKind) -> usize {
        self.state
            .lock()
            .expect("refresh source poisoned")
            .opens
            .get(&kind)
            .copied()
            .unwrap_or_default()
    }

    fn max_in_flight(&self, kind: SourceKind) -> usize {
        self.state
            .lock()
            .expect("refresh source poisoned")
            .max_in_flight
            .get(&kind)
            .copied()
            .unwrap_or_default()
    }

    fn request(&self, kind: SourceKind, index: usize) -> PrivateSourceValidators {
        self.state.lock().expect("refresh source poisoned").requests[&kind][index].clone()
    }
}

#[async_trait]
impl SourceAccess for RefreshSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        let kind = request.kind();
        let action = {
            let mut state = self.state.lock().expect("refresh source poisoned");
            *state.opens.entry(kind).or_default() += 1;
            state
                .requests
                .entry(kind)
                .or_default()
                .push(request.validators().clone());
            let active = state.in_flight.entry(kind).or_default();
            *active += 1;
            let active = *active;
            state
                .max_in_flight
                .entry(kind)
                .and_modify(|maximum| *maximum = (*maximum).max(active))
                .or_insert(active);
            state
                .actions
                .entry(kind)
                .or_default()
                .pop_front()
                .unwrap_or_else(|| {
                    SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable))
                })
        };

        let result = match action {
            SourceAction::Modified {
                bytes,
                validators,
                gate,
            } => {
                if let Some(gate) = gate {
                    let permit = gate.acquire().await.expect("test gate remains open");
                    permit.forget();
                }
                let length = bytes.len() as u64;
                let body: SourceByteStream = Box::pin(stream::once(async move { Ok(bytes) }));
                Ok(SourceResponse::modified(Some(length), body, validators))
            }
            SourceAction::NotModified(validators) => Ok(SourceResponse::not_modified(validators)),
            SourceAction::Failed(failure) => Err(failure),
        };

        let mut state = self.state.lock().expect("refresh source poisoned");
        *state
            .in_flight
            .get_mut(&kind)
            .expect("in-flight source request exists") -= 1;
        result
    }
}

#[tokio::test]
async fn bootstrap_failure_is_failed_with_backoff_and_resume_does_not_reopen() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );

    let core = bootstrap(&source, &snapshots, &clock, false).await;
    let next_attempt_at = clock.now() + chrono::Duration::minutes(1);
    assert!(matches!(
        core.status().m3u(),
        SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::SourceAccess {
                kind: SourceKind::M3u,
                reason: SourceAccessError::Unavailable,
                ..
            },
            next_attempt_at: actual,
        } if *actual == next_attempt_at
    ));
    assert_eq!(source.opens(SourceKind::M3u), 1);

    let report = core.refresh(RefreshTrigger::Resume).await;
    assert!(matches!(
        report.m3u(),
        RefreshOutcome::Skipped {
            reason: RefreshSkipReason::Backoff,
            next_attempt_at: actual,
        } if *actual == next_attempt_at
    ));
    assert_eq!(source.opens(SourceKind::M3u), 1);
}

#[tokio::test]
async fn maximum_retry_after_saturates_to_the_maximum_utc_instant() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::with_retry_after(
            SourceAccessError::Unavailable,
            Duration::MAX,
        )),
    );

    let report = core.refresh(RefreshTrigger::Manual).await;
    assert!(matches!(
        report.m3u(),
        RefreshOutcome::Failed {
            next_attempt_at,
            ..
        } if *next_attempt_at == DateTime::<Utc>::MAX_UTC
    ));
    assert!(matches!(
        core.status().m3u(),
        SourceState::Failed {
            next_attempt_at,
            ..
        } if *next_attempt_at == DateTime::<Utc>::MAX_UTC
    ));
}

#[tokio::test]
async fn m3u_success_and_epg_failure_publish_independently_and_retain_stale_guide() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    source.push_modified(SourceKind::Epg, INITIAL_EPG);
    let core = bootstrap(&source, &snapshots, &clock, true).await;
    let old_epg_validated_at = clock.now();

    clock.set("2026-08-29T13:00:00Z");
    source.push_modified(SourceKind::M3u, EXPANDED_M3U);
    source.push(
        SourceKind::Epg,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );
    let report = core.refresh(RefreshTrigger::Manual).await;

    assert!(matches!(report.m3u(), RefreshOutcome::Updated { .. }));
    assert!(matches!(
        report.epg(),
        Some(RefreshOutcome::Failed {
            failure: SafeFailure::SourceAccess {
                kind: SourceKind::Epg,
                ..
            },
            ..
        })
    ));
    assert!(matches!(core.status().m3u(), SourceState::Fresh { .. }));
    assert!(matches!(
        core.status().epg(),
        Some(SourceState::Failed {
            validated_at: Some(validated_at),
            ..
        }) if *validated_at == old_epg_validated_at
    ));
    assert_eq!(snapshots.activation_count(), 3);

    let channels = core
        .list_channels(ChannelQuery::all(PageRequest::first(limit())))
        .expect("the independently published M3U remains queryable");
    assert_eq!(
        channels
            .items()
            .iter()
            .map(|channel| channel.name())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    let alpha = channels
        .items()
        .iter()
        .find(|channel| channel.name() == "Alpha")
        .expect("the retained EPG still matches Alpha");
    let schedule = core
        .schedule(ScheduleQuery::new(
            alpha.id().clone(),
            PageRequest::first(limit()),
        ))
        .expect("the stale EPG contribution remains usable");
    assert_eq!(schedule.items()[0].title(), "Old News");
}

#[tokio::test]
async fn aborting_the_initiating_manual_caller_does_not_cancel_publication() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    let gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        UPDATED_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&gate)),
    );
    let caller = tokio::spawn({
        let core = core.clone();
        async move { core.refresh(RefreshTrigger::Manual).await }
    });
    wait_for(|| source.opens(SourceKind::M3u) == 2).await;

    caller.abort();
    assert!(
        caller
            .await
            .expect_err("the initiating caller is aborted")
            .is_cancelled()
    );
    gate.add_permits(1);
    wait_for(|| snapshots.activation_count() == 2).await;
    wait_for(|| {
        core.list_channels(ChannelQuery::all(PageRequest::first(limit())))
            .is_ok_and(|page| page.items()[0].name() == "Beta")
    })
    .await;
    assert!(matches!(core.status().m3u(), SourceState::Fresh { .. }));
}

#[tokio::test]
async fn dropping_the_last_core_cancels_a_never_resolving_scheduler_wait() {
    let source = RefreshSource::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let snapshots = MemorySnapshotStore::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let clock = Arc::new(NeverClock {
        now: utc("2026-08-29T12:00:00Z"),
        dropped: Arc::clone(&dropped),
    });
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/channels.m3u",
        None::<String>,
    ))
    .expect("fixture configuration is valid");
    let clock_adapter: Arc<dyn Clock> = clock.clone();
    let core = SparrowCore::bootstrap(
        Some(configuration),
        CoreAdapters::new(Arc::new(source), Arc::new(snapshots), clock_adapter),
    )
    .await
    .expect("core bootstraps");
    let mut events = core.subscribe();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                events.recv().await,
                Some(CoreEvent::RefreshCompleted {
                    kind: SourceKind::M3u,
                    ..
                })
            ) {
                break;
            }
        }
    })
    .await
    .expect("startup refresh completes before shutdown");

    drop(events);
    drop(core);
    drop(clock);
    wait_for(|| dropped.load(Ordering::Acquire)).await;
}

#[tokio::test]
async fn not_modified_uses_private_validators_without_reparse_or_activation() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let initial = validators(Some("etag-one"), Some("last-one"));
    source.push_modified_with(SourceKind::M3u, INITIAL_M3U, initial.clone(), None);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    assert_eq!(snapshots.activation_count(), 1);

    clock.set("2026-08-29T13:00:00Z");
    source.push(
        SourceKind::M3u,
        SourceAction::NotModified(validators(Some("etag-two"), None)),
    );
    let report = core.refresh(RefreshTrigger::Manual).await;

    assert!(
        matches!(report.m3u(), RefreshOutcome::NotModified { validated_at } if *validated_at == clock.now())
    );
    assert_eq!(snapshots.activation_count(), 1);
    assert_eq!(source.request(SourceKind::M3u, 1), initial);
    assert_eq!(
        snapshots.active_validators(SourceKind::M3u),
        validators(Some("etag-two"), Some("last-one"))
    );
    assert!(
        matches!(core.status().m3u(), SourceState::Fresh { validated_at } if *validated_at == clock.now())
    );
}

#[tokio::test]
async fn concurrent_manual_requests_share_one_true_source_flight() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    let gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        UPDATED_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&gate)),
    );

    let barrier = Arc::new(tokio::sync::Barrier::new(17));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let core = core.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            core.refresh(RefreshTrigger::Manual).await
        }));
    }
    barrier.wait().await;
    wait_for(|| source.opens(SourceKind::M3u) == 2).await;
    tokio::task::yield_now().await;
    gate.add_permits(1);

    for task in tasks {
        let report = task.await.expect("refresh caller completes");
        assert!(matches!(report.m3u(), RefreshOutcome::Updated { .. }));
    }
    assert_eq!(source.opens(SourceKind::M3u), 2);
    assert_eq!(source.max_in_flight(SourceKind::M3u), 1);
}

#[tokio::test]
async fn failures_back_off_at_bounded_steps_and_honor_a_longer_retry_after() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;

    for expected_minutes in [1_i64, 5, 15, 60, 60] {
        source.push(
            SourceKind::M3u,
            SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
        );
        let report = core.refresh(RefreshTrigger::Manual).await;
        let expected = clock.now() + chrono::Duration::minutes(expected_minutes);
        assert!(
            matches!(report.m3u(), RefreshOutcome::Failed { next_attempt_at, .. } if *next_attempt_at == expected)
        );
    }

    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::with_retry_after(
            SourceAccessError::Unavailable,
            Duration::from_secs(12 * 60 * 60),
        )),
    );
    let report = core.refresh(RefreshTrigger::Manual).await;
    let expected = clock.now() + chrono::Duration::hours(12);
    assert!(
        matches!(report.m3u(), RefreshOutcome::Failed { next_attempt_at, .. } if *next_attempt_at == expected)
    );

    let opens = source.opens(SourceKind::M3u);
    let skipped = core.refresh(RefreshTrigger::Resume).await;
    assert!(
        matches!(skipped.m3u(), RefreshOutcome::Skipped { reason: RefreshSkipReason::Backoff, next_attempt_at } if *next_attempt_at == expected)
    );
    assert_eq!(source.opens(SourceKind::M3u), opens);
}

#[tokio::test]
async fn scheduler_retries_at_backoff_deadlines_and_resets_policy_after_success() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    let mut events = core.subscribe();
    wait_for_m3u_refresh(&mut events).await;

    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );
    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );
    source.push_modified(SourceKind::M3u, UPDATED_M3U);

    clock.set("2026-08-29T18:00:00Z");
    wait_for(|| {
        source.opens(SourceKind::M3u) == 2
            && matches!(
                core.status().m3u(),
                SourceState::Failed {
                    next_attempt_at,
                    ..
                } if *next_attempt_at == utc("2026-08-29T18:01:00Z")
            )
    })
    .await;
    clock.set("2026-08-29T18:00:59Z");
    settle_scheduler().await;
    assert_eq!(source.opens(SourceKind::M3u), 2);

    clock.set("2026-08-29T18:01:00Z");
    wait_for(|| {
        source.opens(SourceKind::M3u) == 3
            && matches!(
                core.status().m3u(),
                SourceState::Failed {
                    next_attempt_at,
                    ..
                } if *next_attempt_at == utc("2026-08-29T18:06:00Z")
            )
    })
    .await;
    clock.set("2026-08-29T18:05:59Z");
    settle_scheduler().await;
    assert_eq!(source.opens(SourceKind::M3u), 3);

    clock.set("2026-08-29T18:06:00Z");
    wait_for(|| {
        source.opens(SourceKind::M3u) == 4
            && matches!(
                core.status().m3u(),
                SourceState::Fresh { validated_at }
                    if *validated_at == utc("2026-08-29T18:06:00Z")
            )
    })
    .await;

    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );
    clock.set("2026-08-30T00:05:59Z");
    settle_scheduler().await;
    assert_eq!(source.opens(SourceKind::M3u), 4);
    clock.set("2026-08-30T00:06:00Z");
    wait_for(|| {
        source.opens(SourceKind::M3u) == 5
            && matches!(
                core.status().m3u(),
                SourceState::Failed {
                    next_attempt_at,
                    ..
                } if *next_attempt_at == utc("2026-08-30T00:07:00Z")
            )
    })
    .await;
    clock.set("2026-08-30T00:06:59Z");
    settle_scheduler().await;
    assert_eq!(source.opens(SourceKind::M3u), 5);
}

#[tokio::test]
async fn stale_recovered_snapshot_refreshes_and_publishes_automatically_on_startup() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let snapshots = MemorySnapshotStore::default();
    let seed_source = RefreshSource::default();
    seed_source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let seeded = bootstrap(&seed_source, &snapshots, &clock, false).await;
    let mut seed_events = seeded.subscribe();
    wait_for_m3u_refresh(&mut seed_events).await;
    drop(seed_events);
    drop(seeded);
    snapshots.set_active_validated_at(SourceKind::M3u, utc("2026-08-29T00:00:00Z"));

    let refresh_source = RefreshSource::default();
    refresh_source.push_modified(SourceKind::M3u, UPDATED_M3U);
    let restarted = bootstrap(&refresh_source, &snapshots, &clock, false).await;
    assert_eq!(refresh_source.opens(SourceKind::M3u), 0);
    assert!(matches!(
        restarted.status().m3u(),
        SourceState::Stale { .. }
    ));

    wait_for(|| refresh_source.opens(SourceKind::M3u) == 1).await;
    wait_for(|| snapshots.activation_count() == 2).await;
    wait_for(|| {
        restarted
            .list_channels(ChannelQuery::all(PageRequest::first(limit())))
            .is_ok_and(|page| page.items()[0].name() == "Beta")
    })
    .await;
    assert!(matches!(
        restarted.status().m3u(),
        SourceState::Fresh { .. }
    ));
}

#[tokio::test]
async fn playback_defers_automatic_work_and_manual_request_promotes_that_flight() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    source.push_modified(SourceKind::M3u, UPDATED_M3U);
    let lease = core.begin_playback_activity();

    clock.set("2026-08-29T18:00:00Z");
    wait_for(|| matches!(core.status().m3u(), SourceState::Deferred { .. })).await;
    assert_eq!(source.opens(SourceKind::M3u), 1);

    let report = core.refresh(RefreshTrigger::Manual).await;
    assert!(matches!(report.m3u(), RefreshOutcome::Updated { .. }));
    assert_eq!(source.opens(SourceKind::M3u), 2);
    drop(lease);
}

#[tokio::test]
async fn playback_and_automatic_refresh_admission_have_a_total_order() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    let gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        UPDATED_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&gate)),
    );

    clock.set("2026-08-29T18:00:00Z");
    wait_for(|| source.opens(SourceKind::M3u) == 2).await;
    let lease = core.begin_playback_activity();
    gate.add_permits(1);
    wait_for(|| matches!(core.status().m3u(), SourceState::Fresh { validated_at } if *validated_at == clock.now())).await;

    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    clock.set("2026-08-30T00:00:00Z");
    wait_for(|| matches!(core.status().m3u(), SourceState::Deferred { .. })).await;
    assert_eq!(source.opens(SourceKind::M3u), 2);
    drop(lease);
    wait_for(|| source.opens(SourceKind::M3u) == 3).await;
}

#[tokio::test]
async fn concurrent_m3u_and_epg_publications_combine_without_lost_updates() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    source.push_modified(SourceKind::Epg, INITIAL_EPG);
    let core = bootstrap(&source, &snapshots, &clock, true).await;
    let old_page = core
        .list_channels(ChannelQuery::all(PageRequest::first(limit())))
        .expect("old generation is queryable");
    assert_eq!(old_page.items()[0].name(), "Alpha");

    let m3u_gate = Arc::new(Semaphore::new(0));
    let epg_gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        UPDATED_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&m3u_gate)),
    );
    source.push_modified_with(
        SourceKind::Epg,
        UPDATED_EPG,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&epg_gate)),
    );
    let refresh = tokio::spawn({
        let core = core.clone();
        async move { core.refresh(RefreshTrigger::Manual).await }
    });
    wait_for(|| source.opens(SourceKind::M3u) == 2 && source.opens(SourceKind::Epg) == 2).await;
    m3u_gate.add_permits(1);
    epg_gate.add_permits(1);
    let report = refresh.await.expect("combined refresh completes");
    assert!(matches!(report.m3u(), RefreshOutcome::Updated { .. }));
    assert!(matches!(report.epg(), Some(RefreshOutcome::Updated { .. })));

    let page = core
        .list_channels(ChannelQuery::all(PageRequest::first(limit())))
        .expect("new generation is queryable");
    assert_eq!(page.items()[0].name(), "Beta");
    let schedule = core
        .schedule(ScheduleQuery::new(
            page.items()[0].id().clone(),
            PageRequest::first(limit()),
        ))
        .expect("new EPG contribution is combined");
    assert_eq!(schedule.items()[0].title(), "New News");
    assert_eq!(old_page.items()[0].name(), "Alpha");
}

#[tokio::test]
async fn resume_and_exact_deadline_trigger_refresh_and_events_resynchronize_after_lag() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let core = bootstrap(&source, &snapshots, &clock, false).await;
    let mut events = core.subscribe();
    let initial = events
        .recv()
        .await
        .expect("subscription starts with a current status snapshot");
    assert!(matches!(
        initial,
        CoreEvent::CatalogStatusChanged { occurred_at, .. } if occurred_at == clock.now()
    ));

    source.push_modified(SourceKind::M3u, UPDATED_M3U);
    clock.set("2026-08-29T18:00:00Z");
    wait_for(|| source.opens(SourceKind::M3u) == 2).await;
    assert_eq!(
        core.list_channels(ChannelQuery::all(PageRequest::first(limit())))
            .expect("deadline refresh publishes")
            .items()[0]
            .name(),
        "Beta"
    );

    for _ in 0..20 {
        source.push(
            SourceKind::M3u,
            SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
        );
        let _ = core.refresh(RefreshTrigger::Manual).await;
    }
    let event = events
        .recv()
        .await
        .expect("lag produces an explicit resync");
    assert!(matches!(event, CoreEvent::CatalogStatusChanged { .. }));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), events.recv())
            .await
            .is_err(),
        "resynchronization discards every event older than the synthesized current status"
    );

    let opens = source.opens(SourceKind::M3u);
    core.report_lifecycle(LifecycleSignal::Resumed);
    tokio::task::yield_now().await;
    assert_eq!(
        source.opens(SourceKind::M3u),
        opens,
        "resume honors backoff"
    );
}

#[tokio::test]
async fn refresh_is_closed_and_safe_when_no_source_is_configured() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let core = SparrowCore::bootstrap(
        None,
        CoreAdapters::new(
            Arc::new(source.clone()),
            Arc::new(MemorySnapshotStore::default()),
            Arc::new(clock),
        ),
    )
    .await
    .expect("not-configured core is usable");
    let report = core.refresh(RefreshTrigger::Manual).await;
    assert_eq!(report.m3u(), &RefreshOutcome::NotConfigured);
    assert_eq!(report.epg(), None);
    assert_eq!(source.opens(SourceKind::M3u), 0);
}

#[tokio::test]
async fn snapshot_only_bootstrap_returns_before_a_missing_catalog_fetch_completes() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        INITIAL_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&gate)),
    );

    let core = tokio::time::timeout(
        Duration::from_secs(1),
        SparrowCore::bootstrap_from_snapshots(
            Some(configuration(
                "https://offline-at-startup.fixture.invalid/channels.m3u",
                None,
            )),
            CoreAdapters::new(
                Arc::new(source.clone()),
                Arc::new(snapshots),
                Arc::new(clock),
            ),
        ),
    )
    .await
    .expect("bootstrap never awaits source access")
    .expect("snapshot-only core bootstraps");

    assert!(core.status().configuration().is_configured());
    assert!(matches!(
        core.list_channels(ChannelQuery::all(PageRequest::first(limit()))),
        Err(CoreError::CatalogUnavailable { .. })
    ));
    wait_for(|| source.opens(SourceKind::M3u) == 1).await;
    gate.add_permits(1);
    wait_for(|| {
        core.list_channels(ChannelQuery::all(PageRequest::first(limit())))
            .is_ok_and(|page| page.items()[0].name() == "Alpha")
    })
    .await;
}

#[tokio::test]
async fn configuration_can_be_added_removed_and_added_again_from_an_empty_bootstrap() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let core = unconfigured(&source, &snapshots, &clock).await;
    source.push_modified(SourceKind::M3u, INITIAL_M3U);

    let status = core
        .replace_source_configuration(Some(configuration(
            "https://first-provider.fixture.invalid/channels.m3u",
            None,
        )))
        .await;
    assert!(status.configuration().is_configured());
    assert_eq!(source.opens(SourceKind::M3u), 1);
    assert_eq!(channel_names(&core), vec!["Alpha"]);

    source.push_modified(SourceKind::M3u, UPDATED_M3U);
    clock.set("2026-08-29T18:00:00Z");
    wait_for(|| source.opens(SourceKind::M3u) == 2).await;
    wait_for(|| channel_names(&core) == vec!["Beta"]).await;

    let removed = core.replace_source_configuration(None).await;
    assert!(!removed.configuration().is_configured());
    assert!(matches!(
        core.list_channels(ChannelQuery::all(PageRequest::first(limit()))),
        Err(CoreError::NotConfigured)
    ));
    clock.set("2026-08-30T00:00:00Z");
    settle_scheduler().await;
    assert_eq!(source.opens(SourceKind::M3u), 2);

    source.push_modified(SourceKind::M3u, EXPANDED_M3U);
    let replaced = core
        .replace_source_configuration(Some(configuration(
            "https://second-provider.fixture.invalid/channels.m3u",
            None,
        )))
        .await;
    assert!(replaced.configuration().is_configured());
    assert_eq!(source.opens(SourceKind::M3u), 3);
    assert_eq!(channel_names(&core), vec!["Alpha", "Beta"]);
}

#[tokio::test]
async fn failed_replacement_retains_the_new_configuration_as_unavailable() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let core = unconfigured(&source, &snapshots, &clock).await;
    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );

    let status = core
        .replace_source_configuration(Some(configuration(
            "https://unavailable-provider.fixture.invalid/channels.m3u",
            None,
        )))
        .await;

    assert!(status.configuration().is_configured());
    assert!(matches!(
        status.m3u(),
        SourceState::Failed {
            validated_at: None,
            failure: SafeFailure::SourceAccess {
                reason: SourceAccessError::Unavailable,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        core.list_channels(ChannelQuery::all(PageRequest::first(limit()))),
        Err(CoreError::CatalogUnavailable { .. })
    ));
}

#[tokio::test]
async fn aborting_the_replacement_caller_does_not_cancel_the_owned_transition() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let core = unconfigured(&source, &snapshots, &clock).await;
    let gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        INITIAL_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&gate)),
    );
    let replacement = tokio::spawn({
        let core = core.clone();
        async move {
            core.replace_source_configuration(Some(configuration(
                "https://owned-transition.fixture.invalid/channels.m3u",
                None,
            )))
            .await
        }
    });
    wait_for(|| source.opens(SourceKind::M3u) == 1).await;
    replacement.abort();
    gate.add_permits(1);

    wait_for(|| {
        core.list_channels(ChannelQuery::all(PageRequest::first(limit())))
            .is_ok_and(|page| page.items()[0].name() == "Alpha")
    })
    .await;
    assert!(core.status().configuration().is_configured());
}

#[tokio::test]
async fn replacement_recovers_an_eligible_snapshot_before_a_failed_foreground_fetch() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let core = unconfigured(&source, &snapshots, &clock).await;
    let location = "https://offline-provider.fixture.invalid/channels.m3u";
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let _ = core
        .replace_source_configuration(Some(configuration(location, None)))
        .await;
    let _ = core.replace_source_configuration(None).await;
    source.push(
        SourceKind::M3u,
        SourceAction::Failed(SourceAccessFailure::new(SourceAccessError::Unavailable)),
    );

    let recovered = core
        .replace_source_configuration(Some(configuration(location, None)))
        .await;

    assert!(matches!(
        recovered.m3u(),
        SourceState::Failed {
            validated_at: Some(_),
            ..
        }
    ));
    assert_eq!(channel_names(&core), vec!["Alpha"]);
    assert_eq!(snapshots.activation_count(), 1);
    assert_eq!(source.opens(SourceKind::M3u), 2);
}

#[tokio::test]
async fn replacement_invalidates_immediately_and_fences_the_old_refresh_epoch() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let core = unconfigured(&source, &snapshots, &clock).await;
    source.push_modified(SourceKind::M3u, INITIAL_M3U);
    let _ = core
        .replace_source_configuration(Some(configuration(
            "https://old-provider.fixture.invalid/channels.m3u",
            None,
        )))
        .await;
    let mut events = core.subscribe();
    let _ = events.recv().await;

    let old_gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        UPDATED_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&old_gate)),
    );
    let old_refresh = tokio::spawn({
        let core = core.clone();
        async move { core.refresh(RefreshTrigger::Manual).await }
    });
    wait_for(|| source.opens(SourceKind::M3u) == 2).await;

    source.push_modified(SourceKind::M3u, EXPANDED_M3U);
    source.push_modified(SourceKind::Epg, INITIAL_EPG);
    let replacement = tokio::spawn({
        let core = core.clone();
        async move {
            core.replace_source_configuration(Some(configuration(
                "https://new-provider.fixture.invalid/channels.m3u",
                Some("https://new-provider.fixture.invalid/guide.xml"),
            )))
            .await
        }
    });
    wait_for(|| core.status().configuration().has_epg()).await;
    assert!(matches!(
        core.list_channels(ChannelQuery::all(PageRequest::first(limit()))),
        Err(CoreError::CatalogUnavailable { .. })
    ));
    assert!(!old_refresh.is_finished());
    assert_eq!(source.opens(SourceKind::M3u), 2);

    old_gate.add_permits(1);
    let old_report = old_refresh.await.expect("old refresh task completes");
    assert_eq!(old_report.m3u(), &RefreshOutcome::NotConfigured);
    let status = replacement.await.expect("replacement task completes");
    assert!(status.configuration().has_epg());
    assert_eq!(channel_names(&core), vec!["Alpha", "Beta"]);
    assert_eq!(source.max_in_flight(SourceKind::M3u), 1);

    let mut m3u_completions = 0;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(25), events.recv()).await
    {
        if matches!(
            event,
            CoreEvent::RefreshCompleted {
                kind: SourceKind::M3u,
                ..
            }
        ) {
            m3u_completions += 1;
        }
    }
    assert_eq!(
        m3u_completions, 1,
        "only the replacement epoch publishes refresh completion"
    );
}

#[tokio::test]
async fn concurrent_replacements_are_serialized_through_the_public_transition() {
    let clock = ControlledClock::at("2026-08-29T12:00:00Z");
    let source = RefreshSource::default();
    let snapshots = MemorySnapshotStore::default();
    let core = unconfigured(&source, &snapshots, &clock).await;
    let first_gate = Arc::new(Semaphore::new(0));
    source.push_modified_with(
        SourceKind::M3u,
        INITIAL_M3U,
        PrivateSourceValidators::default(),
        Some(Arc::clone(&first_gate)),
    );
    let first = tokio::spawn({
        let core = core.clone();
        async move {
            core.replace_source_configuration(Some(configuration(
                "https://first-provider.fixture.invalid/channels.m3u",
                None,
            )))
            .await
        }
    });
    wait_for(|| source.opens(SourceKind::M3u) == 1).await;

    source.push_modified(SourceKind::M3u, UPDATED_M3U);
    let second = tokio::spawn({
        let core = core.clone();
        async move {
            core.replace_source_configuration(Some(configuration(
                "https://second-provider.fixture.invalid/channels.m3u",
                None,
            )))
            .await
        }
    });
    settle_scheduler().await;
    assert_eq!(source.opens(SourceKind::M3u), 1);
    first_gate.add_permits(1);

    let first_status = first.await.expect("first replacement completes");
    let second_status = second.await.expect("second replacement completes");
    assert!(first_status.configuration().is_configured());
    assert!(second_status.configuration().is_configured());
    assert_eq!(source.opens(SourceKind::M3u), 2);
    assert_eq!(source.max_in_flight(SourceKind::M3u), 1);
    assert_eq!(channel_names(&core), vec!["Beta"]);
}

async fn bootstrap(
    source: &RefreshSource,
    snapshots: &MemorySnapshotStore,
    clock: &ControlledClock,
    epg: bool,
) -> SparrowCore {
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/channels.m3u",
        epg.then_some("https://provider.fixture.invalid/guide.xml"),
    ))
    .expect("fixture configuration is valid");
    SparrowCore::bootstrap(
        Some(configuration),
        CoreAdapters::new(
            Arc::new(source.clone()),
            Arc::new(snapshots.clone()),
            Arc::new(clock.clone()),
        ),
    )
    .await
    .expect("core bootstraps")
}

async fn unconfigured(
    source: &RefreshSource,
    snapshots: &MemorySnapshotStore,
    clock: &ControlledClock,
) -> SparrowCore {
    SparrowCore::bootstrap(
        None,
        CoreAdapters::new(
            Arc::new(source.clone()),
            Arc::new(snapshots.clone()),
            Arc::new(clock.clone()),
        ),
    )
    .await
    .expect("not-configured core bootstraps")
}

fn configuration(m3u: &str, epg: Option<&str>) -> SourceConfiguration {
    SparrowCore::parse_source_configuration(SourceConfigurationInput::new(m3u, epg))
        .expect("fixture Source Configuration is valid")
}

fn channel_names(core: &SparrowCore) -> Vec<String> {
    core.list_channels(ChannelQuery::all(PageRequest::first(limit())))
        .expect("configured catalog is browsable")
        .items()
        .iter()
        .map(|channel| channel.name().to_owned())
        .collect()
}

fn validators(etag: Option<&str>, last_modified: Option<&str>) -> PrivateSourceValidators {
    PrivateSourceValidators::parse(etag.map(str::to_owned), last_modified.map(str::to_owned))
        .expect("fixture validators are valid")
}

fn limit() -> PageLimit {
    PageLimit::new(10).expect("fixture limit is valid")
}

async fn wait_for(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition becomes true");
}

async fn wait_for_m3u_refresh(events: &mut sparrow_core::CoreEventStream) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                events.recv().await,
                Some(CoreEvent::RefreshCompleted {
                    kind: SourceKind::M3u,
                    ..
                })
            ) {
                return;
            }
        }
    })
    .await
    .expect("the automatic M3U refresh completes");
}

async fn settle_scheduler() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixture timestamp")
        .with_timezone(&Utc)
}
