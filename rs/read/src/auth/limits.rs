//! Exact correctness counters, not load measurement. One instance must be
//! shared by ingress before Python/Rust splitting for the admin prefix.
use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv6Addr};

#[derive(Clone, Default)]
pub struct SharedLimits(std::sync::Arc<std::sync::Mutex<Phase2Limits>>);

impl SharedLimits {
    /// One lock acquisition that recovers from poisoning instead of
    /// panicking: a caller that panicked under the lock must not take the
    /// quota authority down with it. The counters are kept as they were —
    /// clearing them would grant fresh quota — and the poison flag is cleared
    /// while holding the recovered guard, so only that one recovery warns,
    /// with a plain count that names no client.
    fn lock_recovering(&self) -> std::sync::MutexGuard<'_, Phase2Limits> {
        match self.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                // Clear the poison flag while holding the recovered guard so
                // only this recovery warns, not every later request.
                self.0.clear_poison();
                tracing::warn!(recoveries = 1_u64, "quota authority lock recovery");
                guard
            }
        }
    }

    pub fn preauth(&self, client: &str, path: &str, now: u64) -> bool {
        self.lock_recovering().preauth(client, path, now)
    }

    /// The one `/api/admin/` authority every ingress path draws from.
    pub fn admit_admin(&self, client: &str, now: u64) -> bool {
        self.lock_recovering().preauth(client, "/api/admin/", now)
    }

    /// Route adapters call this after bearer/current-account admission and
    /// typed validation. Invalid authentication never consumes endpoint quota.
    pub fn check_endpoint(
        &self,
        client: &str,
        path: &str,
        now: u64,
    ) -> Result<(), crate::ApiError> {
        if self.lock_recovering().endpoint(client, path, now) {
            return Ok(());
        }
        let (_, limit, window) = endpoint_policy(path).expect("limited endpoint");
        let unit = if window == 3600 { "hour" } else { "minute" };
        Err(crate::ApiError {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            detail: serde_json::json!({
                "code": "RATE_LIMITED",
                "message": format!("Rate limit exceeded: {limit} per 1 {unit}"),
            }),
            retry_after: Some(window as u32),
            kind: crate::ApiErrorKind::Expected,
        })
    }
}

// Each authority has its own cap. The ordered expiration index keeps eviction
// and expiry bounded without scanning the client map on each request.
const MAX_BUCKETS: usize = 50_000;

// At 32 entries per request, stale accounting drains faster than new clients
// can add it, without making one caller release all 50,000 expired entries.
const EXPIRED_CLEANUP_BUDGET: usize = 32;

type Identity = (String, String);

struct Bucket {
    expires_at: u64,
    count: u32,
}

#[derive(Default)]
struct QuotaStore {
    buckets: HashMap<Identity, Bucket>,
    expiration: BTreeSet<(u64, Identity)>,
    evictions: u64,
}

impl QuotaStore {
    fn accept(
        &mut self,
        owner: &str,
        key: &str,
        limit: u32,
        window: u64,
        now: u64,
        evict: bool,
    ) -> bool {
        // The caller's own expired accounting resets directly, without waiting
        // for the bounded cleanup below to reach it.
        let identity = (owner.into(), key.into());
        if let Some(bucket) = self.buckets.get(&identity)
            && bucket.expires_at <= now
        {
            self.expiration
                .remove(&(bucket.expires_at, identity.clone()));
            self.buckets.remove(&identity);
        }
        // A small fixed batch bounds synchronous cleanup under the shared
        // lock: stale accounting drains faster than new identities can add
        // it, without one caller releasing every expired entry at once.
        for _ in 0..EXPIRED_CLEANUP_BUDGET {
            let Some((expires_at, _)) = self.expiration.first() else {
                break;
            };
            if *expires_at > now {
                break;
            }
            let (_, expired) = self.expiration.pop_first().expect("expiration entry");
            self.buckets.remove(&expired);
        }
        if !self.buckets.contains_key(&identity) {
            if self.buckets.len() >= MAX_BUCKETS {
                if !evict {
                    return false;
                }
                // Only the pre-auth store evicts (PM decision PD2-SEC-1). Every
                // entry shares one window length, so the soonest-expiring entry
                // is the oldest accounting. Refusing the new client instead
                // would let a flood of fresh addresses lock out every
                // legitimate client, administrators included. The trade is
                // deliberate: under sustained churn a client can be evicted and
                // regain budget. A warn-level count makes such a flood visible;
                // it never names the evicted client.
                let (_, expired) = self.expiration.pop_first().expect("full quota index");
                self.buckets.remove(&expired);
                self.evictions = self.evictions.saturating_add(1);
                tracing::warn!(evictions = self.evictions, "preauth quota eviction");
            }
            let expires_at = now.saturating_add(window);
            self.expiration.insert((expires_at, identity.clone()));
            self.buckets.insert(
                identity.clone(),
                Bucket {
                    expires_at,
                    count: 0,
                },
            );
        }
        let bucket = self.buckets.get_mut(&identity).expect("quota entry");
        debug_assert!(bucket.expires_at > now);
        if bucket.count >= limit {
            return false;
        }
        bucket.count += 1;
        true
    }
}

#[derive(Default)]
pub struct Phase2Limits {
    preauth_store: QuotaStore,
    user_store: QuotaStore,
}

fn network_identity(client: &str) -> String {
    match client.parse::<IpAddr>() {
        Ok(IpAddr::V6(address)) => {
            if let Some(ipv4) = address.to_ipv4_mapped() {
                return ipv4.to_string();
            }
            let prefix = u128::from(address) & (u128::MAX << 64);
            Ipv6Addr::from(prefix).to_string()
        }
        Ok(IpAddr::V4(address)) => address.to_string(),
        Err(_) => "unknown".into(),
    }
}

impl Phase2Limits {
    pub fn preauth(&mut self, client: &str, path: &str, now: u64) -> bool {
        !path.starts_with("/api/admin/")
            || self.preauth_store.accept(
                &network_identity(client),
                "/api/admin/",
                300,
                60,
                now,
                true,
            )
    }
    /// Pre-auth evictions since start. A plain number: it names no client.
    pub fn preauth_evictions(&self) -> u64 {
        self.preauth_store.evictions
    }
    /// The owner is the current verified user ID, never a transport address.
    pub fn endpoint(&mut self, user: &str, path: &str, now: u64) -> bool {
        let Some((key, limit, window)) = endpoint_policy(path) else {
            return true;
        };
        self.user_store.accept(user, key, limit, window, now, false)
    }
}

pub fn endpoint_policy(path: &str) -> Option<(&'static str, u32, u64)> {
    if path.starts_with("/api/users/me/preferences/") {
        return Some(("preferences", 30, 60));
    }
    match path {
        "/api/feedback" => Some(("feedback", 5, 60)),
        "/api/admin/feedback" => Some(("admin-feedback", 60, 60)),
        "/api/referrals/invite" => Some(("invite", 10, 3600)),
        _ => None,
    }
}

/// Decision 96 rate-limited correctness/evidence floor, with seconds as window.
pub fn sample_floor(limit_per_minute: f64, window_seconds: f64) -> u64 {
    (0.95 * limit_per_minute * window_seconds / 60.0)
        .floor()
        .clamp(0.0, 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// A full table drops exactly one entry, the soonest-expiring one, and
    /// leaves every other client's accounting as it was. It cannot drop the
    /// client that triggered the eviction: that identity is not stored yet.
    #[test]
    fn eviction_drops_only_the_soonest_expiring_entry() {
        let mut limits = Phase2Limits::default();
        for client in 0..MAX_BUCKETS as u32 {
            let address = Ipv4Addr::from(client).to_string();
            assert!(limits.preauth(&address, "/api/admin/users", 100));
        }
        assert_eq!(limits.preauth_evictions(), 0);
        // A late client spends its whole minute budget. Its first request is a
        // new identity in a full table, so it evicts exactly one entry.
        for _ in 0..300 {
            assert!(limits.preauth("203.0.113.7", "/api/admin/users", 100));
        }
        assert!(!limits.preauth("203.0.113.7", "/api/admin/users", 100));
        assert_eq!(limits.preauth_evictions(), 1);
        let evicted = ("0.0.0.0".to_string(), "/api/admin/".to_string());
        // Ten more new addresses displace the next ten oldest entries, not the
        // spent client, which keeps its refusal.
        for client in MAX_BUCKETS as u32..MAX_BUCKETS as u32 + 10 {
            let address = Ipv4Addr::from(client).to_string();
            assert!(limits.preauth(&address, "/api/admin/users", 100));
        }
        assert_eq!(limits.preauth_evictions(), 11);
        assert!(!limits.preauth_store.buckets.contains_key(&evicted));
        assert!(
            limits
                .preauth_store
                .buckets
                .contains_key(&("203.0.113.7".to_string(), "/api/admin/".to_string()))
        );
        assert!(!limits.preauth("203.0.113.7", "/api/admin/users", 100));
    }

    /// A caller whose own bucket expired is reset directly, even while the
    /// bounded cleanup of other expired entries still has work left. The
    /// caller must never wait for the whole expiration index to drain.
    #[test]
    fn an_expired_caller_resets_directly_while_cleanup_stays_budgeted() {
        let mut store = QuotaStore::default();
        let window = 60;
        // A flood of identities whose accounting all expires at t=100; one
        // slot stays free for the caller, which this store never evicts.
        for index in 0..MAX_BUCKETS as u32 - 1 {
            assert!(store.accept(&format!("user-{index}"), "k", 1, window, 40, false));
        }
        // The caller spends its whole window budget, and that window expires
        // with the flood.
        assert!(store.accept("caller", "k", 1, window, 40, false));
        assert!(!store.accept("caller", "k", 1, window, 99, false));
        // One request at t=101: the caller's own expired bucket is gone, so it
        // is admitted again, while at most EXPIRED_CLEANUP_BUDGET other expired
        // entries were removed around it.
        let before = store.buckets.len();
        assert!(store.accept("caller", "k", 1, window, 101, false));
        let removed = before - store.buckets.len();
        assert!(removed <= EXPIRED_CLEANUP_BUDGET + 1, "removed {removed}");
        // Most of the flood is still stored; the next request removes only
        // another bounded batch, never the rest at once. The caller's fresh
        // window is spent (limit 1), so its refusal changes no accounting.
        let before = store.buckets.len();
        assert!(!store.accept("caller", "k", 1, window, 101, false));
        assert!(before - store.buckets.len() <= EXPIRED_CLEANUP_BUDGET);
    }

    /// One request never drains an unbounded number of expired entries: after a
    /// full flood expires at once, each further request removes at most the
    /// cleanup budget, so the work under the shared lock stays small.
    #[test]
    fn expired_cleanup_after_a_full_flood_removes_a_bounded_batch() {
        let mut store = QuotaStore::default();
        for index in 0..MAX_BUCKETS as u32 {
            assert!(store.accept(&format!("user-{index}"), "k", 1, 60, 40, false));
        }
        assert_eq!(store.buckets.len(), MAX_BUCKETS);
        // All accounting expires at t=100. One request at t=200 removes at most
        // the cleanup budget plus the caller's own new entry.
        assert!(store.accept("fresh", "k", 1, 60, 200, false));
        assert!(
            store.buckets.len() >= MAX_BUCKETS - EXPIRED_CLEANUP_BUDGET,
            "one request removed too much: {}",
            store.buckets.len()
        );
        let mut previous = store.buckets.len();
        for round in 0..5 {
            assert!(store.accept("fresh", "k", 10, 60, 200 + round, false));
            let now = store.buckets.len();
            assert!(previous - now <= EXPIRED_CLEANUP_BUDGET);
            previous = now;
        }
    }

    /// A poisoned quota lock recovers instead of panicking: the counter state
    /// is re-read and the spent quotas of other identities are preserved. The
    /// recovery warns once with a count that names no client.
    #[test]
    fn a_poisoned_lock_recovers_and_keeps_spent_quotas() {
        let limits = SharedLimits::default();
        assert!(limits.preauth("203.0.113.9", "/api/admin/users", 100));
        for _ in 0..300 {
            assert!(limits.preauth("203.0.113.10", "/api/admin/users", 100));
        }
        assert!(!limits.preauth("203.0.113.10", "/api/admin/users", 100));
        // A panicking caller poisons the mutex.
        let inner = limits.0.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = inner.lock().expect("quota authority lock");
            panic!("caller panic under the quota lock");
        }));
        assert!(result.is_err());
        assert!(inner.is_poisoned());
        // The recovered accounting still refuses the spent client and admits a
        // fresh one; nothing was reset by the poison.
        assert!(!limits.preauth("203.0.113.10", "/api/admin/users", 100));
        assert!(limits.preauth("198.51.100.1", "/api/admin/users", 100));
    }

    /// Both stores keep every bucket and every expiration index entry under the
    /// same cap; an eviction replaces one entry instead of growing the map.
    #[test]
    fn both_stores_stay_bounded() {
        let mut limits = Phase2Limits::default();
        for client in 0..50_100_u32 {
            let address = Ipv4Addr::from(client).to_string();
            let _ = limits.preauth(&address, "/api/admin/users", 100);
        }
        for user in 0..50_100_u32 {
            let _ = limits.endpoint(&format!("user-{user}"), "/api/feedback", 100);
        }
        assert_eq!(limits.preauth_store.buckets.len(), MAX_BUCKETS);
        assert_eq!(limits.preauth_store.expiration.len(), MAX_BUCKETS);
        assert_eq!(limits.user_store.buckets.len(), MAX_BUCKETS);
        assert_eq!(limits.user_store.expiration.len(), MAX_BUCKETS);
    }
}
