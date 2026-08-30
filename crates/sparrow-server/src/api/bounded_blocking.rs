use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::{sync::Semaphore, task};

/// Runs CPU-bound API work away from Tokio workers with a bounded in-flight count.
#[derive(Clone)]
pub(super) struct BoundedBlocking {
    permits: Arc<Semaphore>,
}

impl BoundedBlocking {
    pub(super) fn serial() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(1)),
        }
    }

    pub(super) async fn run<Job, Output>(&self, job: Job) -> Result<Output, BlockingTaskUnavailable>
    where
        Job: FnOnce(BlockingTaskCancellation) -> Output + Send + 'static,
        Output: Send + 'static,
    {
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| BlockingTaskUnavailable)?;

        let cancellation = BlockingTaskCancellation::new();
        let cancel_on_drop = CancelOnDrop(cancellation.clone());
        let joined = task::spawn_blocking(move || {
            let _permit = permit;
            job(cancellation)
        })
        .await;
        drop(cancel_on_drop);

        match joined {
            Ok(output) => Ok(output),
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(_) => Err(BlockingTaskUnavailable),
        }
    }
}

#[derive(Clone)]
pub(super) struct BlockingTaskCancellation {
    cancelled: Arc<AtomicBool>,
}

impl BlockingTaskCancellation {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

struct CancelOnDrop(BlockingTaskCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BlockingTaskUnavailable;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use tokio::sync::oneshot;

    use super::BoundedBlocking;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serial_executor_rejects_overload_and_recovers_after_active_work() {
        let executor = BoundedBlocking::serial();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));

        let (first_started_tx, first_started_rx) = oneshot::channel();
        let first_executor = executor.clone();
        let first_active = Arc::clone(&active);
        let first_maximum = Arc::clone(&maximum);
        let first_release = Arc::clone(&release);
        let first = tokio::spawn(async move {
            first_executor
                .run(move |_| {
                    observe_active(&first_active, &first_maximum);
                    first_started_tx
                        .send(())
                        .expect("the test receives the start signal");
                    while !first_release.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    first_active.fetch_sub(1, Ordering::AcqRel);
                })
                .await
        });
        first_started_rx
            .await
            .expect("the first blocking task starts");

        let rejected_started = Arc::new(AtomicBool::new(false));
        let rejected_started_from_job = Arc::clone(&rejected_started);
        assert_eq!(
            executor
                .run(move |_| rejected_started_from_job.store(true, Ordering::Release))
                .await,
            Err(super::BlockingTaskUnavailable)
        );
        assert!(!rejected_started.load(Ordering::Acquire));

        let async_probe = tokio::spawn(async { 41_u8 + 1 });
        assert_eq!(async_probe.await.expect("the async probe completes"), 42);

        release.store(true, Ordering::Release);
        first
            .await
            .expect("the first join task completes")
            .expect("the first blocking task completes");

        let later_active = Arc::clone(&active);
        let later_maximum = Arc::clone(&maximum);
        executor
            .run(move |_| {
                observe_active(&later_active, &later_maximum);
                later_active.fetch_sub(1, Ordering::AcqRel);
            })
            .await
            .expect("work succeeds after the active task releases its permit");
        assert_eq!(maximum.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    #[should_panic(expected = "blocking search defect")]
    async fn blocking_task_panics_are_resumed() {
        let executor = BoundedBlocking::serial();

        let _ = executor
            .run(|_| -> () { panic!("blocking search defect") })
            .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_waiter_cooperatively_cancels_active_work() {
        let executor = BoundedBlocking::serial();
        let (started_tx, started_rx) = oneshot::channel();
        let (finished_tx, finished_rx) = oneshot::channel();
        let active_executor = executor.clone();
        let active = tokio::spawn(async move {
            active_executor
                .run(move |cancellation| {
                    started_tx
                        .send(())
                        .expect("the test receives the start signal");
                    while !cancellation.is_cancelled() {
                        std::thread::yield_now();
                    }
                    finished_tx
                        .send(())
                        .expect("the test receives the finish signal");
                })
                .await
        });
        started_rx.await.expect("the blocking task starts");

        active.abort();
        assert!(
            active
                .await
                .expect_err("the waiter is aborted")
                .is_cancelled()
        );
        finished_rx
            .await
            .expect("the blocking task observes cancellation");

        executor
            .run(|_| ())
            .await
            .expect("the permit is released after cooperative cancellation");
    }

    fn observe_active(active: &AtomicUsize, maximum: &AtomicUsize) {
        let current = active.fetch_add(1, Ordering::AcqRel) + 1;
        maximum.fetch_max(current, Ordering::AcqRel);
    }
}
