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
            detail: serde_json::json!(format!("Rate limit exceeded: {limit} per 1 {unit}")),
            retry_after: None,
            kind: crate::ApiErrorKind::Expected,
        })
    }
}

#[derive(Default)]
pub struct Phase2Limits {
    buckets: HashMap<(String, String), (u64, u32)>,
}

impl Phase2Limits {
    fn accept(&mut self, client: &str, key: &str, limit: u32, window: u64, now: u64) -> bool {
        self.buckets
            .retain(|_, (started, _)| now.saturating_sub(*started) < 3600);
        let bucket = self
            .buckets
            .entry((client.into(), key.into()))
            .or_insert((now, 0));
        if now.saturating_sub(bucket.0) >= window {
            *bucket = (now, 0);
        }
        if bucket.1 >= limit {
            return false;
        }
        bucket.1 += 1;
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
