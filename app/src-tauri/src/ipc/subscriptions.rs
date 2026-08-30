use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use sparrow_core::SparrowCore;
use tauri::ipc::Channel;
use tokio::sync::oneshot;

use super::dto::{ClientErrorDto, CoreEventDto};

const SUBSCRIPTION_PREFIX: &str = "sub1_";

#[derive(Clone, Default)]
pub(crate) struct SubscriptionRegistry {
    shared: Arc<RegistryShared>,
}

#[derive(Default)]
struct RegistryShared {
    next: AtomicU64,
    active: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl SubscriptionRegistry {
    pub(crate) fn subscribe(
        &self,
        core: Arc<SparrowCore>,
        events: Channel<CoreEventDto>,
    ) -> Result<String, ClientErrorDto> {
        let sequence = self
            .shared
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| ClientErrorDto::service_unavailable())?
            + 1;
        let subscription_id = format!("{SUBSCRIPTION_PREFIX}{sequence:016x}");
        let (cancel, cancelled) = oneshot::channel();
        self.shared
            .active
            .lock()
            .expect("subscription registry poisoned")
            .insert(subscription_id.clone(), cancel);

        let id_for_task = subscription_id.clone();
        let registry = Arc::downgrade(&self.shared);
        tauri::async_runtime::spawn(forward_events(
            core,
            events,
            cancelled,
            registry,
            id_for_task,
        ));
        Ok(subscription_id)
    }

    pub(crate) fn unsubscribe(&self, subscription_id: &str) {
        if !valid_subscription_id(subscription_id) {
            return;
        }
        let cancellation = self
            .shared
            .active
            .lock()
            .expect("subscription registry poisoned")
            .remove(subscription_id);
        if let Some(cancellation) = cancellation {
            let _ = cancellation.send(());
        }
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.shared
            .active
            .lock()
            .expect("subscription registry poisoned")
            .len()
    }
}

async fn forward_events(
    core: Arc<SparrowCore>,
    events: Channel<CoreEventDto>,
    mut cancelled: oneshot::Receiver<()>,
    registry: Weak<RegistryShared>,
    subscription_id: String,
) {
    let mut stream = core.subscribe();
    loop {
        tokio::select! {
            _ = &mut cancelled => break,
            event = stream.recv() => {
                let Some(event) = event else {
                    break;
                };
                if events.send(CoreEventDto::from(event)).is_err() {
                    break;
                }
            }
        }
    }

    if let Some(registry) = registry.upgrade() {
        registry
            .active
            .lock()
            .expect("subscription registry poisoned")
            .remove(&subscription_id);
    }
}

fn valid_subscription_id(value: &str) -> bool {
    value.len() == SUBSCRIPTION_PREFIX.len() + 16
        && value.starts_with(SUBSCRIPTION_PREFIX)
        && value[SUBSCRIPTION_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use sparrow_core::{CoreAdapters, SparrowCore, SystemClock};
    use tauri::ipc::{Channel, InvokeResponseBody};
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn forwards_initial_event_and_unsubscribe_is_idempotent() {
        let directory = TempDir::new().expect("temporary directory");
        let snapshots = sparrow_snapshot_store::AtomicFileSnapshotStore::open(directory.path())
            .expect("snapshot store opens");
        let source = sparrow_source_http::HttpSourceAccess::new().expect("source adapter opens");
        let core = Arc::new(
            SparrowCore::bootstrap(
                None,
                CoreAdapters::new(Arc::new(source), Arc::new(snapshots), Arc::new(SystemClock)),
            )
            .await
            .expect("unconfigured core bootstraps"),
        );
        let messages = Arc::new(Mutex::new(Vec::new()));
        let messages_for_channel = Arc::clone(&messages);
        let events = Channel::new(move |body| {
            messages_for_channel
                .lock()
                .expect("messages lock poisoned")
                .push(body);
            Ok(())
        });
        let registry = SubscriptionRegistry::default();

        let subscription_id = registry
            .subscribe(core, events)
            .expect("subscription starts");
        assert!(valid_subscription_id(&subscription_id));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !messages.lock().expect("messages lock poisoned").is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("initial event arrives");

        let json = {
            let messages = messages.lock().expect("messages lock poisoned");
            let InvokeResponseBody::Json(json) = &messages[0] else {
                panic!("catalog events use JSON IPC bodies");
            };
            json.clone()
        };
        assert!(json.contains("catalog-status-changed"));
        assert!(!json.contains("http://"));
        assert!(!json.contains("https://"));

        registry.unsubscribe(&subscription_id);
        registry.unsubscribe(&subscription_id);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if registry.active_count() == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("subscription is removed");
    }

    #[test]
    fn subscription_ids_are_bounded_and_strict() {
        assert!(valid_subscription_id("sub1_0000000000000001"));
        assert!(!valid_subscription_id("sub1_000000000000000A"));
        assert!(!valid_subscription_id("1"));
        assert!(!valid_subscription_id("sub1_0000000000000001-extra"));
        assert!(!valid_subscription_id("sub1_private/location"));
    }
}
