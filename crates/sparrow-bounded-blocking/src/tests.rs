use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{Instant, timeout},
};

use super::{BlockingTaskCancellation, BlockingTaskUnavailable, BoundedBlocking};

const TEST_WAIT: Duration = Duration::from_secs(1);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serial_executor_bounds_the_handoff_queue_and_recovers() {
    let executor = handoff_executor();
    let active = HeldTask::start(&executor).await;

    let queued_executor = executor.clone();
    let (queued_started_tx, mut queued_started_rx) = oneshot::channel();
    let queued = tokio::spawn(async move {
        queued_executor
            .run(move |_| {
                queued_started_tx
                    .send(())
                    .expect("the queued job starts after handoff");
            })
            .await
    });
    wait_for_handoff_waiter(&executor).await;

    let overflow_started = Arc::new(AtomicBool::new(false));
    let overflow_job = Arc::clone(&overflow_started);
    assert_eq!(
        executor
            .run(move |_| overflow_job.store(true, Ordering::Release))
            .await,
        Err(BlockingTaskUnavailable)
    );
    assert!(!overflow_started.load(Ordering::Acquire));
    assert!(matches!(
        queued_started_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    active.finish().await;
    timeout(TEST_WAIT, queued)
        .await
        .expect("the queued wait is finite")
        .expect("the queued task joins")
        .expect("the queued job completes");
    executor
        .run(|_| ())
        .await
        .expect("the executor recovers after handoff");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_waits_while_cancelled_work_releases_the_permit() {
    let executor = handoff_executor();
    let release = Arc::new(AtomicBool::new(false));
    let cancellation = BlockingTaskCancellation::new();
    let (started_tx, started_rx) = oneshot::channel();
    let (cancelled_tx, cancelled_rx) = oneshot::channel();
    let active_executor = executor.clone();
    let active_release = Arc::clone(&release);
    let active_cancellation = cancellation.clone();
    let active = tokio::spawn(async move {
        active_executor
            .run_with_cancellation(active_cancellation, move |cancellation| {
                started_tx.send(()).expect("the active job starts");
                while !cancellation.is_cancelled() {
                    std::thread::yield_now();
                }
                cancelled_tx
                    .send(())
                    .expect("the active job reports cancellation");
                while !active_release.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
            })
            .await
    });
    timeout(TEST_WAIT, started_rx)
        .await
        .expect("the start wait is finite")
        .expect("the active job starts");

    cancellation.cancel();
    timeout(TEST_WAIT, cancelled_rx)
        .await
        .expect("the cancellation wait is finite")
        .expect("the active job observes cancellation");
    let mut replacement = Box::pin(executor.run(|_| 42_u8));
    assert!(
        timeout(Duration::from_millis(10), &mut replacement)
            .await
            .is_err(),
        "the replacement waits instead of failing"
    );
    release.store(true, Ordering::Release);
    assert_eq!(
        timeout(TEST_WAIT, replacement)
            .await
            .expect("the replacement wait is finite"),
        Ok(42)
    );
    active
        .await
        .expect("the active task joins")
        .expect("the active job completes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_an_active_waiter_cooperatively_cancels_its_job() {
    let executor = BoundedBlocking::serial();
    let (started_tx, started_rx) = oneshot::channel();
    let (finished_tx, finished_rx) = oneshot::channel();
    let active_executor = executor.clone();
    let active = tokio::spawn(async move {
        active_executor
            .run(move |cancellation| {
                started_tx.send(()).expect("the active job starts");
                while !cancellation.is_cancelled() {
                    std::thread::yield_now();
                }
                finished_tx
                    .send(())
                    .expect("the active job reports cancellation");
            })
            .await
    });
    started_rx.await.expect("the active job starts");
    active.abort();
    assert!(
        active
            .await
            .expect_err("the waiter is aborted")
            .is_cancelled()
    );
    timeout(TEST_WAIT, finished_rx)
        .await
        .expect("the cancellation wait is finite")
        .expect("the detached job observes cancellation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_queued_waiter_releases_the_queue_slot() {
    let executor = handoff_executor();
    let active = HeldTask::start(&executor).await;
    let queued_executor = executor.clone();
    let queued = tokio::spawn(async move { queued_executor.run(|_| 1_u8).await });
    wait_for_handoff_waiter(&executor).await;
    queued.abort();
    assert!(
        queued
            .await
            .expect_err("the queued waiter aborts")
            .is_cancelled()
    );

    let final_executor = executor.clone();
    let final_request = tokio::spawn(async move { final_executor.run(|_| 2_u8).await });
    wait_for_handoff_waiter(&executor).await;
    active.finish().await;
    assert_eq!(
        timeout(TEST_WAIT, final_request)
            .await
            .expect("the final wait is finite")
            .expect("the final task joins"),
        Ok(2)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cooperatively_cancelling_a_queued_job_releases_the_queue_slot() {
    let executor = handoff_executor();
    let active = HeldTask::start(&executor).await;
    let cancellation = BlockingTaskCancellation::new();
    let queued_started = Arc::new(AtomicBool::new(false));
    let queued_job_started = Arc::clone(&queued_started);
    let queued_executor = executor.clone();
    let queued_cancellation = cancellation.clone();
    let queued = tokio::spawn(async move {
        queued_executor
            .run_with_cancellation(queued_cancellation, move |_| {
                queued_job_started.store(true, Ordering::Release);
            })
            .await
    });
    wait_for_handoff_waiter(&executor).await;

    cancellation.cancel();
    assert_eq!(
        timeout(TEST_WAIT, queued)
            .await
            .expect("the cancelled wait is finite")
            .expect("the cancelled task joins"),
        Err(BlockingTaskUnavailable)
    );
    assert!(!queued_started.load(Ordering::Acquire));

    let replacement_executor = executor.clone();
    let replacement = tokio::spawn(async move { replacement_executor.run(|_| 2_u8).await });
    wait_for_handoff_waiter(&executor).await;
    active.finish().await;
    assert_eq!(
        timeout(TEST_WAIT, replacement)
            .await
            .expect("the replacement wait is finite")
            .expect("the replacement task joins"),
        Ok(2)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_timeout_is_finite() {
    let wait = Duration::from_millis(5);
    let executor = BoundedBlocking::serial_with_handoff_wait(wait);
    let active = HeldTask::start(&executor).await;
    let started = Instant::now();
    assert_eq!(executor.run(|_| ()).await, Err(BlockingTaskUnavailable));
    assert!(started.elapsed() >= wait);
    active.finish().await;
}

#[tokio::test]
#[should_panic(expected = "blocking defect")]
async fn blocking_panics_resume_on_the_waiter() {
    let _ = BoundedBlocking::serial()
        .run(|_| -> () { panic!("blocking defect") })
        .await;
}

fn handoff_executor() -> BoundedBlocking {
    BoundedBlocking::serial_with_handoff_wait(TEST_WAIT)
}

async fn wait_for_handoff_waiter(executor: &BoundedBlocking) {
    timeout(TEST_WAIT, async {
        while executor.handoff_waiters.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the handoff waiter starts");
}

struct HeldTask {
    release: Arc<AtomicBool>,
    join: JoinHandle<Result<(), BlockingTaskUnavailable>>,
}

impl HeldTask {
    async fn start(executor: &BoundedBlocking) -> Self {
        let release = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = oneshot::channel();
        let active_executor = executor.clone();
        let active_release = Arc::clone(&release);
        let join = tokio::spawn(async move {
            active_executor
                .run(move |_| {
                    started_tx.send(()).expect("the held job starts");
                    while !active_release.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                })
                .await
        });
        timeout(TEST_WAIT, started_rx)
            .await
            .expect("the start wait is finite")
            .expect("the held job starts");
        Self { release, join }
    }

    async fn finish(self) {
        self.release.store(true, Ordering::Release);
        timeout(TEST_WAIT, self.join)
            .await
            .expect("the held job wait is finite")
            .expect("the held task joins")
            .expect("the held job completes");
    }
}
