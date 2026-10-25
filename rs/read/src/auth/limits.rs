//! Exact correctness counters, not load measurement. One instance must be
//! shared by ingress before Python/Rust splitting for the admin prefix.
//!
//! Every limit is a fixed window per client and bucket key. Pre-auth buckets
//! are path prefixes checked before routing, so every path variant under a
//! prefix shares one counter. Endpoint buckets are checked by the route after
//! authentication and validation; an auth route's bucket ignores one trailing
//! slash, so both slash forms share one counter.
use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv6Addr};

use super::error::{AuthCode, AuthFailure};

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
pub struct SharedLimits(std::sync::Arc<std::sync::Mutex<QuotaStores>>);

impl SharedLimits {
    pub fn preauth(&self, client: &str, path: &str, now: u64) -> bool {
        self.preauth_check(client, path, now).is_ok()
    }

    /// The pre-auth check with the time until a refused window resets.
    pub fn preauth_check(&self, client: &str, path: &str, now: u64) -> Result<(), Refusal> {
        let Some((_, _, window)) = preauth_policy(path) else {
            return Ok(());
        };
        if self
            .0
            .lock()
            .expect("quota authority lock")
            .preauth(client, path, now)
        {
            Ok(())
        } else {
            Err(Refusal {
                retry_after_seconds: window.max(1),
            })
        }
    }

    /// The one `/api/admin/` authority every ingress path draws from.
    pub fn admit_admin(&self, client: &str, now: u64) -> bool {
        self.0
            .lock()
            .expect("quota authority lock")
            .preauth(client, "/api/admin/", now)
    }

    /// Endpoint quota for an address-keyed route (unauthenticated auth
    /// routes). It draws from the bounded, evicting address store, so a flood
    /// of distinct addresses can never fill the non-evicting verified-user
    /// store. The user store keeps only verified user IDs.
    pub fn check_endpoint_by_address(
        &self,
        client: &str,
        path: &str,
        now: u64,
    ) -> Result<(), crate::ApiError> {
        let check = self
            .0
            .lock()
            .expect("quota authority lock")
            .endpoint_check_by_address(client, path, now);
        endpoint_refusal(path, check)
    }
    /// Route adapters call this after bearer/current-account admission and
    /// typed validation. Invalid authentication never consumes endpoint quota.
    /// The bucket owner is the verified user ID.
    ///
    /// An auth route's refusal is the auth envelope with `Retry-After`; the
    /// Phase 2 routes keep their existing refusal body.
    pub fn check_endpoint(
        &self,
        user: &str,
        path: &str,
        now: u64,
    ) -> Result<(), crate::ApiError> {
        let check = self
            .0
            .lock()
            .expect("quota authority lock")
            .endpoint_check(user, path, now);
        endpoint_refusal(path, check)
    }
}

/// The one refusal envelope for both endpoint paths: the auth envelope with
/// the remaining seconds for an auth route, the shared code/message object
/// with `Retry-After` for the business routes. Building it here keeps the
/// address-keyed and user-keyed refusals from drifting apart.
fn endpoint_refusal(path: &str, check: Result<(), Refusal>) -> Result<(), crate::ApiError> {
    let Err(refusal) = check else {
        return Ok(());
    };
    if auth_endpoint_policy(path).is_some() {
        return Err(refusal.auth_failure().into());
    }
    let (_, limit, window) = endpoint_policy(path).expect("limited endpoint");
    let unit = if window == 3600 { "hour" } else { "minute" };
    Err(crate::ApiError {
        status: axum::http::StatusCode::TOO_MANY_REQUESTS,
        detail: serde_json::json!({
            "code": "RATE_LIMITED",
            "message": format!("Rate limit exceeded: {limit} per 1 {unit}"),
        }),
        retry_after: Some(refusal.retry_after_seconds as u32),
        kind: crate::ApiErrorKind::Expected,
    })
}

// Each authority has its own cap. The ordered expiration index keeps eviction
// and expiry bounded without scanning the client map on each request.
const MAX_BUCKETS: usize = 50_000;

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
        self.accept_check(owner, key, limit, window, now, evict)
            .is_ok()
    }

    /// The refused window's remaining seconds, at least one.
    fn accept_check(
        &mut self,
        owner: &str,
        key: &str,
        limit: u32,
        window: u64,
        now: u64,
        evict: bool,
    ) -> Result<(), u64> {
        while let Some((expires_at, _)) = self.expiration.first() {
            if *expires_at > now {
                break;
            }
            let (_, identity) = self.expiration.pop_first().expect("expiration entry");
            self.buckets.remove(&identity);
        }
        let identity = (owner.into(), key.into());
        if !self.buckets.contains_key(&identity) {
            if self.buckets.len() >= MAX_BUCKETS {
                if !evict {
                    return Err(window.max(1));
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
            return Err(bucket.expires_at.saturating_sub(now).max(1));
        }
        bucket.count += 1;
        Ok(())
    }
}

#[derive(Default)]
pub struct QuotaStores {
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
        // A value that is not an address is still its own pre-auth identity.
        // Collapsing every such value into one bucket would let one client
        // spend another's budget; only the middleware's own literal "unknown"
        // groups requests that had no peer address at all.
        Err(_) => client.to_string(),
    }
}

impl QuotaStores {
    /// Pre-auth admission by client address. The bounded store evicts the
    /// soonest-expiring entry rather than refusing a new client under a flood.
    pub fn preauth(&mut self, client: &str, path: &str, now: u64) -> bool {
        let Some((key, limit, window)) = preauth_policy(path) else {
            return true;
        };
        self.preauth_store
            .accept(&network_identity(client), key, limit, window, now, true)
    }

    /// Endpoint admission by verified user ID. The user store never evicts, so
    /// one account cannot displace another account's accounting.
    pub fn endpoint(&mut self, user: &str, path: &str, now: u64) -> bool {
        let Some((key, limit, window)) = endpoint_policy(path) else {
            return true;
        };
        self.user_store.accept(user, key, limit, window, now, false)
    }

    /// The Phase 3 auth routes use this refusal-with-retry form; it shares the
    /// same bounded pre-auth store and policy as [`QuotaStores::preauth`].
    pub fn preauth_check(&mut self, client: &str, path: &str, now: u64) -> Result<(), Refusal> {
        let Some((key, limit, window)) = preauth_policy(path) else {
            return Ok(());
        };
        self.preauth_store
            .accept_check(&network_identity(client), key, limit, window, now, true)
            .map_err(|retry_after_seconds| Refusal { retry_after_seconds })
    }

    /// The Phase 3 endpoint form with the same user-keyed store as
    /// [`QuotaStores::endpoint`].
    pub fn endpoint_check(&mut self, user: &str, path: &str, now: u64) -> Result<(), Refusal> {
        let Some((key, limit, window)) = endpoint_policy(path) else {
            return Ok(());
        };
        self.user_store
            .accept_check(user, key, limit, window, now, false)
            .map_err(|retry_after_seconds| Refusal { retry_after_seconds })
    }

    /// Endpoint admission keyed by client address in the evicting address
    /// store; used by unauthenticated auth routes whose only identity is the
    /// peer address. Verified-user routes never call this. The refused window's
    /// remaining seconds travel with the refusal.
    fn endpoint_check_by_address(
        &mut self,
        client: &str,
        path: &str,
        now: u64,
    ) -> Result<(), Refusal> {
        let Some((key, limit, window)) = endpoint_policy(path) else {
            return Ok(());
        };
        self.preauth_store
            .accept_check(&network_identity(client), key, limit, window, now, true)
            .map_err(|retry_after_seconds| Refusal { retry_after_seconds })
    }

    /// Pre-auth evictions since start. A plain number: it names no client.
    pub fn preauth_evictions(&self) -> u64 {
        self.preauth_store.evictions
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
