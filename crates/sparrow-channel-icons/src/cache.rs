//! Viewport, request and eviction policy for a native Channel Icon cache.

use std::{
    borrow::Borrow,
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

pub const REQUEST_LIMIT: usize = 4;
const MAX_LOCATION_BYTES: usize = 4096;
const NEGATIVE_TTL: Duration = Duration::from_secs(30);
const NAVIGATION_QUIET: Duration = Duration::from_millis(140);

#[derive(Clone, Copy)]
pub struct CacheLimits {
    pub max_bytes: usize,
    pub max_entries: usize,
    pub max_desired: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 << 20,
            max_entries: 96,
            max_desired: 24,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RequestId(u64);

impl RequestId {
    fn next(counter: &mut u64) -> Self {
        let id = Self(*counter);
        *counter = counter.wrapping_add(1);
        id
    }
}

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
    stale: bool,
}

#[derive(PartialEq)]
enum DiskRead {
    Pending(RequestId),
    Miss,
}

/// Keeps values and desired locations for one view. The host starts and aborts
/// the tasks, and releases the renderer resources returned by eviction.
pub struct IconCache<V> {
    limits: CacheLimits,
    desired: Vec<IconKey>,
    disk_reads: HashMap<IconKey, DiskRead>,
    pending: HashMap<IconKey, RequestId>,
    failed: HashMap<IconKey, Instant>,
    ready: HashMap<IconKey, Cached<V>>,
    bytes: usize,
    next_request: u64,
    quiet_until: Option<Instant>,
}

impl<V> Default for IconCache<V> {
    fn default() -> Self {
        Self::with_limits(CacheLimits::default())
    }
}

impl<V> IconCache<V> {
    pub fn with_limits(limits: CacheLimits) -> Self {
        Self {
            limits,
            desired: Vec::new(),
            disk_reads: HashMap::new(),
            pending: HashMap::new(),
            failed: HashMap::new(),
            ready: HashMap::new(),
            bytes: 0,
            next_request: 0,
            quiet_until: None,
        }
    }

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
        self.pending
            .drain()
            .map(|(_, request)| request)
            .chain(self.disk_reads.drain().filter_map(|(_, read)| match read {
                DiskRead::Pending(id) => Some(id),
                DiskRead::Miss => None,
            }))
            .collect()
    }

    /// Accepts viewport order, de-duplicates locations, and returns superseded requests.
    pub fn reconcile<'a>(
        &mut self,
        locations: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> Vec<RequestId> {
        let mut desired = Vec::<IconKey>::new();
        for location in locations {
            if desired.len() == self.limits.max_desired {
                break;
            }
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
        }
        let mut retired = Vec::new();
        self.disk_reads.retain(|key, read| {
            if desired.contains(key) {
                return true;
            }
            if let DiskRead::Pending(id) = read {
                retired.push(*id);
            }
            false
        });
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

    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }

    /// Locations eligible for a disk check, in viewport priority order.
    pub fn disk_candidates(&self) -> impl Iterator<Item = Arc<str>> + '_ {
        self.desired
            .iter()
            .filter(|key| {
                !self.ready.contains_key(*key)
                    && !self.disk_reads.contains_key(*key)
                    && !self.pending.contains_key(*key)
            })
            .map(|key| Arc::clone(&key.0))
    }

    pub fn note_disk_miss(&mut self, location: Arc<str>) {
        self.disk_reads.insert(IconKey(location), DiskRead::Miss);
    }

    /// Reserve a disk check independently of the HTTP request slots.
    pub fn begin_disk_read(&mut self, location: Arc<str>) -> RequestId {
        let id = RequestId::next(&mut self.next_request);
        self.disk_reads
            .insert(IconKey(location), DiskRead::Pending(id));
        id
    }

    pub fn complete_disk_read(
        &mut self,
        id: RequestId,
        value: Option<((V, usize), bool)>,
        now: Instant,
    ) -> (bool, Vec<V>) {
        let Some(key) = self
            .disk_reads
            .iter()
            .find_map(|(key, read)| (*read == DiskRead::Pending(id)).then(|| key.clone()))
        else {
            return (false, Vec::new());
        };
        match value {
            Some((value, stale)) => {
                self.disk_reads.remove(&key);
                self.insert(key, value, stale, now)
            }
            None => {
                self.disk_reads.insert(key, DiskRead::Miss);
                (false, Vec::new())
            }
        }
    }

    pub fn insert_cached(
        &mut self,
        location: &str,
        value: (V, usize),
        stale: bool,
        now: Instant,
    ) -> (bool, Vec<V>) {
        let Some(key) = self.desired.iter().find(|key| key.0.as_ref() == location) else {
            return (false, Vec::new());
        };
        self.insert(key.clone(), value, stale, now)
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
            let id = RequestId::next(&mut self.next_request);
            self.pending.insert(key.clone(), id);
            requests.push(IconRequest {
                id,
                key: key.clone(),
            });
        }
        requests
    }

    fn needs_request(&self, key: &IconKey, now: Instant) -> bool {
        self.ready.get(key).is_none_or(|entry| entry.stale)
            && !self.pending.contains_key(key)
            && !matches!(self.disk_reads.get(key), Some(DiskRead::Pending(_)))
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
        if let Some(value) = value {
            return self.insert(key, value, false, now);
        }
        self.note_failure(key, now);
        (false, Vec::new())
    }

    fn insert(
        &mut self,
        key: IconKey,
        (value, bytes): (V, usize),
        stale: bool,
        now: Instant,
    ) -> (bool, Vec<V>) {
        let bytes = bytes.saturating_add(key.0.len());
        if bytes > self.limits.max_bytes || self.limits.max_entries == 0 {
            self.note_failure(key, now);
            return (false, vec![value]);
        }
        if !stale {
            self.failed.remove(&key);
        }
        let old = self.ready.insert(
            key.clone(),
            Cached {
                value,
                used_at: now,
                bytes,
                stale,
            },
        );
        let mut evicted = Vec::new();
        if let Some(old) = old {
            self.bytes -= old.bytes;
            evicted.push(old.value);
        }
        self.bytes += bytes;
        evicted.extend(self.evict());
        let retained = self.ready.contains_key(&key);
        if !retained {
            self.note_failure(key, now);
        }
        (retained, evicted)
    }

    fn note_failure(&mut self, key: IconKey, now: Instant) {
        if self.failed.len() >= self.limits.max_entries
            && let Some(oldest) = self
                .failed
                .iter()
                .min_by_key(|(_, expiry)| *expiry)
                .map(|(key, _)| key.clone())
        {
            self.failed.remove(&oldest);
        }
        self.failed.insert(key, now + NEGATIVE_TTL);
    }

    fn evict(&mut self) -> Vec<V> {
        let mut values = Vec::new();
        while self.bytes > self.limits.max_bytes || self.ready.len() > self.limits.max_entries {
            let Some(key) = self
                .ready
                .iter()
                .min_by_key(|(key, entry)| {
                    let priority = self
                        .desired
                        .iter()
                        .rev()
                        .position(|desired| desired == *key);
                    (priority, entry.used_at)
                })
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
