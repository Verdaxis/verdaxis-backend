//! Access, refresh and stream token issuance.
//!
//! The issuer holds the current signing key only; verification stays in
//! `token.rs`. Every claim set here is the one the Python stack verifies, so a
//! token minted by either stack works during the migration (coexistence C1).

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use uuid::Uuid;

/// Access tokens live 15 minutes, matching the Python setting.
pub const ACCESS_TOKEN_MINUTES: i64 = 15;
/// Refresh tokens live 7 days, matching the Python setting.
pub const REFRESH_TOKEN_DAYS: i64 = 7;
/// Stream tokens live 60 seconds; the stream routes re-check state per event.
pub const STREAM_TOKEN_SECONDS: i64 = 60;

pub struct TokenIssuer {
    key: EncodingKey,
    issuer: String,
    audience: String,
}

impl TokenIssuer {
    pub fn new(current_secret: &str, issuer: &str, audience: &str) -> Self {
        Self {
            key: EncodingKey::from_secret(current_secret.as_bytes()),
            issuer: issuer.to_owned(),
            audience: audience.to_owned(),
        }
    }

    /// A 15-minute access token. `issued_after` moves the issuance time one
    /// microsecond past a session cutoff so the token is not immediately
    /// refused by the inclusive cutoff comparison.
    pub fn access(
        &self,
        subject: Uuid,
        now: DateTime<Utc>,
        issued_after: Option<DateTime<Utc>>,
    ) -> String {
        let issued_at = issue_time(now, issued_after);
        self.sign(json!({
            "sub": subject.to_string(),
            "exp": (issued_at + Duration::minutes(ACCESS_TOKEN_MINUTES)).timestamp(),
            "type": "access",
            "iat": issued_at.timestamp(),
            "iat_us": microseconds(issued_at),
            "iss": self.issuer.as_str(),
            "aud": self.audience.as_str(),
        }))
    }

    /// A 7-day refresh token. `jti` is the 43-character token identifier whose
    /// SHA-256 hex digest is the stored `jti_hash`.
    pub fn refresh(
        &self,
        subject: Uuid,
        family_id: Uuid,
        jti: &str,
        now: DateTime<Utc>,
        issued_after: Option<DateTime<Utc>>,
    ) -> String {
        let issued_at = issue_time(now, issued_after);
        self.sign(json!({
            "sub": subject.to_string(),
            "exp": (issued_at + Duration::days(REFRESH_TOKEN_DAYS)).timestamp(),
            "type": "refresh",
            "iat": issued_at.timestamp(),
            "iat_us": microseconds(issued_at),
            "jti": jti,
            "family_id": family_id.to_string(),
            "iss": self.issuer.as_str(),
            "aud": self.audience.as_str(),
        }))
    }

    /// A 60-second stream token for SSE query-parameter authentication.
    pub fn stream(
        &self,
        subject: Uuid,
        organization: Uuid,
        environment: &str,
        now: DateTime<Utc>,
    ) -> String {
        let environment = environment.trim().to_lowercase();
        self.sign(json!({
            "sub": subject.to_string(),
            "org_id": organization.to_string(),
            "environment": environment,
            "exp": (now + Duration::seconds(STREAM_TOKEN_SECONDS)).timestamp(),
            "type": "stream",
            "iat": now.timestamp(),
            "iat_us": microseconds(now),
            "iss": self.issuer.as_str(),
            "aud": self.audience.as_str(),
        }))
    }

    fn sign(&self, claims: Value) -> String {
        encode(&Header::new(Algorithm::HS256), &claims, &self.key)
            .expect("HS256 claims with a UUID subject always serialize")
    }
}

impl fmt::Debug for TokenIssuer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenIssuer")
            .field("key", &"[REDACTED]")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish()
    }
}

fn issue_time(now: DateTime<Utc>, issued_after: Option<DateTime<Utc>>) -> DateTime<Utc> {
    match issued_after {
        Some(cutoff) if now <= cutoff => cutoff + Duration::microseconds(1),
        _ => now,
    }
}

fn microseconds(value: DateTime<Utc>) -> i64 {
    value.timestamp_micros()
}
