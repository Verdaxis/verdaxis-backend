//! Exact correctness counters, not load measurement. One instance must be
//! shared by ingress before Python/Rust splitting for the admin prefix.
use std::collections::HashMap;

#[derive(Clone, Default)]
pub struct SharedLimits(std::sync::Arc<std::sync::Mutex<Phase2Limits>>);

impl SharedLimits {
    pub fn preauth(&self, client: &str, path: &str, now: u64) -> bool {
        self.0
            .lock()
            .expect("quota authority lock")
            .preauth(client, path, now)
    }

    /// The one `/api/admin/` authority every ingress path draws from.
    pub fn admit_admin(&self, client: &str, now: u64) -> bool {
        self.0
            .lock()
            .expect("quota authority lock")
            .preauth(client, "/api/admin/", now)
    }

    /// Route adapters call this after bearer/current-account admission and
    /// typed validation. Invalid authentication never consumes endpoint quota.
    pub fn check_endpoint(
        &self,
        client: &str,
        path: &str,
        now: u64,
    ) -> Result<(), crate::ApiError> {
        if self
            .0
            .lock()
            .expect("quota authority lock")
            .endpoint(client, path, now)
        {
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

// Match the bounded Python backstop. Maintenance runs at most once per
// shortest policy window, never once per request under the shared mutex.
const MAX_BUCKETS: usize = 50_000;
const PRUNE_INTERVAL_SECONDS: u64 = 60;

struct Bucket {
    expires_at: u64,
    count: u32,
}

#[derive(Default)]
pub struct Phase2Limits {
    buckets: HashMap<(String, String), Bucket>,
    next_prune_at: u64,
}

impl Phase2Limits {
    fn accept(&mut self, client: &str, key: &str, limit: u32, window: u64, now: u64) -> bool {
        if now >= self.next_prune_at {
            self.buckets.retain(|_, bucket| now < bucket.expires_at);
            self.next_prune_at = now.saturating_add(PRUNE_INTERVAL_SECONDS);
        }
        let identity = (client.into(), key.into());
        if self.buckets.len() >= MAX_BUCKETS && !self.buckets.contains_key(&identity) {
            // Keep active accounting intact. New identities wait for the next
            // bounded maintenance pass rather than evicting another quota.
            return false;
        }
        let bucket = self.buckets.entry(identity).or_insert(Bucket {
            expires_at: now.saturating_add(window),
            count: 0,
        });
        if now >= bucket.expires_at {
            bucket.expires_at = now.saturating_add(window);
            bucket.count = 0;
        }
        if bucket.count >= limit {
            return false;
        }
        bucket.count += 1;
        true
    }
    pub fn preauth(&mut self, client: &str, path: &str, now: u64) -> bool {
        !path.starts_with("/api/admin/") || self.accept(client, "/api/admin/", 300, 60, now)
    }
    /// Call only after authentication and typed request validation succeeded.
    pub fn endpoint(&mut self, client: &str, path: &str, now: u64) -> bool {
        let Some((key, limit, window)) = endpoint_policy(path) else {
            return true;
        };
        self.accept(client, key, limit, window, now)
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

    #[test]
    fn distinct_clients_have_bounded_storage() {
        let mut limits = Phase2Limits::default();
        for client in 0..50_100 {
            let _ = limits.preauth(&format!("client-{client}"), "/api/admin/users", 100);
        }
        assert!(limits.buckets.len() <= 50_000);
    }

    #[test]
    fn endpoint_refusal_has_a_machine_readable_code_and_retry() {
        let limits = SharedLimits::default();
        for _ in 0..5 {
            assert!(limits
                .check_endpoint("client", "/api/feedback", 100)
                .is_ok());
        }
        let error = limits
            .check_endpoint("client", "/api/feedback", 100)
            .unwrap_err();
        assert_eq!(error.detail["code"], "RATE_LIMITED");
        assert!(error.detail["message"].is_string());
        assert_eq!(error.retry_after, Some(60));
    }

    #[test]
    fn capacity_admits_new_preauth_clients() {
        let mut limits = Phase2Limits::default();
        for client in 0..50_100_u32 {
            let address = std::net::Ipv4Addr::from(client).to_string();
            assert!(limits.preauth(&address, "/api/admin/users", 100));
        }
        assert!(limits.preauth("203.0.113.99", "/api/admin/users", 100));
    }

    #[test]
    fn preauth_capacity_does_not_consume_user_capacity() {
        let mut limits = Phase2Limits::default();
        for client in 0..50_100_u32 {
            let address = std::net::Ipv4Addr::from(client).to_string();
            let _ = limits.preauth(&address, "/api/admin/users", 100);
        }
        for _ in 0..5 {
            assert!(limits.endpoint("verified-user", "/api/feedback", 100));
        }
        assert!(!limits.endpoint("verified-user", "/api/feedback", 100));
        assert!(limits.endpoint("another-user", "/api/feedback", 100));
    }

    #[test]
    fn ipv6_clients_share_a_prefix_bucket() {
        let mut limits = Phase2Limits::default();
        for address in 0..1000_u32 {
            let admitted = limits.preauth(
                &format!("2001:db8:1:2::{address:x}"),
                "/api/admin/users",
                100,
            );
            assert_eq!(admitted, address < 300);
        }
        assert!(limits.preauth("2001:db8:1:3::1", "/api/admin/users", 100));
    }

    #[test]
    fn maintenance_expires_each_bucket_at_its_own_window() {
        let mut limits = Phase2Limits::default();
        assert!(limits.preauth("admin", "/api/admin/users", 100));
        assert!(limits.endpoint("inviter", "/api/referrals/invite", 100));
        assert!(limits.preauth("next", "/api/admin/users", 160));
        assert!(!limits
            .buckets
            .contains_key(&("admin".into(), "/api/admin/".into())));
        assert!(limits
            .buckets
            .contains_key(&("inviter".into(), "invite".into())));
    }
}
