use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug)]
pub struct VerifiedAccess {
    pub subject: Uuid,
    pub issued_at: Option<DateTime<Utc>>,
    pub(crate) issuance_out_of_range: bool,
}

/// A current database account projection, never a JWT role/status projection.
/// Task 6 must load this under the same user-row lock as the route transaction.
#[derive(Debug, Deserialize)]
pub struct AccountState {
    pub exists: bool,
    pub status: String,
    pub must_change_password: bool,
    pub password_changed_at: Option<i64>,
    pub authentication_revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    MissingBearer,
    Credentials,
    PasswordChanged,
    SessionRevoked,
    Pending,
    Rejected,
    PasswordChangeRequired,
    /// Python's out-of-range timestamp conversion is an unmapped HTTP 500.
    /// Keep it visible; it blocks affected cutover pending source disposition.
    IssuanceOutOfRange,
}

impl AuthError {
    pub fn status(self) -> u16 {
        match self {
            Self::Pending | Self::Rejected | Self::PasswordChangeRequired => 403,
            Self::IssuanceOutOfRange => 500,
            _ => 401,
        }
    }

    pub fn www_authenticate(self) -> Option<&'static str> {
        (self.status() == 401).then_some("Bearer")
    }

    pub fn body(self) -> Value {
        let detail = match self {
            Self::MissingBearer => json!("Not authenticated"),
            Self::Credentials => json!("Could not validate credentials"),
            Self::PasswordChanged => {
                json!({"code": "AUTH_PASSWORD_CHANGED", "message": "Password was changed; sign in again"})
            }
            Self::SessionRevoked => {
                json!({"code": "AUTH_SESSION_REVOKED", "message": "Session was revoked; sign in again"})
            }
            Self::Pending => json!("Account is PENDING. Please wait for admin approval."),
            Self::Rejected => json!("Account is REJECTED. Please wait for admin approval."),
            Self::PasswordChangeRequired => json!("Password change required."),
            Self::IssuanceOutOfRange => return json!("Internal Server Error"),
        };
        json!({"detail": detail})
    }
}

pub fn validate_account(access: &VerifiedAccess, user: &AccountState) -> Result<(), AuthError> {
    if !user.exists {
        return Err(AuthError::Credentials);
    }
    for (cutoff, error) in [
        (user.password_changed_at, AuthError::PasswordChanged),
        (user.authentication_revoked_at, AuthError::SessionRevoked),
    ] {
        if let Some(cutoff) = cutoff {
            if access.issuance_out_of_range {
                return Err(AuthError::IssuanceOutOfRange);
            }
            if access
                .issued_at
                .is_none_or(|issued| issued.timestamp_micros() <= cutoff)
            {
                return Err(error);
            }
        }
    }
    match user.status.as_str() {
        "APPROVED" => {}
        "PENDING" => return Err(AuthError::Pending),
        "REJECTED" => return Err(AuthError::Rejected),
        _ => return Err(AuthError::Credentials),
    }
    if user.must_change_password {
        return Err(AuthError::PasswordChangeRequired);
    }
    Ok(())
}
