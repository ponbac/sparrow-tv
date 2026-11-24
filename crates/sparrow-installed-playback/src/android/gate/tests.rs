use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::oneshot;

use crate::{NativeStreamHandle, PlaybackSessionId};

use super::*;

fn identity() -> AndroidPlaybackIdentity {
    AndroidPlaybackIdentity::new(
        PlaybackSessionId::parse("play1_0123456789abcdef0123456789abcdef_1".to_owned()).unwrap(),
        NativeStreamHandle::parse("stream1_0000000000000001".to_owned()).unwrap(),
    )
}

#[tokio::test]
async fn cancelled_presentation_wait_keeps_the_gate_until_acknowledged() {
    let gate = Arc::new(AndroidPresentationGate::default());
    let (posted, posted_rx) = oneshot::channel();
    let (acknowledge, acknowledged) = oneshot::channel();
    let caller = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move {
            gate.presentation_call(|| async move {
                posted.send(()).expect("the caller observes the post");
                acknowledged.await.expect("the posted operation settles");
                Ok(())
            })
            .await
        }
    });
    posted_rx
        .await
        .expect("the presentation operation was posted");
    caller.abort();
    assert!(
        caller
            .await
            .expect_err("the requester was cancelled")
            .is_cancelled()
    );

    let transition = gate.transport_transition(|| async { Ok(()) });
    tokio::pin!(transition);
    assert!(futures_util::poll!(&mut transition).is_pending());
    acknowledge
        .send(())
        .expect("the posted operation is still alive");
    transition
        .await
        .expect("the acknowledgement releases the gate");
}

#[tokio::test]
async fn android_transport_and_presentation_stop_before_a_generation_is_replaced() {
    let gate = AndroidPresentationGate::default();
    let order = Arc::new(StdMutex::new(Vec::new()));
    let transport_stop_order = Arc::clone(&order);
    let stop_order = Arc::clone(&order);
    let replace_order = Arc::clone(&order);
    let replaced = gate
        .stop_before_transport_replace(
            || async move {
                transport_stop_order
                    .lock()
                    .expect("ordering fixture is readable")
                    .push("transport-stop");
                Ok(())
            },
            || async move {
                stop_order
                    .lock()
                    .expect("ordering fixture is readable")
                    .push("presentation-stop");
                Ok(())
            },
            || async move {
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
            || async { Err(PlaybackManagerError::Cancelled) },
            || async move {
                presentation_flag.store(true, Ordering::SeqCst);
                Ok(())
            },
            || async move {
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
            || async { Err(PlaybackManagerError::Cancelled) },
            || async move {
                presentation_flag.store(true, Ordering::SeqCst);
                Err(AndroidPlaybackError)
            },
            || async move {
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
            || async { Ok(()) },
            || async { Err(AndroidPlaybackError) },
            || async move {
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
            || async move {
                transport_order
                    .lock()
                    .expect("cleanup order is readable")
                    .push("transport");
                Err(PlaybackManagerError::Cancelled)
            },
            || async move {
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
        gate.stop_after_transport(|| async { Ok(()) }, || async { Err(AndroidPlaybackError) })
            .await,
        Err(PlaybackManagerError::Unavailable)
    ));
}

#[tokio::test]
async fn aborted_stop_still_finishes_presentation_cleanup_before_the_gate_reopens() {
    let gate = Arc::new(AndroidPresentationGate::default());
    let presentation_stopped = Arc::new(AtomicBool::new(false));
    let following_call_started = Arc::new(AtomicBool::new(false));
    let (transport_entered, transport_entered_rx) = oneshot::channel();
    let (release_transport, release_transport_rx) = oneshot::channel();
    let (presentation_done, presentation_done_rx) = oneshot::channel();
    let (following_requested, following_requested_rx) = oneshot::channel();

    let cleanup_gate = Arc::clone(&gate);
    let presentation_flag = Arc::clone(&presentation_stopped);
    let cleanup = tokio::spawn(async move {
        cleanup_gate
            .stop_after_transport(
                || async move {
                    let _ = transport_entered.send(());
                    let _ = release_transport_rx.await;
                    Ok(())
                },
                || async move {
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
            .presentation_call(|| async move {
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
    let (transport_entered, transport_entered_rx) = oneshot::channel();
    let (release_transport, release_transport_rx) = oneshot::channel();
    let (presentation_done, presentation_done_rx) = oneshot::channel();

    let restart_gate = Arc::clone(&gate);
    let presentation_flag = Arc::clone(&presentation_stopped);
    let replacement_flag = Arc::clone(&replacement_polled);
    let restart = tokio::spawn(async move {
        restart_gate
            .stop_before_transport_replace(
                || async move {
                    let _ = transport_entered.send(());
                    let _ = release_transport_rx.await;
                    Ok(())
                },
                || async move {
                    presentation_flag.store(true, Ordering::SeqCst);
                    let _ = presentation_done.send(());
                    Ok(())
                },
                || async move {
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
    gate.presentation_call(|| async { Ok(()) })
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
    let (cleanup_entered, cleanup_entered_rx) = oneshot::channel();
    let (release_cleanup, release_cleanup_rx) = oneshot::channel();

    let cleanup_gate = Arc::clone(&gate);
    let cleanup_active = Arc::clone(&transport_active);
    let cleanup = tokio::spawn(async move {
        cleanup_gate
            .stop_after_transport(
                || async move {
                    cleanup_active.store(false, Ordering::SeqCst);
                    let _ = cleanup_entered.send(());
                    let _ = release_cleanup_rx.await;
                    Ok(())
                },
                || async { Ok(()) },
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
            .validate_then_start(
                identity(),
                |validated| async move {
                    assert_eq!(validated, identity());
                    start_active
                        .load(Ordering::SeqCst)
                        .then_some(())
                        .ok_or(PlaybackManagerError::Cancelled)
                },
                move |_| {
                    start_flag.store(true, Ordering::SeqCst);
                    async { Ok(()) }
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
    let (start_entered, start_entered_rx) = oneshot::channel();
    let (release_start, release_start_rx) = oneshot::channel();

    let start_gate = Arc::clone(&gate);
    let start_active = Arc::clone(&transport_active);
    let start_order = Arc::clone(&order);
    let start = tokio::spawn(async move {
        start_gate
            .validate_then_start(
                identity(),
                |validated| async move {
                    assert_eq!(validated, identity());
                    start_active
                        .load(Ordering::SeqCst)
                        .then_some(())
                        .ok_or(PlaybackManagerError::Cancelled)
                },
                |issued| async move {
                    assert_eq!(issued, identity());
                    start_order
                        .lock()
                        .expect("presentation order is readable")
                        .push("presentation-start");
                    start_entered
                        .send(())
                        .expect("test observes presentation start");
                    release_start_rx
                        .await
                        .expect("test releases presentation start");
                    Ok(())
                },
            )
            .await
    });
    start_entered_rx.await.expect("start owns the gate");

    let cleanup_gate = Arc::clone(&gate);
    let cleanup_active = Arc::clone(&transport_active);
    let cleanup_transport_order = Arc::clone(&order);
    let cleanup_presentation_order = Arc::clone(&order);
    let cleanup = tokio::spawn(async move {
        cleanup_gate
            .stop_after_transport(
                || async move {
                    cleanup_active.store(false, Ordering::SeqCst);
                    cleanup_transport_order
                        .lock()
                        .expect("presentation order is readable")
                        .push("transport-stop");
                    Ok(())
                },
                || async move {
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
