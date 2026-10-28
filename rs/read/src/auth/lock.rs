//! Lock keys, lock orders and the bounded write retry that Rust shares with the
//! Python writers on one database.
//!
//! **Session writes** take their locks in this order and never another:
//!
//! 1. the device advisory lock (key from [`device_lock_key`]);
//! 2. the user row, `FOR UPDATE`;
//! 3. the user's reset-token row or refresh-session rows, `FOR UPDATE`.
//!
//! Python takes the same order with the same advisory key, so a Rust and a
//! Python writer for one browser device wait for each other instead of
//! deadlocking. The statements live in [`super::store`].
//!
//! **Organization writes** use the business transaction owner without a
//! preheld actor lock. Take every affected organization row, every affected
//! user row including the actor, and every affected join request, each in
//! ascending id, and only then any market invalidation
//! ([`lock_organization_write`]). A whole organization write runs at most
//! [`ORG_WRITE_ATTEMPTS`] times when PostgreSQL reports a serialization
//! failure or a deadlock ([`retry_org_write`]).

use std::future::Future;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{ApiError, ApiErrorKind};

/// The length of a lowercase SHA-256 hex digest.
const DIGEST_HEX_LENGTH: usize = 64;

/// The advisory key uses the first eight digest bytes (sixteen hex digits).
const LOCK_KEY_BYTES: usize = 8;

/// True when `value` is a lowercase SHA-256 hex digest, the only form in which
/// token and device values reach the database.
pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == DIGEST_HEX_LENGTH
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The PostgreSQL advisory-lock key for one browser device.
///
/// The key is the first eight bytes of the device cookie's SHA-256 digest,
/// read big-endian as a signed 64-bit integer. Python maps the same digest to
/// the same key, so both stacks contend on one lock. `device_hash` is the
/// lowercase hex digest stored in `refresh_sessions.device_id_hash`; anything
/// else has no key.
pub fn device_lock_key(device_hash: &str) -> Option<i64> {
    if !is_sha256_hex(device_hash) {
        return None;
    }
    let mut prefix = [0u8; LOCK_KEY_BYTES];
    for (index, byte) in prefix.iter_mut().enumerate() {
        let pair = &device_hash[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(i64::from_be_bytes(prefix))
}

/// Total attempts for one organization write: the first try and at most two
/// retries.
pub const ORG_WRITE_ATTEMPTS: usize = 3;

/// The SQLSTATEs after which a whole organization write may run again:
/// serialization failure and deadlock. A lock or statement timeout is not
/// retried; it keeps its own mapping.
pub const RETRYABLE_SQLSTATES: [&str; 2] = ["40001", "40P01"];

/// The error code a client reads after every attempt met a write conflict.
pub const ORG_WRITE_BUSY_CODE: &str = "ORGANIZATION_WRITE_BUSY";

/// A failure that [`retry_org_write`] can classify.
pub trait WriteConflict {
    /// True for a serialization failure (40001) or a deadlock (40P01).
    fn is_write_conflict(&self) -> bool;
}

impl WriteConflict for sqlx::Error {
    fn is_write_conflict(&self) -> bool {
        self.as_database_error()
            .and_then(|error| error.code())
            .is_some_and(|code| RETRYABLE_SQLSTATES.contains(&code.as_ref()))
    }
}

/// The result of an organization write that did not commit.
#[derive(Debug)]
pub enum OrgWriteError<E> {
    /// Every one of the [`ORG_WRITE_ATTEMPTS`] attempts met a write conflict.
    Busy,
    /// The first failure that is not a write conflict, unchanged.
    Failed(E),
}

/// The error one attempt of [`retry_org_write`] returns. Only a database
/// write conflict makes the helper run the attempt again.
#[derive(Debug)]
pub enum OrgWriteAttemptError {
    /// A database failure; 40001 and 40P01 are write conflicts.
    Database(sqlx::Error),
    /// A refusal the route decided on; it is never retried.
    Application(ApiError),
}

impl From<sqlx::Error> for OrgWriteAttemptError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<ApiError> for OrgWriteAttemptError {
    fn from(error: ApiError) -> Self {
        Self::Application(error)
    }
}

impl WriteConflict for OrgWriteAttemptError {
    fn is_write_conflict(&self) -> bool {
        match self {
            Self::Database(error) => error.is_write_conflict(),
            Self::Application(_) => false,
        }
    }
}

impl From<OrgWriteError<OrgWriteAttemptError>> for ApiError {
    fn from(error: OrgWriteError<OrgWriteAttemptError>) -> Self {
        match error {
            OrgWriteError::Busy => org_write_busy(),
            OrgWriteError::Failed(OrgWriteAttemptError::Application(error)) => error,
            OrgWriteError::Failed(OrgWriteAttemptError::Database(_)) => ApiError::database(),
        }
    }
}

/// The bounded busy answer after the last attempt met a write conflict: the
/// existing 503 status, a `Retry-After`, and a stable code.
pub fn org_write_busy() -> ApiError {
    ApiError {
        kind: ApiErrorKind::Database,
        status: StatusCode::SERVICE_UNAVAILABLE,
        detail: json!({
            "code": ORG_WRITE_BUSY_CODE,
            "message": "The organization is being changed by another request; retry shortly.",
        }),
        retry_after: Some(1),
    }
}

/// Run one organization write until it commits, fails for another reason, or
/// has met a write conflict on all [`ORG_WRITE_ATTEMPTS`] attempts.
///
/// `attempt` receives the attempt number, starting at 1. Each call must run a
/// complete transaction: begin, take the locks with [`lock_organization_write`],
/// write, and commit. A failed attempt must be rolled back before it returns,
/// so the next attempt starts from nothing. A failed `COMMIT` whose outcome is
/// unknown must be reported as an error that is not a write conflict, so a
/// write that may have committed is never run again.
pub async fn retry_org_write<T, E, F, Fut>(mut attempt: F) -> Result<T, OrgWriteError<E>>
where
    E: WriteConflict,
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    for number in 1..=ORG_WRITE_ATTEMPTS {
        match attempt(number).await {
            Ok(value) => return Ok(value),
            Err(error) if error.is_write_conflict() => {
                tracing::info!(
                    attempt = number,
                    attempts = ORG_WRITE_ATTEMPTS,
                    "organization write met a serialization failure or deadlock"
                );
            }
            Err(error) => return Err(OrgWriteError::Failed(error)),
        }
    }
    Err(OrgWriteError::Busy)
}

/// The rows one organization write locks, each list sorted ascending and
/// without duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrganizationWriteScope {
    organizations: Vec<Uuid>,
    users: Vec<Uuid>,
    join_requests: Vec<Uuid>,
}

impl OrganizationWriteScope {
    /// Include every affected user, including the actor when no user lock is
    /// held. The business owner acquires this entire scope before admission.
    pub fn new(
        organizations: impl IntoIterator<Item = Uuid>,
        users: impl IntoIterator<Item = Uuid>,
        join_requests: impl IntoIterator<Item = Uuid>,
    ) -> Self {
        Self {
            organizations: sorted_unique(organizations),
            users: sorted_unique(users),
            join_requests: sorted_unique(join_requests),
        }
    }

    pub fn organizations(&self) -> &[Uuid] {
        &self.organizations
    }

    pub fn users(&self) -> &[Uuid] {
        &self.users
    }

    pub fn join_requests(&self) -> &[Uuid] {
        &self.join_requests
    }
}

fn sorted_unique(ids: impl IntoIterator<Item = Uuid>) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The ids whose rows existed and are now locked. A requested id that is
/// absent here had no row; the caller decides whether that is a conflict.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LockedOrganizationWrite {
    pub organizations: Vec<Uuid>,
    pub users: Vec<Uuid>,
    pub join_requests: Vec<Uuid>,
}

/// Lock the rows of one organization write in the canonical order on the
/// caller's transaction: organizations, then users, then join requests, each
/// by ascending id. PostgreSQL's `uuid` order and Rust's [`Uuid`] order are
/// both byte order, so the row locks follow the sorted lists.
pub async fn lock_organization_write(
    connection: &mut PgConnection,
    scope: &OrganizationWriteScope,
) -> Result<LockedOrganizationWrite, sqlx::Error> {
    let organizations = lock_rows(
        connection,
        "SELECT id FROM organizations WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &scope.organizations,
    )
    .await?;
    let users = lock_rows(
        connection,
        "SELECT id FROM users WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &scope.users,
    )
    .await?;
    let join_requests = lock_rows(
        connection,
        "SELECT id FROM organization_join_requests WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &scope.join_requests,
    )
    .await?;
    Ok(LockedOrganizationWrite {
        organizations,
        users,
        join_requests,
    })
}

async fn lock_rows(
    connection: &mut PgConnection,
    statement: &'static str,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(statement)
        .bind(ids)
        .fetch_all(&mut *connection)
        .await
}
