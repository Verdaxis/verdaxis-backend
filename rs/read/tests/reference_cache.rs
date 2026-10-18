use std::time::{Duration, Instant};
use verdaxis_read::runtime::cache::ReferenceCache;

#[test]
fn reference_expiry_is_monotonic_and_never_stale() {
    let start = Instant::now();
    let cache = ReferenceCache::new(2, 100);
    cache.insert("a", vec![1, 2], start);
    assert_eq!(
        cache.get(&"a", start + Duration::from_secs(59)),
        Some(vec![1, 2])
    );
    assert_eq!(cache.get(&"a", start + Duration::from_secs(60)), None);
}

#[test]
fn reference_budget_eviction_and_clear_are_bounded() {
    let now = Instant::now();
    let cache = ReferenceCache::new(2, 4);
    cache.insert("a", vec![1, 2], now);
    cache.insert("b", vec![3, 4], now);
    cache.insert("c", vec![5, 6], now);
    assert!(cache.get(&"a", now).is_none());
    assert!(cache.get(&"c", now).is_some());
    cache.insert("oversized", vec![0; 5], now);
    assert!(cache.get(&"oversized", now).is_none());
    cache.clear();
    assert!(cache.get(&"c", now).is_none());
}

#[test]
fn concurrent_fills_keep_keys_isolated() {
    let cache = std::sync::Arc::new(ReferenceCache::new(32, 1024));
    let threads: Vec<_> = (0..8)
        .map(|key| {
            let cache = cache.clone();
            std::thread::spawn(move || cache.insert(key, vec![key], Instant::now()))
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    for key in 0..8 {
        assert_eq!(cache.get(&key, Instant::now()), Some(vec![key]));
    }
}
