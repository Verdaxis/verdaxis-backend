//! Stream token admission: the stream token profile and the current account
//! and organization state.
//!
//! The shared auth verifier checks the signature, issuer, audience, time claims
//! and the `stream` token type. This module adds the profile's required claims
//! (organization, environment and a bounded expiry) and re-reads the mutable
//! account and organization state. It holds no signing configuration and
//! accepts no other token type.

use std::fmt;

use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{
    AccountState, TokenIssuance, TokenType, VerificationSettings, VerifiedToken, cutoff_refusal,
    verify_token,
};

/// The longest stream token that is parsed. Longer input is refused before
/// any decoding.
pub const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// The account status and the organization verification status that admit a
/// private stream.
const APPROVED: &str = "APPROVED";

/// A verified stream identity. Only [`verify_stream`] creates one, so a
/// principal always comes from a stream token verified for this environment.
#[derive(Clone)]
pub struct StreamPrincipal {
    user_id: Uuid,
    organization_id: Uuid,
    expires_at: DateTime<Utc>,
    issuance: TokenIssuance,
}

impl StreamPrincipal {
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    /// The organization whose private channel this principal may read. It
    /// comes from the signed token, never from the user's current membership.
    pub fn organization_id(&self) -> Uuid {
        self.organization_id
    }

    /// The signed expiry. A stream must end at this instant.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// The issuance time that the account cutoffs compare against.
    pub fn issuance(&self) -> TokenIssuance {
        self.issuance
    }
}

impl fmt::Debug for StreamPrincipal {
    // Identifiers stay out of debug output and logs.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamPrincipal")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Bounded refusal classes. An HTTP owner maps a class to its status and code;
/// no claim value or database text is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    /// No token was supplied.
    TokenRequired,
    /// The token, or the current account or organization, does not admit the
    /// stream.
    TokenInvalid,
    /// The account store could not be read.
    AuthorizationUnavailable,
}

impl StreamError {
    pub fn status(self) -> u16 {
        match self {
            Self::TokenRequired | Self::TokenInvalid => 401,
            Self::AuthorizationUnavailable => 503,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::TokenRequired => "STREAM_TOKEN_REQUIRED",
            Self::TokenInvalid => "STREAM_TOKEN_INVALID",
            Self::AuthorizationUnavailable => "STREAM_AUTH_UNAVAILABLE",
        }
    }
}

/// Verify a stream token for the configured environment.
///
/// The result is known before any response header exists. Another token type,
/// a wrong key, issuer, audience or environment, a missing or wrongly typed
/// required claim, an expiry outside years 1 to 9999 and an expired token are
/// all `TokenInvalid`; nothing here can panic on claim content.
pub fn verify_stream(
    token: &str,
    auth: &VerificationSettings,
    environment: &str,
    now: DateTime<Utc>,
) -> Result<StreamPrincipal, StreamError> {
    if token.is_empty() {
        return Err(StreamError::TokenRequired);
    }
    if token.len() > MAX_TOKEN_BYTES {
        return Err(StreamError::TokenInvalid);
    }
    let verified =
        verify_token(token, auth, TokenType::Stream, now).map_err(|_| StreamError::TokenInvalid)?;
    // The issuer writes the configured environment in this normalized form.
    let environment = environment.trim().to_lowercase();
    if verified.claim("environment").and_then(Value::as_str) != Some(environment.as_str()) {
        return Err(StreamError::TokenInvalid);
    }
    Ok(StreamPrincipal {
        user_id: verified.subject(),
        organization_id: organization(&verified).ok_or(StreamError::TokenInvalid)?,
        expires_at: expiry(&verified).ok_or(StreamError::TokenInvalid)?,
        issuance: verified.issuance(),
    })
}

/// The `org_id` claim: a UUID string.
fn organization(verified: &VerifiedToken) -> Option<Uuid> {
    let organization = verified.claim("org_id")?.as_str()?;
    Uuid::parse_str(organization).ok()
}

/// The `exp` claim: an integer NumericDate in years 1 to 9999, the range of
/// the issuance times that the account cutoffs accept. The shared verifier has
/// already refused an expiry at or before `now`.
fn expiry(verified: &VerifiedToken) -> Option<DateTime<Utc>> {
    let seconds = verified.claim("exp")?.as_i64()?;
    DateTime::from_timestamp(seconds, 0).filter(|instant| (1..=9999).contains(&instant.year()))
}

/// Current account rules for a principal: the account exists, neither cutoff
/// covers the token's issuance (password cutoff first, both inclusive at
/// microsecond precision), the account is approved and needs no password
/// change. Every refusal is the same bounded class.
pub fn admit_account(
    principal: &StreamPrincipal,
    account: &AccountState,
) -> Result<(), StreamError> {
    let admitted = account.exists
        && cutoff_refusal(principal.issuance, account).is_none()
        && account.status == APPROVED
        && !account.must_change_password;
    if admitted {
        Ok(())
    } else {
        Err(StreamError::TokenInvalid)
    }
}

/// Organization rule: only a current `APPROVED` organization admits a private
/// stream. A missing organization is refused.
pub fn admit_organization(verification_status: Option<&str>) -> Result<(), StreamError> {
    if verification_status == Some(APPROVED) {
        Ok(())
    } else {
        Err(StreamError::TokenInvalid)
    }
}

/// The account, its current organization and the state of the token's
/// organization, read by one statement so all come from the same snapshot.
/// Reading the token's organization keeps membership and organization state
/// separate rules; an admitted account belongs to that organization.
const ADMISSION_STATE: &str = "SELECT u.organization_id, u.status::text AS status, \
     u.must_change_password, u.password_changed_at, u.authentication_revoked_at, \
     o.verification_status::text AS organization_status \
     FROM users u LEFT JOIN organizations o ON o.id = $2 \
     WHERE u.id = $1";

#[derive(sqlx::FromRow)]
struct AdmissionState {
    organization_id: Option<Uuid>,
    status: String,
    must_change_password: bool,
    password_changed_at: Option<DateTime<Utc>>,
    authentication_revoked_at: Option<DateTime<Utc>>,
    organization_status: Option<String>,
}

/// Re-read the current account and organization for a principal.
///
/// Used before response headers and again while a stream is open. It runs one
/// read statement and takes no lock. The caller owns the connection and its
/// deadlines and must not hold a transaction open while it waits for a client.
/// A user who moved to another organization is refused: a principal never
/// follows the user into a new organization. A failed read, including a value
/// that cannot be decoded, is `AuthorizationUnavailable`.
pub async fn revalidate(
    db: &mut PgConnection,
    principal: &StreamPrincipal,
) -> Result<(), StreamError> {
    let state: Option<AdmissionState> = sqlx::query_as(ADMISSION_STATE)
        .bind(principal.user_id)
        .bind(principal.organization_id)
        .fetch_optional(&mut *db)
        .await
        .map_err(|_| StreamError::AuthorizationUnavailable)?;
    let Some(state) = state else {
        return Err(StreamError::TokenInvalid);
    };
    if state.organization_id != Some(principal.organization_id) {
        return Err(StreamError::TokenInvalid);
    }
    admit_account(
        principal,
        &AccountState {
            exists: true,
            status: state.status,
            must_change_password: state.must_change_password,
            password_changed_at: state.password_changed_at.map(|at| at.timestamp_micros()),
            authentication_revoked_at: state
                .authentication_revoked_at
                .map(|at| at.timestamp_micros()),
        },
    )?;
    admit_organization(state.organization_status.as_deref())
}
