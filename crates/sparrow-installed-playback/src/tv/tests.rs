use std::{collections::VecDeque, sync::Mutex};

use sparrow_core::{ChannelId, ChannelQuality};

use crate::{AudioSelection, android::AndroidPlaybackError};

use super::{
    super::{TvPlaybackAudio, TvPresentationFuture, TvQualityVariant, transport::Reply},
    *,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Call {
    Open(ChannelId),
    Reopen,
    SelectAudio,
    Suspend,
    StopTransport,
    StartPicture,
    StopPicture(TvPresentationStop),
    Lifecycle(bool),
    Wake,
    Viewport(TvPresentationKey, Option<bool>),
}

#[derive(Default)]
struct Engine {
    calls: Vec<Call>,
    serial: u64,
    transport_open: bool,
    picture_open: bool,
    retained_picture: Option<TvPresentationKey>,
    fail_start_picture: bool,
    fail_open: VecDeque<PlaybackAccessError>,
    blocked_open: Option<oneshot::Receiver<()>>,
    cancel_open: Option<oneshot::Sender<()>>,
    status_state: Option<&'static str>,
    status_unavailable: bool,
    frame_count: Option<u64>,
    fail_release: bool,
    controls_unavailable: bool,
    viewport_unavailable: bool,
}

#[derive(Default)]
struct Fixture(Arc<Mutex<Engine>>);

impl Fixture {
    fn owner(self: &Arc<Self>) -> Owner {
        Owner::new(None, self.clone(), self.clone(), 1)
    }

    fn calls(&self) -> Vec<Call> {
        self.0.lock().unwrap().calls.clone()
    }
}

impl Transport for Fixture {
    fn admit_open(self: Arc<Self>, session: PlaybackSessionId, request: Open) -> Reply<Opening> {
        Box::pin(async move {
            let mut engine = self.0.lock().unwrap();
            assert!(!engine.transport_open && !engine.picture_open);
            engine.calls.push(match request {
                Open::Start(channel) => Call::Open(channel),
                Open::Reopen => Call::Reopen,
                Open::Restart(..) => Call::SelectAudio,
            });
            let response: Opening = if let Some(blocked) = engine.blocked_open.take() {
                Box::pin(async move {
                    let _ = blocked.await;
                    Err(PlaybackManagerError::Cancelled)
                })
            } else if let Some(error) = engine.fail_open.pop_front() {
                Box::pin(async move { Err(PlaybackManagerError::Access(error)) })
            } else {
                engine.serial += 1;
                engine.transport_open = true;
                let handle =
                    NativeStreamHandle::parse(format!("stream1_{:016x}", engine.serial)).unwrap();
                Box::pin(async move {
                    Ok(Opened {
                        identity: AndroidPlaybackIdentity::new(session, handle),
                        audio: TvPlaybackAudio {
                            tracks: Vec::new(),
                            selection: AudioSelection::None,
                            preference_status: None,
                        },
                    })
                })
            };
            Ok(response)
        })
    }

    fn suspend(self: Arc<Self>, _: PlaybackSessionId, _: Option<NativeStreamHandle>) -> Reply<()> {
        Box::pin(async move {
            let mut engine = self.0.lock().unwrap();
            engine.calls.push(Call::Suspend);
            engine.transport_open = false;
            if let Some(cancel) = engine.cancel_open.take() {
                let _ = cancel.send(());
            }
            Ok(())
        })
    }

    fn stop(self: Arc<Self>, _: PlaybackSessionId, _: Option<NativeStreamHandle>) -> Reply<()> {
        Box::pin(async move {
            let mut engine = self.0.lock().unwrap();
            engine.calls.push(Call::StopTransport);
            engine.transport_open = false;
            Ok(())
        })
    }

    fn validate(self: Arc<Self>, _: PlaybackSessionId, _: NativeStreamHandle) -> Reply<()> {
        Box::pin(async move {
            assert!(self.0.lock().unwrap().transport_open);
            Ok(())
        })
    }

    fn lifecycle(self: Arc<Self>, visible: bool) -> Reply<()> {
        Box::pin(async move {
            let mut engine = self.0.lock().unwrap();
            engine.calls.push(Call::Lifecycle(visible));
            if !visible {
                engine.transport_open = false;
            }
            Ok(())
        })
    }

    fn wake(self: Arc<Self>, _: PlaybackSessionId) -> Reply<()> {
        Box::pin(async move {
            self.0.lock().unwrap().calls.push(Call::Wake);
            Ok(())
        })
    }
}

impl TvPresentation for Fixture {
    fn start(
        &self,
        _: TvPresentationKey,
        _: AndroidPlaybackViewport,
        _: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()> {
        let engine = Arc::clone(&self.0);
        Box::pin(async move {
            let mut engine = engine.lock().unwrap();
            assert!(engine.transport_open && !engine.picture_open);
            engine.calls.push(Call::StartPicture);
            if engine.fail_start_picture {
                return Err(AndroidPlaybackError);
            }
            engine.retained_picture = None;
            engine.picture_open = true;
            Ok(())
        })
    }

    fn stop(&self, key: TvPresentationKey, how: TvPresentationStop) -> TvPresentationFuture<()> {
        let engine = Arc::clone(&self.0);
        Box::pin(async move {
            let mut engine = engine.lock().unwrap();
            assert!(!engine.transport_open);
            engine.calls.push(Call::StopPicture(how));
            if engine.fail_release {
                return Err(AndroidPlaybackError);
            }
            engine.picture_open = false;
            match how {
                TvPresentationStop::RetainPicture => engine.retained_picture = Some(key),
                TvPresentationStop::ClearPicture
                    if engine.retained_picture.as_ref() == Some(&key) =>
                {
                    engine.retained_picture = None;
                }
                TvPresentationStop::ClearPicture => {}
            }
            Ok(())
        })
    }

    fn status(
        &self,
        _: TvPresentationKey,
    ) -> TvPresentationFuture<crate::android::AndroidPlaybackStatus> {
        let engine = Arc::clone(&self.0);
        Box::pin(async move {
            let engine = engine.lock().unwrap();
            if engine.status_unavailable {
                return Err(AndroidPlaybackError);
            }
            let state = engine.status_state.unwrap_or("playing");
            let frames = engine.frame_count.unwrap_or(1);
            crate::android::AndroidPlaybackStatus::parse_json(&format!(
                r#"{{"state":"{state}","decodedFrames":{frames},"droppedFrames":0,"bufferedDurationMs":1,"silent":false,"audio":"pending"}}"#
            ))
        })
    }

    fn set_controls(
        &self,
        _: TvPresentationKey,
        _: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()> {
        let engine = Arc::clone(&self.0);
        Box::pin(async move {
            if engine.lock().unwrap().controls_unavailable {
                return Err(AndroidPlaybackError);
            }
            Ok(())
        })
    }

    fn set_viewport(
        &self,
        key: TvPresentationKey,
        viewport: Option<AndroidPlaybackViewport>,
    ) -> TvPresentationFuture<()> {
        let engine = Arc::clone(&self.0);
        Box::pin(async move {
            let mut engine = engine.lock().unwrap();
            engine.calls.push(Call::Viewport(
                key,
                viewport.map(AndroidPlaybackViewport::fullscreen),
            ));
            if engine.viewport_unavailable {
                return Err(AndroidPlaybackError);
            }
            Ok(())
        })
    }
}

fn setup() -> (Arc<Fixture>, Owner) {
    let fixture = Arc::new(Fixture::default());
    let owner = fixture.owner();
    (fixture, owner)
}

fn target(number: u32) -> TvTuneTarget {
    TvTuneTarget::new(
        ChannelId::parse(format!("ch1_{number:064x}")).unwrap(),
        number,
    )
}

async fn visible(owner: &mut Owner, host: TvHostId) {
    owner.host_started(host).await.unwrap();
    owner
        .command(Command::SetViewports {
            host,
            docked: AndroidPlaybackViewport::parse(40, 24, 640, 360, false).ok(),
            fullscreen: AndroidPlaybackViewport::parse(0, 0, 1920, 1080, true).ok(),
        })
        .await;
}

async fn open(owner: &mut Owner) {
    assert!(owner.drive().await);
    let result = opened(owner.session.as_mut().unwrap().opening.as_mut()).await;
    owner.complete_open(result).await;
}

async fn tune(owner: &mut Owner, target: TvTuneTarget) -> TvPresentationKey {
    owner
        .command(Command::Tune {
            target,
            repeat: false,
        })
        .await;
    open(owner).await;
    assert!(owner.drive().await);
    let key = owner.current_picture().unwrap().clone();
    owner
        .presentation_event(&key, TvPresentationEvent::FirstFrame, Instant::now())
        .await;
    assert_eq!(owner.snapshot.phase, Phase::Playing);
    key
}

#[tokio::test]
async fn retune_releases_the_exact_picture_before_the_latest_target_and_ignores_old_callbacks() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let first = tune(&mut owner, target(1)).await;
    owner
        .command(Command::Tune {
            target: target(2),
            repeat: false,
        })
        .await;
    owner
        .command(Command::Tune {
            target: target(3),
            repeat: false,
        })
        .await;
    owner
        .presentation_event(
            &first,
            TvPresentationEvent::StreamInterrupted,
            Instant::now(),
        )
        .await;
    assert!(owner.drive().await);
    open(&mut owner).await;
    assert!(owner.drive().await);
    assert_eq!(owner.snapshot.selected, Some(target(3)));
    assert_eq!(
        &fixture.calls()[4..],
        [
            Call::StopTransport,
            Call::StopPicture(TvPresentationStop::ClearPicture),
            Call::Open(target(3).channel),
            Call::StartPicture,
        ]
    );
    owner
        .presentation_event(&first, TvPresentationEvent::Ended, Instant::now())
        .await;
    assert_eq!(owner.snapshot.phase, Phase::Starting);
}

#[tokio::test]
async fn cleanup_without_confirmation_blocks_new_provider_connections_until_a_successful_stop() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    tune(&mut owner, target(1)).await;
    fixture.0.lock().unwrap().fail_release = true;
    owner
        .command(Command::Tune {
            target: target(2),
            repeat: false,
        })
        .await;
    assert!(owner.drive().await);
    assert_eq!(
        owner.snapshot.phase,
        Phase::Failed {
            failure: Failure::CleanupUnconfirmed,
            can_restart: false
        }
    );
    owner
        .command(Command::Tune {
            target: target(3),
            repeat: false,
        })
        .await;
    assert!(!owner.drive().await);
    assert!(!fixture.calls().contains(&Call::Open(target(3).channel)));

    fixture.0.lock().unwrap().fail_release = false;
    owner.command(Command::Stop).await;
    assert!(owner.drive().await);
    assert_eq!(owner.snapshot.phase, Phase::Idle);
    assert!(!fixture.0.lock().unwrap().picture_open);
}

#[tokio::test]
async fn a_visibility_pause_reopens_the_pinned_session_but_user_pause_survives_home() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let first = tune(&mut owner, target(1)).await;
    owner.host_stopped(1, false, false).await.unwrap();
    assert_eq!(
        owner.snapshot.phase,
        Phase::Paused(TvPauseCause::Visibility)
    );
    assert!(fixture.calls().ends_with(&[
        Call::Lifecycle(false),
        Call::StopPicture(TvPresentationStop::ClearPicture)
    ]));
    visible(&mut owner, 2).await;
    open(&mut owner).await;
    assert!(owner.drive().await);
    let second = owner.current_picture().unwrap().clone();
    assert_eq!(second.identity.session_id(), first.identity.session_id());
    assert_ne!(
        second.identity.stream_handle(),
        first.identity.stream_handle()
    );
    assert_eq!(second.host, 2);
    owner
        .presentation_event(&second, TvPresentationEvent::AudioFocusLost, Instant::now())
        .await;
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
    owner.host_stopped(2, false, false).await.unwrap();
    visible(&mut owner, 3).await;
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
    assert!(!owner.drive().await);
}

#[tokio::test]
async fn an_initial_timeout_uses_one_lower_variant_before_the_finite_recovery_budget() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    fixture
        .0
        .lock()
        .unwrap()
        .fail_open
        .extend([PlaybackAccessError::TimedOut; 5]);
    let mut high = target(1);
    high.quality = Some(ChannelQuality::Fhd);
    high.lower = Some(TvQualityVariant {
        channel: target(2).channel,
        quality: ChannelQuality::Hd,
    });
    owner
        .command(Command::Tune {
            target: high,
            repeat: false,
        })
        .await;
    open(&mut owner).await;
    assert_eq!(
        owner.snapshot.quality_fallback,
        Some((ChannelQuality::Fhd, ChannelQuality::Hd))
    );
    assert_eq!(
        owner.snapshot.selected.as_ref().unwrap().channel,
        target(2).channel
    );
    assert!(
        fixture
            .calls()
            .ends_with(&[Call::Open(target(1).channel), Call::StopTransport])
    );
    for (index, seconds) in [1, 5, 15].into_iter().enumerate() {
        let before = Instant::now();
        open(&mut owner).await;
        let deadline = owner.retry_at.unwrap();
        assert!(deadline >= before + Duration::from_secs(seconds));
        assert!(deadline <= Instant::now() + Duration::from_secs(seconds));
        assert_eq!(
            owner.snapshot.phase,
            Phase::Recovering {
                attempt: index as u8 + 1,
                failure: Failure::SourceTimeout
            }
        );
        owner.on_deadline(deadline).await;
    }
    open(&mut owner).await;
    assert_eq!(
        owner.snapshot.phase,
        Phase::Failed {
            failure: Failure::SourceTimeout,
            can_restart: true
        }
    );
    assert_eq!(owner.snapshot.recovery_attempts, 3);
    assert!(owner.session.is_none());
}

#[tokio::test]
async fn held_center_does_not_fullscreen_and_zap_commits_only_the_latest_target() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let key = tune(&mut owner, target(1)).await;
    owner
        .command(Command::Zap {
            target: target(2),
            revision: owner.snapshot.zap_revision,
        })
        .await;
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: true,
        })
        .await;
    assert!(!owner.snapshot.controls.fullscreen);
    assert_eq!(owner.snapshot.pending_zap, Some(target(2)));
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await;
    assert!(owner.snapshot.controls.fullscreen);
    assert!(owner.snapshot.pending_zap.is_none());
    assert!(
        fixture
            .calls()
            .ends_with(&[Call::Viewport(key, Some(true))])
    );
    let revision = owner.snapshot.zap_revision;
    for number in [2, 3] {
        owner
            .command(Command::Zap {
                target: target(number),
                revision,
            })
            .await;
    }
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await;
    owner.command(Command::Fullscreen).await;
    assert_eq!(owner.snapshot.pending_zap, Some(target(3)));
    let deadline = owner.zap_at.unwrap();
    owner.on_deadline(deadline - Duration::from_millis(1)).await;
    assert_eq!(owner.snapshot.selected, Some(target(1)));
    owner.on_deadline(deadline).await;
    assert_eq!(owner.snapshot.selected, Some(target(3)));
    assert!(owner.snapshot.pending_zap.is_none());
    assert!(owner.drive().await);
    open(&mut owner).await;
    assert!(fixture.calls().ends_with(&[Call::Open(target(3).channel)]));
    assert!(owner.snapshot.controls.fullscreen);
}

#[tokio::test]
async fn pause_and_host_suspension_reject_zaps_from_a_cancelled_sequence() {
    for (cancel, resume) in [
        (Command::Pause, Command::Resume),
        (Command::HostStopped(1), Command::HostStarted(1)),
    ] {
        let (_, mut owner) = setup();
        visible(&mut owner, 1).await;
        tune(&mut owner, target(1)).await;
        let revision = owner.snapshot.zap_revision;
        owner
            .command(Command::Zap {
                target: target(2),
                revision,
            })
            .await;
        assert_eq!(owner.snapshot.pending_zap, Some(target(2)));

        owner.command(cancel).await;
        owner.command(resume).await;
        assert_eq!(owner.snapshot.selected, Some(target(1)));
        assert!(owner.snapshot.pending_zap.is_none());
        assert_ne!(owner.snapshot.zap_revision, revision);

        owner
            .command(Command::Zap {
                target: target(3),
                revision,
            })
            .await;
        assert!(owner.snapshot.pending_zap.is_none());
        owner
            .command(Command::Zap {
                target: target(2),
                revision: owner.snapshot.zap_revision,
            })
            .await;
        assert_eq!(owner.snapshot.pending_zap, Some(target(2)));
    }
}

#[tokio::test]
async fn a_fullscreen_pause_can_move_the_exact_retained_picture_back_to_the_dock() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let key = tune(&mut owner, target(1)).await;
    owner.command(Command::Fullscreen).await;
    owner.command(Command::Pause).await;
    owner.command(Command::ExitFullscreen).await;
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
    assert!(!owner.snapshot.controls.fullscreen);
    assert!(fixture.calls().ends_with(&[
        Call::Viewport(key.clone(), Some(true)),
        Call::Suspend,
        Call::StopPicture(TvPresentationStop::RetainPicture),
        Call::Viewport(key.clone(), Some(false)),
    ]));

    // The Activity can remove the retained picture before the next viewport request.
    fixture.0.lock().unwrap().viewport_unavailable = true;
    owner.command(Command::Fullscreen).await;
    assert!(
        fixture
            .calls()
            .ends_with(&[Call::Viewport(key, Some(true))])
    );
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
}

#[tokio::test]
async fn finishing_an_activity_clears_the_retired_picture_and_stops_a_user_paused_session() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    tune(&mut owner, target(1)).await;
    owner.command(Command::Pause).await;
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
    owner.command(Command::HostFinished(1)).await;
    assert_eq!(owner.snapshot.phase, Phase::Idle);
    assert!(owner.session.is_none());
    assert!(fixture.calls().ends_with(&[
        Call::Suspend,
        Call::StopPicture(TvPresentationStop::RetainPicture),
        Call::StopTransport,
        Call::StopPicture(TvPresentationStop::ClearPicture),
        Call::Lifecycle(false),
    ]));
}

#[tokio::test]
async fn stopping_a_pending_open_releases_it_without_starting_a_presentation() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let (cancel, blocked) = oneshot::channel();
    {
        let mut engine = fixture.0.lock().unwrap();
        engine.cancel_open = Some(cancel);
        engine.blocked_open = Some(blocked);
    }
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await;
    assert!(owner.drive().await);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !fixture.calls().contains(&Call::Open(target(1).channel)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    owner.command(Command::Stop).await;
    assert!(owner.drive().await);
    assert_eq!(owner.snapshot.phase, Phase::Idle);
    assert!(owner.session.is_none());
    assert!(
        fixture
            .calls()
            .ends_with(&[Call::Suspend, Call::StopTransport])
    );
    assert!(!fixture.calls().contains(&Call::StartPicture));
}

#[tokio::test]
async fn source_change_confirms_release_and_fences_tune_until_the_guard_drops() {
    let (fixture, owner) = setup();
    let (commands, inbox) = mpsc::channel(32);
    let (published, snapshot) = watch::channel(Snapshot::default());
    let runner = super::super::TvPlaybackRunner {
        commands: commands.clone(),
        snapshot,
    };
    let owner_task = tokio::spawn(owner.run(inbox, published));
    runner.host_visibility(1, true).await.unwrap();
    commands
        .send(Command::SetViewports {
            host: 1,
            docked: AndroidPlaybackViewport::parse(40, 24, 640, 360, false).ok(),
            fullscreen: None,
        })
        .await
        .unwrap();
    commands
        .send(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        runner
            .subscribe()
            .wait_for(|state| state.transport_token.is_some()),
    )
    .await
    .unwrap()
    .unwrap();

    let guard = runner.stop_for_source_change().await.unwrap();
    assert!(fixture.calls().ends_with(&[
        Call::StopTransport,
        Call::StopPicture(TvPresentationStop::ClearPicture)
    ]));
    commands
        .send(Command::Tune {
            target: target(2),
            repeat: false,
        })
        .await
        .unwrap();
    runner.host_visibility(1, true).await.unwrap();
    assert!(!fixture.calls().contains(&Call::Open(target(2).channel)));

    drop(guard);
    commands
        .send(Command::Tune {
            target: target(3),
            repeat: false,
        })
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        runner
            .subscribe()
            .wait_for(|state| state.transport_token.is_some() && state.selected == Some(target(3))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(fixture.calls().contains(&Call::Open(target(3).channel)));
    drop(runner);
    drop(commands);
    owner_task.await.unwrap();
}

#[tokio::test]
async fn sampled_failure_does_not_override_the_exact_presentation_event() {
    for (sample, sample_phase, event, failure) in [
        (
            "failed",
            AndroidPlaybackPhase::Failed,
            TvPresentationEvent::Ended,
            Some(Failure::Ended),
        ),
        (
            "stopped",
            AndroidPlaybackPhase::Stopped,
            TvPresentationEvent::Ended,
            Some(Failure::Ended),
        ),
        (
            "failed",
            AndroidPlaybackPhase::Failed,
            TvPresentationEvent::PlayerUnavailable,
            Some(Failure::PlayerUnavailable),
        ),
        (
            "stopped",
            AndroidPlaybackPhase::Stopped,
            TvPresentationEvent::StreamInterrupted,
            None,
        ),
    ] {
        let (fixture, mut owner) = setup();
        visible(&mut owner, 1).await;
        let high = TvTuneTarget {
            quality: Some(ChannelQuality::Fhd),
            lower: Some(TvQualityVariant {
                channel: target(2).channel,
                quality: ChannelQuality::Hd,
            }),
            ..target(1)
        };
        let key = tune(&mut owner, high.clone()).await;
        fixture.0.lock().unwrap().status_state = Some(sample);
        owner.on_deadline(owner.status_at.unwrap()).await;
        assert_eq!(owner.snapshot.phase, Phase::Playing);
        assert_eq!(owner.snapshot.status.unwrap().phase(), sample_phase);
        assert_eq!(owner.snapshot.selected, Some(high.clone()));
        assert_eq!(owner.snapshot.quality_fallback, None);

        owner
            .command(Command::PresentationEvent { key, event })
            .await;
        if let Some(failure) = failure {
            assert_eq!(
                owner.snapshot.phase,
                Phase::Failed {
                    failure,
                    can_restart: true,
                }
            );
            assert_eq!(owner.snapshot.selected, Some(high));
            assert_eq!(owner.snapshot.quality_fallback, None);
        } else {
            assert_eq!(owner.snapshot.phase, Phase::Starting);
            assert_eq!(
                owner.snapshot.selected.as_ref().unwrap().channel,
                target(2).channel
            );
            assert_eq!(
                owner.snapshot.quality_fallback,
                Some((ChannelQuality::Fhd, ChannelQuality::Hd))
            );
        }
    }
}

#[tokio::test]
async fn decoder_deadlines_use_first_frame_progress_and_reset_recovery_after_sixty_seconds() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    {
        let mut engine = fixture.0.lock().unwrap();
        engine.status_state = Some("starting");
        engine.frame_count = Some(0);
    }
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await;
    open(&mut owner).await;
    assert!(owner.drive().await);
    let startup = owner.start_by.unwrap();
    owner.on_deadline(startup - Duration::from_millis(1)).await;
    assert_eq!(owner.snapshot.phase, Phase::Starting);
    fixture.0.lock().unwrap().status_state = Some("failed");
    owner.on_deadline(startup).await;
    assert_eq!(
        owner.snapshot.phase,
        Phase::Recovering {
            attempt: 1,
            failure: Failure::StreamInterrupted
        }
    );

    owner.on_deadline(owner.retry_at.unwrap()).await;
    open(&mut owner).await;
    assert!(owner.drive().await);
    let key = owner.current_picture().unwrap().clone();
    let first = Instant::now();
    owner
        .presentation_event(&key, TvPresentationEvent::FirstFrame, first)
        .await;
    fixture.0.lock().unwrap().status_state = None;
    for elapsed in 1..60 {
        fixture.0.lock().unwrap().frame_count = Some(elapsed);
        owner
            .on_deadline(first + Duration::from_secs(elapsed))
            .await;
        assert_eq!(owner.snapshot.recovery_attempts, 1);
    }
    fixture.0.lock().unwrap().frame_count = Some(60);
    owner.on_deadline(first + Duration::from_secs(60)).await;
    assert_eq!(owner.snapshot.recovery_attempts, 0);
    owner.on_deadline(first + Duration::from_secs(69)).await;
    assert_eq!(owner.snapshot.phase, Phase::Playing);
    owner.on_deadline(first + Duration::from_secs(70)).await;
    assert_eq!(
        owner.snapshot.phase,
        Phase::Recovering {
            attempt: 1,
            failure: Failure::StreamInterrupted
        }
    );
}

#[tokio::test]
async fn visible_host_handoffs_preserve_the_pending_recovery_deadline_and_budget() {
    let (_, mut owner) = setup();
    visible(&mut owner, 1).await;
    let key = tune(&mut owner, target(1)).await;
    owner
        .presentation_event(&key, TvPresentationEvent::StreamInterrupted, Instant::now())
        .await;
    let original_deadline = owner.retry_at.unwrap();

    visible(&mut owner, 2).await;
    owner.host_stopped(2, false, false).await.unwrap();
    assert_eq!(owner.snapshot.visible_host, Some(1));
    assert_eq!(owner.retry_at, Some(original_deadline));
    assert_eq!(owner.snapshot.recovery_attempts, 1);
    owner
        .on_deadline(original_deadline - Duration::from_millis(1))
        .await;
    assert_eq!(
        owner.snapshot.phase,
        Phase::Recovering {
            attempt: 1,
            failure: Failure::StreamInterrupted,
        }
    );
    owner.on_deadline(original_deadline).await;
    assert_eq!(owner.snapshot.phase, Phase::Starting);
}

#[tokio::test]
async fn pausing_retires_the_live_status() {
    let (_, mut owner) = setup();
    visible(&mut owner, 1).await;
    tune(&mut owner, target(1)).await;
    owner.on_deadline(owner.status_at.unwrap()).await;
    assert_eq!(
        owner.snapshot.status.unwrap().phase(),
        AndroidPlaybackPhase::Playing
    );

    owner.command(Command::Pause).await;

    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
    assert!(owner.snapshot.status.is_none());
}

#[tokio::test]
async fn frame_samples_update_diagnostics_without_notifying_viewers() {
    let (fixture, mut owner) = setup();
    let (published, mut updates) = watch::channel(Snapshot::default());
    visible(&mut owner, 1).await;
    tune(&mut owner, target(1)).await;
    owner.on_deadline(owner.status_at.unwrap()).await;
    owner.publish(&published);
    assert_eq!(updates.borrow_and_update().phase, Phase::Playing);

    fixture.0.lock().unwrap().frame_count = Some(2);
    owner.on_deadline(owner.status_at.unwrap()).await;
    owner.publish(&published);

    assert_eq!(updates.borrow().status.unwrap().decoded_frames(), 2);
    assert!(!updates.has_changed().unwrap());

    owner.command(Command::Pause).await;
    owner.publish(&published);
    assert!(updates.has_changed().unwrap());
    let snapshot = updates.borrow_and_update();
    assert_eq!(snapshot.phase, Phase::Paused(TvPauseCause::User));
    assert!(snapshot.status.is_none());
}

#[tokio::test]
async fn changing_the_inactive_viewport_keeps_the_current_picture_in_place() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let key = tune(&mut owner, target(1)).await;
    let before = fixture.calls().len();

    owner
        .command(Command::SetViewports {
            host: 1,
            docked: AndroidPlaybackViewport::parse(40, 24, 640, 360, false).ok(),
            fullscreen: None,
        })
        .await;

    assert_eq!(fixture.calls().len(), before);
    owner.command(Command::Fullscreen).await;
    assert_eq!(&fixture.calls()[before..], &[Call::Viewport(key, None)]);
}

#[tokio::test]
async fn missing_geometry_hides_the_exact_live_or_retained_picture() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let key = tune(&mut owner, target(1)).await;
    let before = fixture.calls().len();

    owner
        .command(Command::SetViewports {
            host: 1,
            docked: None,
            fullscreen: None,
        })
        .await;
    assert_eq!(
        &fixture.calls()[before..],
        &[Call::Viewport(key.clone(), None)]
    );
    assert_eq!(owner.snapshot.phase, Phase::Playing);

    owner.command(Command::Pause).await;
    owner
        .command(Command::SetViewports {
            host: 1,
            docked: Some(AndroidPlaybackViewport::parse(40, 24, 640, 360, false).unwrap()),
            fullscreen: None,
        })
        .await;
    assert!(
        fixture
            .calls()
            .ends_with(&[Call::Viewport(key.clone(), Some(false))])
    );
    owner.command(Command::Fullscreen).await;
    assert!(fixture.calls().ends_with(&[Call::Viewport(key, None)]));
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
}

#[tokio::test]
async fn suspension_can_cancel_an_admitted_reopen_with_no_provider_reply() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    tune(&mut owner, target(1)).await;
    owner.command(Command::Pause).await;

    let (cancel, blocked) = oneshot::channel();
    {
        let mut engine = fixture.0.lock().unwrap();
        engine.cancel_open = Some(cancel);
        engine.blocked_open = Some(blocked);
    }
    owner.command(Command::Resume).await;
    assert!(owner.drive().await);
    assert!(fixture.calls().ends_with(&[Call::Reopen]));
    tokio::time::timeout(Duration::from_secs(1), owner.command(Command::Pause))
        .await
        .unwrap();

    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
    assert!(fixture.calls().ends_with(&[Call::Reopen, Call::Suspend]));
    assert!(!fixture.0.lock().unwrap().transport_open);
}

#[tokio::test]
async fn start_rejection_clears_the_previous_retained_picture() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let old = tune(&mut owner, target(1)).await;
    owner.command(Command::Pause).await;
    {
        let mut engine = fixture.0.lock().unwrap();
        assert_eq!(engine.retained_picture, Some(old));
        engine.fail_start_picture = true;
    }

    owner.command(Command::Resume).await;
    open(&mut owner).await;
    assert!(owner.drive().await);

    assert_eq!(
        owner.snapshot.phase,
        Phase::Recovering {
            attempt: 1,
            failure: Failure::PresentationUnavailable,
        }
    );
    assert!(fixture.0.lock().unwrap().retained_picture.is_none());
}

#[tokio::test]
async fn layout_loss_and_restore_do_not_arm_a_decoder_deadline_during_provider_open() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    let (cancel, blocked) = oneshot::channel();
    {
        let mut engine = fixture.0.lock().unwrap();
        engine.cancel_open = Some(cancel);
        engine.blocked_open = Some(blocked);
    }
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await;
    assert!(owner.drive().await);
    owner
        .command(Command::SetViewports {
            host: 1,
            docked: None,
            fullscreen: None,
        })
        .await;
    assert!(!owner.drive().await);
    visible(&mut owner, 1).await;
    assert!(!owner.drive().await);

    owner.on_deadline(Instant::now() + START_TIMEOUT).await;
    assert_eq!(owner.snapshot.phase, Phase::Starting);
    assert_eq!(owner.next_deadline(), None);
    assert!(fixture.calls().ends_with(&[Call::Open(target(1).channel)]));

    owner.command(Command::Pause).await;
    assert_eq!(owner.snapshot.phase, Phase::Paused(TvPauseCause::User));
}

#[tokio::test]
async fn sampled_pause_resets_start_and_stall_watchdogs_until_rendering_resumes() {
    let (fixture, mut owner) = setup();
    visible(&mut owner, 1).await;
    owner
        .command(Command::Tune {
            target: target(1),
            repeat: false,
        })
        .await;
    open(&mut owner).await;
    assert!(owner.drive().await);

    fixture.0.lock().unwrap().status_state = Some("paused");
    let startup = owner.start_by.unwrap();
    owner.on_deadline(startup).await;
    assert_eq!(owner.snapshot.phase, Phase::Starting);
    owner.on_deadline(startup + START_TIMEOUT).await;
    assert_eq!(owner.snapshot.phase, Phase::Starting);

    fixture.0.lock().unwrap().status_state = Some("playing");
    let first = startup + START_TIMEOUT + STATUS_INTERVAL;
    owner.on_deadline(first).await;
    assert_eq!(owner.snapshot.phase, Phase::Playing);

    fixture.0.lock().unwrap().status_state = Some("paused");
    let resumed = first + STALL_TIMEOUT;
    owner.on_deadline(resumed).await;
    assert_eq!(owner.snapshot.phase, Phase::Playing);

    fixture.0.lock().unwrap().status_state = Some("playing");
    owner
        .on_deadline(resumed + STALL_TIMEOUT - STATUS_INTERVAL)
        .await;
    assert_eq!(owner.snapshot.phase, Phase::Playing);
    owner.on_deadline(resumed + STALL_TIMEOUT).await;
    assert!(matches!(owner.snapshot.phase, Phase::Recovering { .. }));
}

#[tokio::test]
async fn generic_presentation_ack_errors_use_the_recovery_budget_without_downgrading() {
    for operation in ["start", "status", "controls", "viewport"] {
        let (fixture, mut owner) = setup();
        visible(&mut owner, 1).await;
        let high = TvTuneTarget {
            quality: Some(ChannelQuality::Fhd),
            lower: Some(TvQualityVariant {
                channel: target(2).channel,
                quality: ChannelQuality::Hd,
            }),
            ..target(1)
        };
        if operation == "start" {
            fixture.0.lock().unwrap().fail_start_picture = true;
            owner
                .command(Command::Tune {
                    target: high.clone(),
                    repeat: false,
                })
                .await;
            open(&mut owner).await;
            assert!(owner.drive().await);
        } else {
            tune(&mut owner, high.clone()).await;
            match operation {
                "status" => {
                    fixture.0.lock().unwrap().status_unavailable = true;
                    owner.on_deadline(owner.status_at.unwrap()).await;
                }
                "controls" => {
                    fixture.0.lock().unwrap().controls_unavailable = true;
                    owner.command(Command::ToggleMute).await;
                }
                "viewport" => {
                    fixture.0.lock().unwrap().viewport_unavailable = true;
                    owner.command(Command::Fullscreen).await;
                }
                _ => unreachable!(),
            }
        }

        assert_eq!(owner.snapshot.selected, Some(high));
        assert_eq!(owner.snapshot.quality_fallback, None);
        assert_eq!(owner.snapshot.recovery_attempts, 1, "{operation}");
        assert_eq!(
            owner.snapshot.phase,
            Phase::Recovering {
                attempt: 1,
                failure: Failure::PresentationUnavailable,
            },
            "{operation}"
        );
        assert!(!fixture.0.lock().unwrap().transport_open);
    }
}
