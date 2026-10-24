//! Exact correctness counters, not load measurement. One instance must be
//! shared by ingress before Python/Rust splitting for the admin prefix.
//!
//! Every limit is a fixed window per client and bucket key. Pre-auth buckets
//! are path prefixes checked before routing, so every path variant under a
//! prefix shares one counter. Endpoint buckets are checked by the route after
//! authentication and validation; an auth route's bucket ignores one trailing
//! slash, so both slash forms share one counter.
use std::collections::HashMap;

use super::error::{AuthCode, AuthFailure};

/// Buckets older than this are dropped; it is the longest window in use.
const BUCKET_RETENTION_SECONDS: u64 = 3600;

/// A refused request and the seconds until its window resets (at least 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub retry_after_seconds: u64,
}

impl Refusal {
    /// The auth envelope: 429 `RATE_LIMITED` with `Retry-After`.
    pub fn auth_failure(self) -> AuthFailure {
        let seconds = u32::try_from(self.retry_after_seconds).unwrap_or(u32::MAX);
        AuthFailure::new(AuthCode::RateLimited).with_retry_after(seconds)
    }
}

#[derive(Clone, Default)]
pub struct SharedLimits(std::sync::Arc<std::sync::Mutex<Phase2Limits>>);

impl SharedLimits {
    pub fn preauth(&self, client: &str, path: &str, now: u64) -> bool {
        self.preauth_check(client, path, now).is_ok()
    }

    /// The pre-auth check with the time until a refused window resets.
    pub fn preauth_check(&self, client: &str, path: &str, now: u64) -> Result<(), Refusal> {
        self.0
            .lock()
            .expect("quota authority lock")
            .preauth_check(client, path, now)
    }

    /// Route adapters call this after bearer/current-account admission and
    /// typed validation. Invalid authentication never consumes endpoint quota.
    ///
    /// An auth route's refusal is the auth envelope with `Retry-After`; the
    /// Phase 2 routes keep their existing refusal body.
    pub fn check_endpoint(
        &self,
        client: &str,
        path: &str,
        now: u64,
    ) -> Result<(), crate::ApiError> {
        let Err(refusal) = self
            .0
            .lock()
            .expect("quota authority lock")
            .endpoint_check(client, path, now)
        else {
            return Ok(());
        };
        if auth_endpoint_policy(path).is_some() {
            return Err(refusal.auth_failure().into());
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
    fn admit(
        &mut self,
        client: &str,
        key: &str,
        limit: u32,
        window: u64,
        now: u64,
    ) -> Result<(), Refusal> {
        self.buckets
            .retain(|_, (started, _)| now.saturating_sub(*started) < BUCKET_RETENTION_SECONDS);
        let bucket = self
            .buckets
            .entry((client.into(), key.into()))
            .or_insert((now, 0));
        if now.saturating_sub(bucket.0) >= window {
            *bucket = (now, 0);
        }
        if bucket.1 >= limit {
            let elapsed = now.saturating_sub(bucket.0);
            return Err(Refusal {
                retry_after_seconds: window.saturating_sub(elapsed).max(1),
            });
        }
        bucket.1 += 1;
        Ok(())
    }

    pub fn preauth(&mut self, client: &str, path: &str, now: u64) -> bool {
        self.preauth_check(client, path, now).is_ok()
    }

    pub fn preauth_check(&mut self, client: &str, path: &str, now: u64) -> Result<(), Refusal> {
        match preauth_policy(path) {
            Some((prefix, limit, window)) => self.admit(client, prefix, limit, window, now),
            None => Ok(()),
        }
    }

    /// Call only after authentication and typed request validation succeeded.
    pub fn endpoint(&mut self, client: &str, path: &str, now: u64) -> bool {
        self.endpoint_check(client, path, now).is_ok()
    }

    pub fn endpoint_check(&mut self, client: &str, path: &str, now: u64) -> Result<(), Refusal> {
        match endpoint_policy(path) {
            Some((key, limit, window)) => self.admit(client, key, limit, window, now),
            None => Ok(()),
        }
    }
}

/// Pre-auth prefixes: (prefix, requests per window, window seconds). The first
/// matching prefix is the bucket and its key. `/api/auth/register` also covers
/// `/api/auth/register-with-org`, so both registration routes share one
/// pre-auth bucket.
const PREAUTH_POLICIES: &[(&str, u32, u64)] = &[
    ("/api/admin/", 300, 60),
    ("/api/auth/login", 30, 60),
    ("/api/auth/forgot-password", 15, 60),
    ("/api/auth/register", 30, 60),
];

pub fn preauth_policy(path: &str) -> Option<(&'static str, u32, u64)> {
    PREAUTH_POLICIES
        .iter()
        .find(|(prefix, _, _)| path.starts_with(prefix))
        .copied()
}

/// Per-client endpoint limits of the Phase 3 auth routes: (route path, bucket
/// key, requests per minute). The keys differ from every pre-auth prefix, so a
/// request counts once in each layer. Logout has no endpoint limit.
const AUTH_ENDPOINT_POLICIES: &[(&str, &str, u32)] = &[
    ("/api/auth/login", "auth-login", 5),
    ("/api/auth/refresh", "auth-refresh", 10),
    ("/api/auth/stream-token", "auth-stream-token", 30),
    ("/api/auth/me/password", "auth-password-change", 3),
    ("/api/auth/invitations/accept", "auth-invitation-accept", 5),
    ("/api/auth/forgot-password", "auth-forgot-password", 3),
    ("/api/auth/reset-password", "auth-reset-password", 5),
    ("/api/auth/verify-email", "auth-verify-email", 10),
    (
        "/api/auth/resend-verification-email",
        "auth-resend-verification",
        3,
    ),
    ("/api/auth/register", "auth-register", 5),
    ("/api/auth/register-with-org", "auth-register-with-org", 5),
];

/// The auth endpoint window: one minute.
const AUTH_ENDPOINT_WINDOW_SECONDS: u64 = 60;

fn auth_endpoint_policy(path: &str) -> Option<(&'static str, u32, u64)> {
    let route = path.strip_suffix('/').unwrap_or(path);
    AUTH_ENDPOINT_POLICIES
        .iter()
        .find(|(candidate, _, _)| *candidate == route)
        .map(|(_, key, limit)| (*key, *limit, AUTH_ENDPOINT_WINDOW_SECONDS))
}

pub fn endpoint_policy(path: &str) -> Option<(&'static str, u32, u64)> {
    if let Some(policy) = auth_endpoint_policy(path) {
        return Some(policy);
    }
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
