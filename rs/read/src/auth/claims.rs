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

/// A token's issuance time, read from `iat_us` (preferred) or `iat`.
///
/// `out_of_range` marks a value that cannot be represented as a time. It is a
/// refusal under an active cutoff, never a panic or a 500 (PD7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenIssuance {
    pub issued_at: Option<DateTime<Utc>>,
    pub out_of_range: bool,
}

impl TokenIssuance {
    const OUT_OF_RANGE: Self = Self {
        issued_at: None,
        out_of_range: true,
    };

    fn from_microseconds(microseconds: i64) -> Self {
        let issued_at = DateTime::from_timestamp_micros(microseconds)
            .filter(|value| (1..=9999).contains(&chrono::Datelike::year(value)));
        Self {
            issued_at,
            out_of_range: issued_at.is_none(),
        }
    }
}

/// The shared claim reader for any token type.
///
/// `iat_us` is preferred when it is an integer literal, including a Python
/// `bool` (0 or 1). A negative `i64` falls back to `iat`; an integer outside
/// `i64` is out of range and does not fall back. Otherwise the reader falls
/// back to a numeric `iat`. This is the rule the access verifier applies, so
/// access and refresh tokens agree.
pub fn read_issuance(claims: &Value) -> TokenIssuance {
    let preferred = match claims.get("iat_us") {
        Some(Value::Bool(value)) => Some(TokenIssuance::from_microseconds(i64::from(*value))),
        // With `arbitrary_precision`, `is_f64` is false for every integer
        // literal, including one outside `i64`.
        Some(Value::Number(number)) if !number.is_f64() => match number.as_i64() {
            Some(microseconds) if microseconds < 0 => None,
            Some(microseconds) => Some(TokenIssuance::from_microseconds(microseconds)),
            None => Some(TokenIssuance::OUT_OF_RANGE),
        },
        _ => None,
    };
    if let Some(issuance) = preferred {
        return issuance;
    }
    let Some(Value::Number(number)) = claims.get("iat") else {
        return TokenIssuance::default();
    };
    let Some(seconds) = number.as_f64() else {
        return TokenIssuance::OUT_OF_RANGE;
    };
    let microseconds = (seconds * 1_000_000.0).round_ties_even();
    if !microseconds.is_finite() || microseconds < i64::MIN as f64 || microseconds > i64::MAX as f64
    {
        return TokenIssuance::OUT_OF_RANGE;
    }
    TokenIssuance::from_microseconds(microseconds as i64)
}

/// The `sub` claim as a UUID. Only a JSON string is a credential: a number,
/// array, object, `null` or boolean is a 401 with no database read (D1).
pub fn read_subject(claims: &Value) -> Result<Uuid, AuthError> {
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or(AuthError::Credentials)?;
    Uuid::parse_str(subject).map_err(|_| AuthError::Credentials)
}

/// The cutoff refusal for an issuance time, password cutoff first, inclusive
/// at microsecond precision. A missing or unreadable issuance time is cut off
/// only when a cutoff exists.
pub fn cutoff_refusal(issuance: TokenIssuance, user: &AccountState) -> Option<AuthError> {
    for (cutoff, error) in [
        (user.password_changed_at, AuthError::PasswordChanged),
        (user.authentication_revoked_at, AuthError::SessionRevoked),
    ] {
        if cutoff.is_some_and(|cutoff| {
            issuance.out_of_range
                || issuance
                    .issued_at
                    .is_none_or(|issued| issued.timestamp_micros() <= cutoff)
        }) {
            return Some(error);
        }
    }
    None
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
}

impl AuthError {
    pub fn status(self) -> u16 {
        match self {
            Self::Pending | Self::Rejected | Self::PasswordChangeRequired => 403,
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
        };
        json!({"detail": detail})
    }
}

pub fn validate_account(access: &VerifiedAccess, user: &AccountState) -> Result<(), AuthError> {
    if !user.exists {
        return Err(AuthError::Credentials);
    }
    // PD7 / decision 94: an unusable issuance time fails the active cutoff,
    // with password cutoff precedence, never an HTTP 500.
    if let Some(error) = cutoff_refusal(
        TokenIssuance {
            issued_at: access.issued_at,
            out_of_range: access.issuance_out_of_range,
        },
        user,
    ) {
        return Err(error);
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
