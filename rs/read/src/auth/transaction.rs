use std::{future::Future, pin::Pin, time::Duration};

use sqlx::{Connection, PgConnection, Row};

use super::{
    AccountState, AuthError, AuthenticatedRequest, CurrentActor, UserLockMode, VerifiedAccess,
    validate_account_for_path,
};
use crate::{ApiError, AppState};

#[derive(Debug)]
pub enum AuthenticatedError {
    Authentication(AuthError),
    Database(ApiError),
    AmbiguousCommit,
}

/// Borrowed request authority. No pool or transaction completion is exposed.
pub struct AuthenticatedTransaction<'a> {
    connection: &'a mut PgConnection,
    actor: CurrentActor,
}

impl AuthenticatedTransaction<'_> {
    pub fn actor(&self) -> &CurrentActor {
        &self.actor
    }
    pub fn connection(&mut self) -> &mut PgConnection {
        self.connection
    }
}

/// The error envelope of one scoped Phase 5 actor-lock statement.
///
/// SQLSTATE classification stays in the shared market read helper, so this
/// path adds no second list: `55P03`, `57014` and the retryable transport
/// failures become the generic `READ_UNAVAILABLE` result with `Retry-After`.
fn scoped_failure(error: sqlx::Error) -> ApiError {
    crate::routes::market_data_error::database_failure("compliance actor lock", &error)
}

pub async fn run_authenticated<T: Send + 'static>(
    state: &AppState,
    access: VerifiedAccess,
    request: AuthenticatedRequest,
    work: impl for<'a> FnOnce(
        &'a mut AuthenticatedTransaction<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>
    + Send
    + 'static,
) -> Result<T, AuthenticatedError> {
    let permits = state.reads.clone();
    let pool = state.pools.read.clone();
    // Detached owner retains permit, connection and row lock after client
    // cancellation. No route can replay an ambiguous commit.
    tokio::spawn(async move {
        let _permit = tokio::time::timeout(Duration::from_secs(30), permits.acquire_owned()).await
            .map_err(|_| AuthenticatedError::Database(ApiError::database()))?
            .map_err(|_| AuthenticatedError::Database(ApiError::database()))?;
        let mut connection = pool.acquire().await.map_err(|_| AuthenticatedError::Database(ApiError::database()))?;
        // The guard marks the connection as in a transaction, so dropping it
        // (including during an unwind) queues a ROLLBACK. The next use of the
        // connection sends that ROLLBACK before any other statement.
        let mut transaction = connection
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
            .await
            .map_err(|_| AuthenticatedError::Database(ApiError::database()))?;
        // Scoped Phase 5 reads own a short actor-lock deadline, so their own
        // lock and statement refusals use the market read envelope: a lock
        // timeout or cancelled statement is the generic retryable 503. Every
        // other route keeps the plain database mapping.
        let scoped = request.scoped_read_timeouts();
        let result = async {
            if scoped {
                for statement in [
                    "SET LOCAL lock_timeout = '500ms'",
                    "SET LOCAL statement_timeout = '5s'",
                ] {
                    sqlx::query(statement)
                        .execute(&mut *transaction)
                        .await
                        .map_err(|error| AuthenticatedError::Database(scoped_failure(error)))?;
                }
            }
            let query = match request.lock_mode() {
                UserLockMode::Share => "SELECT id,organization_id,role::text AS role,status::text AS status,email_verified,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1 FOR SHARE",
                UserLockMode::Update => "SELECT id,organization_id,role::text AS role,status::text AS status,email_verified,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1 FOR UPDATE",
            };
            let row = sqlx::query(query).bind(access.subject).fetch_optional(&mut *transaction).await
                .map_err(|error| AuthenticatedError::Database(if scoped { scoped_failure(error) } else { ApiError::database() }))?
                .ok_or(AuthenticatedError::Authentication(AuthError::Credentials))?;
            let password: Option<chrono::DateTime<chrono::Utc>> = row.get("password_changed_at");
            let revoked: Option<chrono::DateTime<chrono::Utc>> = row.get("authentication_revoked_at");
            validate_account_for_path(&access, &AccountState {
                exists:true, status:row.get("status"), must_change_password:row.get("must_change_password"),
                password_changed_at:password.map(|v|v.timestamp_micros()), authentication_revoked_at:revoked.map(|v|v.timestamp_micros()),
            }, request.route_template()).map_err(AuthenticatedError::Authentication)?;
            let actor = CurrentActor { id:row.get("id"), organization_id:row.get("organization_id"), role:row.get("role"), email_verified:row.get("email_verified") };
            work(&mut AuthenticatedTransaction { connection:&mut transaction, actor }).await.map_err(AuthenticatedError::Database)
        }.await;
        if result.is_err() {
            if transaction.rollback().await.is_err() || connection.ping().await.is_err() {
                let _ = connection.close().await;
            } else { connection.return_to_pool().await; }
            return result;
        }
        if transaction.commit().await.is_err() {
            // The commit may have applied. Never reuse the connection: the
            // caller cannot safely replay or complete this request.
            let _ = connection.close().await;
            return Err(AuthenticatedError::AmbiguousCommit);
        }
        if connection.ping().await.is_err() {
            let _ = connection.close().await;
            return Err(AuthenticatedError::Database(ApiError::database()));
        }
        connection.return_to_pool().await;
        result
    }).await.map_err(|_| AuthenticatedError::Database(ApiError::internal()))?
}
