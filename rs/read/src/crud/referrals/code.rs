//! R01 referral-code allocation.
//!
//! Source: `app/routers/referrals.py:36-70`.
//!
//! The allocation helper is shared with Task 19's invite route, which creates a
//! code before its first delivery attempt. This operation changes only
//! `users.referral_code`; the route transaction already holds the actor's
//! current-user row lock, so two concurrent calls for one account serialize and
//! return the same committed code.

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ApiError;
use crate::ApiErrorKind;
use crate::Json;
use crate::auth::CurrentActor;
use crate::routes::ports::query_error;

const MY_CODE_PATH: &str = "/api/referrals/my-code";

/// Python's `_assign_unique_referral_code(retries=10)` bound. Never raise it:
/// an unbounded retry loop is the defect the source deliberately avoids.
pub const COLLISION_PROBES: usize = 10;

/// `string.ascii_uppercase + string.digits`. Index order is significant,
/// because `secrets.choice` selects by index into exactly this alphabet.
const ALPHABET: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const PREFIX: &str = "VDX-";
const SUFFIX_LENGTH: usize = 6;

/// The largest multiple of 36 that fits in one byte.
const UNIFORM_RANGE: u8 = 252;

/// Referral-code candidates for one allocation attempt.
///
/// Production draws each character from fresh random bytes with rejection
/// sampling, so every character is uniform over the 36-character alphabet, as
/// `secrets.choice` is. A `contract-tests` build may substitute a fixed
/// sequence through `RUST_CONTRACT_REFERRAL_CANDIDATES`; that override does not
/// exist in a release build, so a deployed service can never be steered to a
/// chosen code, and a finite sequence that runs out simply ends the attempt.
pub enum Candidates {
    Random,
    Fixed(std::vec::IntoIter<String>),
}

impl Candidates {
    pub fn new() -> Self {
        #[cfg(feature = "contract-tests")]
        if std::env::var("ENVIRONMENT").as_deref() == Ok("test")
            && let Ok(text) = std::env::var("RUST_CONTRACT_REFERRAL_CANDIDATES")
            && let Ok(sequence) = serde_json::from_str::<Vec<String>>(&text)
        {
            return Self::fixed(sequence);
        }
        Self::Random
    }

    pub(crate) fn for_actor(actor: Uuid) -> Self {
        #[cfg(feature = "contract-tests")]
        if std::env::var("ENVIRONMENT").as_deref() == Ok("test")
            && let Ok(raw) = std::env::var("REFERRAL_CANDIDATE_MAP")
            && let Ok(map) =
                serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(&raw)
            && let Some(sequence) = map.get(&actor.to_string())
        {
            return Self::fixed(sequence.clone());
        }
        let _ = actor;
        Self::new()
    }

    /// A deterministic candidate sequence, for tests and the contract harness.
    pub fn fixed(codes: Vec<String>) -> Self {
        Self::Fixed(codes.into_iter())
    }
}

impl Iterator for Candidates {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Random => Some(random_code()),
            Self::Fixed(sequence) => sequence.next(),
        }
    }
}

impl Default for Candidates {
    fn default() -> Self {
        Self::new()
    }
}

/// Build one random code from fresh random bytes.
fn random_code() -> String {
    let mut code = String::with_capacity(PREFIX.len() + SUFFIX_LENGTH);
    code.push_str(PREFIX);
    for _ in 0..SUFFIX_LENGTH {
        code.push(char::from(ALPHABET[uniform_index()]));
    }
    code
}

/// Draw one alphabet index from fresh random bytes.
///
/// Values at or above [`UNIFORM_RANGE`] are rejected instead of folded, which
/// keeps every index equally likely. Sixteen bytes per draw and 4/256 rejected
/// per byte make running out of usable bytes effectively impossible.
fn uniform_index() -> usize {
    loop {
        for byte in Uuid::new_v4().as_bytes() {
            if *byte < UNIFORM_RANGE {
                return usize::from(*byte % 36);
            }
        }
    }
}

/// R01: return the actor's referral code, allocating one when the account has
/// none.
///
/// The caller already holds the actor's current-user row lock, so the read of
/// the existing code and a new assignment cannot interleave with another
/// transaction for the same account.
pub(crate) async fn allocate_my_code(
    connection: &mut PgConnection,
    actor: &CurrentActor,
    frontend_url: &str,
    candidates: &mut Candidates,
) -> Result<Json, ApiError> {
    if !actor.email_verified {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            detail: json!("Email must be verified to get a referral code"),
            retry_after: None,
            kind: ApiErrorKind::Expected,
        });
    }
    // Python treats the empty string as "no code" because `not ""` is true.
    let existing =
        sqlx::query_scalar::<_, Option<String>>("SELECT referral_code FROM users WHERE id = $1")
            .bind(actor.id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| query_error(MY_CODE_PATH, &error))?
            .flatten()
            .filter(|code| !code.is_empty());
    let code = match existing {
        Some(code) => code,
        None => assign_unique(connection, actor.id, candidates).await?,
    };
    Ok(referral_code_response(frontend_url, &code))
}

/// Probe candidates until one is free, then commit it to the actor.
///
/// The probe and the assignment stay inside the caller's transaction, so the
/// actor's row is the only business write and it commits with the response.
async fn assign_unique(
    connection: &mut PgConnection,
    actor: Uuid,
    candidates: &mut Candidates,
) -> Result<String, ApiError> {
    for _ in 0..COLLISION_PROBES {
        let Some(candidate) = candidates.next() else {
            break;
        };
        let taken: Option<i32> = sqlx::query_scalar("SELECT 1 FROM users WHERE referral_code = $1")
            .bind(&candidate)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| query_error(MY_CODE_PATH, &error))?;
        if taken.is_none() {
            sqlx::query("UPDATE users SET referral_code = $1 WHERE id = $2")
                .bind(&candidate)
                .bind(actor)
                .execute(&mut *connection)
                .await
                .map_err(|error| query_error(MY_CODE_PATH, &error))?;
            return Ok(candidate);
        }
    }
    Err(ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: json!("Failed to generate unique referral code"),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    })
}

/// Build the `ReferralCodeResponse` body.
///
/// Python concatenates `FRONTEND_URL + "/invite/" + code` without trimming the
/// configured URL, so a trailing slash is preserved verbatim.
pub fn referral_code_response(frontend_url: &str, code: &str) -> Json {
    json!({
        "referral_code": code,
        "referral_link": format!("{frontend_url}/invite/{code}"),
    })
}
