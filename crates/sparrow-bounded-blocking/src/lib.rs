use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{Notify, Semaphore},
    task, time,
};

const SERIAL_HANDOFF_WAIT: Duration = Duration::from_millis(100);
const SERIAL_HANDOFF_WAITERS: usize = 1;

/// Runs CPU-bound work away from Tokio workers with one active job and one
/// short, cancellation-safe handoff waiter.
#[derive(Clone)]
pub struct BoundedBlocking {
    permits: Arc<Semaphore>,
    handoff_waiters: Arc<Semaphore>,
    handoff_wait: Duration,
}

impl BoundedBlocking {
    pub fn serial() -> Self {
        Self::serial_with_handoff_wait(SERIAL_HANDOFF_WAIT)
    }

    fn serial_with_handoff_wait(handoff_wait: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(1)),
            handoff_waiters: Arc::new(Semaphore::new(SERIAL_HANDOFF_WAITERS)),
            handoff_wait,
        }
    }

    pub async fn run<Job, Output>(&self, job: Job) -> Result<Output, BlockingTaskUnavailable>
    where
        Job: FnOnce(BlockingTaskCancellation) -> Output + Send + 'static,
        Output: Send + 'static,
    {
        self.run_with_cancellation(BlockingTaskCancellation::new(), job)
            .await
    }

    pub async fn run_with_cancellation<Job, Output>(
        &self,
        cancellation: BlockingTaskCancellation,
        job: Job,
    ) -> Result<Output, BlockingTaskUnavailable>
    where
        Job: FnOnce(BlockingTaskCancellation) -> Output + Send + 'static,
        Output: Send + 'static,
    {
        let permit = self.acquire_permit(&cancellation).await?;
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

    async fn acquire_permit(
        &self,
        cancellation: &BlockingTaskCancellation,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, BlockingTaskUnavailable> {
        if cancellation.is_cancelled() {
            return Err(BlockingTaskUnavailable);
        }
        if let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() {
            return Ok(permit);
        }

        let _handoff_waiter = Arc::clone(&self.handoff_waiters)
            .try_acquire_owned()
            .map_err(|_| BlockingTaskUnavailable)?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(BlockingTaskUnavailable),
            permit = time::timeout(
                self.handoff_wait,
                Arc::clone(&self.permits).acquire_owned(),
            ) => permit
                .map_err(|_| BlockingTaskUnavailable)?
                .map_err(|_| BlockingTaskUnavailable),
        }
    }
}

#[derive(Clone)]
pub struct BlockingTaskCancellation {
    inner: Arc<BlockingTaskCancellationInner>,
}

struct BlockingTaskCancellationInner {
    cancelled: AtomicBool,
    notification: Notify,
}

impl BlockingTaskCancellation {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BlockingTaskCancellationInner {
                cancelled: AtomicBool::new(false),
                notification: Notify::new(),
            }),
        }
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.notification.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    async fn cancelled(&self) {
        let notification = self.inner.notification.notified();
        tokio::pin!(notification);
        notification.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notification.await;
    }
}

impl Default for BlockingTaskCancellation {
    fn default() -> Self {
        Self::new()
    }
}

struct CancelOnDrop(BlockingTaskCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockingTaskUnavailable;

#[cfg(test)]
mod tests;
