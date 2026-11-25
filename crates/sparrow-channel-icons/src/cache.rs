//! Viewport, request and eviction policy for a native Channel Icon cache.

use std::{
    borrow::Borrow,
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

pub const REQUEST_LIMIT: usize = 4;
const MAX_DESIRED: usize = 24;
const MAX_ENTRIES: usize = 96;
const CACHE_BYTES: usize = 16 * 1024 * 1024;
// Leave room for the largest decoded icon on both CPU and GPU per desired location.
const MAX_LOCATION_BYTES: usize = CACHE_BYTES / MAX_DESIRED - (256 * 256 * 8);
const NEGATIVE_TTL: Duration = Duration::from_secs(30);
const NAVIGATION_QUIET: Duration = Duration::from_millis(140);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RequestId(u64);

#[derive(Debug)]
pub struct IconRequest {
    id: RequestId,
    key: IconKey,
}

impl IconRequest {
    pub fn id(&self) -> RequestId {
        self.id
    }

    pub fn location(&self) -> &str {
        &self.key.0
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct IconKey(Arc<str>);

impl Borrow<str> for IconKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IconKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IconKey(..)")
    }
}

struct Cached<V> {
    value: V,
    used_at: Instant,
    bytes: usize,
}

/// Keeps values and desired locations for one view. The host starts and aborts
/// the tasks, and releases the renderer resources returned by eviction.
pub struct IconCache<V> {
    desired: Vec<IconKey>,
    pending: HashMap<IconKey, RequestId>,
    failed: HashMap<IconKey, Instant>,
    ready: HashMap<IconKey, Cached<V>>,
    bytes: usize,
    next_request: u64,
    quiet_until: Option<Instant>,
}

impl<V> Default for IconCache<V> {
    fn default() -> Self {
        Self {
            desired: Vec::new(),
            pending: HashMap::new(),
            failed: HashMap::new(),
            ready: HashMap::new(),
            bytes: 0,
            next_request: 0,
            quiet_until: None,
        }
    }
}

impl<V> IconCache<V> {
    pub fn note_navigation(&mut self, now: Instant) {
        self.quiet_until = Some(now + NAVIGATION_QUIET);
    }

    pub fn notification_quiet_until(&self) -> Option<Instant> {
        self.quiet_until.filter(|_| !self.pending.is_empty())
    }

    /// Returns a wakeup only when navigation is delaying an eligible fetch.
    pub fn deferred_admission(&self, now: Instant) -> Option<Instant> {
        let when = self.quiet_until.filter(|&when| when > now)?;
        (self.pending.len() < REQUEST_LIMIT
            && self.desired.iter().any(|key| self.needs_request(key, when)))
        .then_some(when)
    }

    /// Retires requests while preserving the successful images for a resume.
    pub fn hide(&mut self) -> Vec<RequestId> {
        self.quiet_until = None;
        self.forget_viewport()
    }

    /// A new catalog can retry failures, including at unchanged locations.
    pub fn reset_generation(&mut self) -> Vec<RequestId> {
        self.failed.clear();
        self.forget_viewport()
    }

    fn forget_viewport(&mut self) -> Vec<RequestId> {
        self.desired.clear();
        self.pending.drain().map(|(_, request)| request).collect()
    }

    /// Accepts viewport order, de-duplicates locations, and returns superseded requests.
    pub fn reconcile<'a>(
        &mut self,
        locations: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> Vec<RequestId> {
        let mut desired = Vec::<IconKey>::new();
        for location in locations {
            if location.len() > MAX_LOCATION_BYTES
                || desired.iter().any(|key| key.0.as_ref() == location)
            {
                continue;
            }
            let key = self
                .desired
                .iter()
                .find(|key| key.0.as_ref() == location)
                .cloned()
                .unwrap_or_else(|| IconKey(Arc::from(location)));
            if let Some(entry) = self.ready.get_mut(&key) {
                entry.used_at = now;
            }
            desired.push(key);
            if desired.len() == MAX_DESIRED {
                break;
            }
        }
        let mut retired = Vec::new();
        self.pending.retain(|key, request| {
            if desired.contains(key) {
                true
            } else {
                retired.push(*request);
                false
            }
        });
        self.failed.retain(|_, expires| *expires > now);
        self.desired = desired;
        retired
    }

    pub fn get(&self, location: &str) -> Option<&V> {
        self.ready.get(location).map(|entry| &entry.value)
    }

    pub fn admit(&mut self, now: Instant) -> Vec<IconRequest> {
        let mut requests = Vec::new();
        if self.quiet_until.is_some_and(|when| now < when) {
            return requests;
        }
        for key in &self.desired {
            if self.pending.len() >= REQUEST_LIMIT {
                break;
            }
            if !self.needs_request(key, now) {
                continue;
            }
            let id = RequestId(self.next_request);
            self.next_request = self.next_request.wrapping_add(1);
            self.pending.insert(key.clone(), id);
            requests.push(IconRequest {
                id,
                key: key.clone(),
            });
        }
        requests
    }

    fn needs_request(&self, key: &IconKey, now: Instant) -> bool {
        !self.ready.contains_key(key)
            && !self.pending.contains_key(key)
            && self.failed.get(key).is_none_or(|&expires| expires <= now)
    }

    /// Reports a visible change and any values evicted. Ignore results for
    /// forgotten requests, even when the same location is visible again.
    pub fn complete(
        &mut self,
        id: RequestId,
        value: Option<(V, usize)>,
        now: Instant,
    ) -> (bool, Vec<V>) {
        let Some(key) = self
            .pending
            .iter()
            .find_map(|(key, request)| (*request == id).then(|| key.clone()))
        else {
            return (false, Vec::new());
        };
        self.pending.remove(&key);
        if let Some((value, bytes)) = value {
            let bytes = bytes.saturating_add(key.0.len());
            if bytes <= CACHE_BYTES {
                self.bytes += bytes;
                self.ready.insert(
                    key,
                    Cached {
                        value,
                        used_at: now,
                        bytes,
                    },
                );
                return (true, self.evict());
            }
        }
        if self.failed.len() >= MAX_ENTRIES
            && let Some(oldest) = self
                .failed
                .iter()
                .min_by_key(|(_, expiry)| *expiry)
                .map(|(key, _)| key.clone())
        {
            self.failed.remove(&oldest);
        }
        self.failed.insert(key, now + NEGATIVE_TTL);
        (false, Vec::new())
    }

    fn evict(&mut self) -> Vec<V> {
        let mut values = Vec::new();
        while self.bytes > CACHE_BYTES || self.ready.len() > MAX_ENTRIES {
            let Some(key) = self
                .ready
                .iter()
                .min_by_key(|(key, entry)| (self.desired.contains(key), entry.used_at))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.ready.remove(&key) {
                self.bytes -= entry.bytes;
                values.push(entry.value);
            }
        }
        values
    }
}
