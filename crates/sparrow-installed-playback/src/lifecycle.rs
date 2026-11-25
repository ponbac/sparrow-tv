use std::{future::Future, sync::Mutex};

use tokio::{runtime::Handle, sync::oneshot};

use crate::PlaybackManagerError;

/// Orders lifecycle transitions by receipt, independently of completion waiters.
#[derive(Default)]
pub struct LifecycleOrder {
    last: Mutex<Option<oneshot::Receiver<()>>>,
}

impl LifecycleOrder {
    /// Submits a transition immediately. Dropping the returned completion does
    /// not cancel the transition or leave a gap in the queue.
    pub fn dispatch<Operation, Output>(
        &self,
        runtime: &Handle,
        operation: Operation,
    ) -> impl Future<Output = Result<Output, PlaybackManagerError>>
    + Send
    + 'static
    + use<Operation, Output>
    where
        Operation: Future<Output = Result<Output, PlaybackManagerError>> + Send + 'static,
        Output: Send + 'static,
    {
        let (finished, completion) = oneshot::channel();
        let task = self
            .last
            .lock()
            .map_err(|_| PlaybackManagerError::Unavailable)
            .map(|mut last| {
                let predecessor = last.replace(completion);
                runtime.spawn(async move {
                    let _finished = finished;
                    if let Some(predecessor) = predecessor {
                        let _ = predecessor.await;
                    }
                    operation.await
                })
            });
        async move { task?.await.map_err(|_| PlaybackManagerError::Unavailable)? }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_submitted_transition_keeps_its_turn_until_the_operation_finishes() {
        let order = LifecycleOrder::default();
        let runtime = Handle::current();
        let (started, starting) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let first = order.dispatch(&runtime, async {
            started.send(()).expect("the first operation starts");
            released.await.expect("the first operation is released");
            Ok(())
        });
        drop(first);
        starting.await.expect("the first operation runs");

        let (second_started, mut second_starting) = oneshot::channel();
        let second = order.dispatch(&runtime, async {
            second_started
                .send(())
                .expect("the second operation starts");
            Ok(())
        });
        tokio::task::yield_now().await;
        assert!(matches!(
            second_starting.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        release.send(()).expect("the first operation remains alive");
        second.await.expect("the second operation finishes");
        second_starting.await.expect("the second operation runs");
    }
}
