use std::time::{Duration, Instant};

use sparrow_channel_icons::cache::{IconCache, REQUEST_LIMIT};

#[test]
fn duplicate_placements_use_one_slot_and_retire_only_forgotten_requests() {
    let now = Instant::now();
    let mut cache = IconCache::<u8>::default();
    let key = "https://fixture.invalid/private-icon";
    assert!(
        cache
            .reconcile([key, key, "b", "c", "d", "e"], now)
            .is_empty()
    );
    let requests = cache.admit(now);
    assert_eq!(requests.len(), REQUEST_LIMIT);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.location())
            .collect::<Vec<_>>(),
        [key, "b", "c", "d"]
    );
    assert!(!format!("{:?}", requests[0]).contains(key));
    assert!(cache.admit(now).is_empty());

    let retired = cache.reconcile([key, "e"], now);
    assert_eq!(retired.len(), 3);
    assert!(
        requests[1..]
            .iter()
            .all(|request| retired.contains(&request.id()))
    );
    let next = cache.admit(now);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].location(), "e");

    assert_eq!(
        cache.complete(requests[0].id(), Some((7, 1)), now),
        (true, vec![])
    );
    assert_eq!(cache.get(key), Some(&7));
}

#[test]
fn hidden_and_replaced_requests_cannot_publish_into_the_new_viewport() {
    let now = Instant::now();
    let mut cache = IconCache::default();
    cache.reconcile(["same"], now);
    let original = cache.admit(now)[0].id();
    assert_eq!(cache.hide(), [original]);
    assert!(cache.admit(now).is_empty());

    cache.reconcile(["same"], now);
    let resumed = cache.admit(now)[0].id();
    assert_ne!(original, resumed);
    assert_eq!(cache.complete(original, Some((1, 1)), now), (false, vec![]));
    assert_eq!(cache.get("same"), None);

    assert_eq!(cache.reset_generation(), [resumed]);
    cache.reconcile(["same"], now);
    let current = cache.admit(now)[0].id();
    assert_ne!(current, resumed);
    assert_eq!(cache.complete(resumed, Some((2, 1)), now), (false, vec![]));
    assert_eq!(cache.complete(current, Some((3, 1)), now), (true, vec![]));
    assert_eq!(cache.get("same"), Some(&3));
}

#[test]
fn failures_suppress_retries_until_expiry_or_catalog_change() {
    let now = Instant::now();
    let mut cache = IconCache::<()>::default();
    cache.reconcile(["failure"], now);
    let original = cache.admit(now)[0].id();
    assert_eq!(cache.complete(original, None, now), (false, vec![]));
    assert!(cache.admit(now + Duration::from_secs(29)).is_empty());

    let later = now + Duration::from_secs(30);
    let retry = cache.admit(later)[0].id();
    assert_ne!(retry, original);
    cache.complete(retry, None, later);
    assert!(cache.admit(later).is_empty());

    cache.reset_generation();
    cache.reconcile(["failure"], later);
    assert_eq!(cache.admit(later)[0].location(), "failure");
}

#[test]
fn growing_the_cache_evicts_the_least_recently_seen_offscreen_value() {
    const IMAGE_BYTES: usize = 5 * 1024 * 1024;
    let now = Instant::now();
    let mut cache = IconCache::default();
    for (index, key) in ["a", "b", "c"].into_iter().enumerate() {
        let at = now + Duration::from_secs(index as u64);
        cache.reconcile([key], at);
        let request = cache.admit(at)[0].id();
        assert_eq!(
            cache.complete(request, Some((key, IMAGE_BYTES)), at),
            (true, vec![])
        );
    }
    cache.reconcile(["a"], now + Duration::from_secs(3));
    let later = now + Duration::from_secs(4);
    cache.reconcile(["c", "d"], later);
    let request = cache.admit(later)[0].id();
    assert_eq!(
        cache.complete(request, Some(("d", IMAGE_BYTES)), later),
        (true, vec!["b"])
    );
    assert_eq!(cache.get("b"), None);
    assert_eq!(cache.get("a"), Some(&"a"));
    assert_eq!(cache.get("c"), Some(&"c"));
    assert_eq!(cache.get("d"), Some(&"d"));
}

#[test]
fn repeated_navigation_moves_the_fill_deadline_and_admits_the_latest_viewport() {
    let now = Instant::now();
    let mut cache = IconCache::<()>::default();
    cache.note_navigation(now);
    assert_eq!(cache.deferred_admission(now), None);
    cache.reconcile(["first"], now);
    assert_eq!(
        cache.deferred_admission(now),
        Some(now + Duration::from_millis(140))
    );
    assert!(cache.admit(now).is_empty());

    let repeat = now + Duration::from_millis(130);
    cache.note_navigation(repeat);
    cache.reconcile(["latest"], repeat);
    let deadline = repeat + Duration::from_millis(140);
    assert_eq!(cache.deferred_admission(repeat), Some(deadline));
    assert!(cache.admit(deadline - Duration::from_millis(1)).is_empty());

    // A deadline wake can admit without a preceding completion or new viewport.
    let requests = cache.admit(deadline);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].location(), "latest");
    assert_eq!(cache.deferred_admission(deadline), None);
    assert!(cache.admit(deadline).is_empty());
}

#[test]
fn failed_refresh_preserves_the_stale_image() {
    let now = Instant::now();
    let mut cache = IconCache::default();
    cache.reconcile(["a"], now);
    let id = cache.admit(now)[0].id();
    assert_eq!(cache.publish_stale(id, ("old", 100), now), (true, vec![]));
    assert_eq!(cache.get("a"), Some(&"old"));
    assert!(cache.admit(now).is_empty());
    assert_eq!(cache.complete(id, None, now), (false, vec![]));
    assert_eq!(cache.get("a"), Some(&"old"));
}

#[test]
fn successful_refresh_replaces_the_stale_image() {
    let now = Instant::now();
    let mut cache = IconCache::default();
    cache.reconcile(["a"], now);
    let id = cache.admit(now)[0].id();
    assert_eq!(cache.publish_stale(id, ("old", 100), now), (true, vec![]));
    assert_eq!(cache.get("a"), Some(&"old"));
    assert!(cache.admit(now).is_empty());
    assert_eq!(
        cache.complete(id, Some(("new", 100)), now),
        (true, vec!["old"])
    );
    assert_eq!(cache.get("a"), Some(&"new"));
}
