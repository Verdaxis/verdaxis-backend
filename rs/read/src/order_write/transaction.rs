//! The one sealed order-book write boundary: transaction, locks, receipt and
//! outbox for a single command.
//!
//! Everything that mutates the order book goes through [`execute_order_command`].
//! It owns the transaction lifetime, the lock order, the receipt, the outbox and
//! the bounded retry, and it exposes no connection, no statement and no lock to
//! any other module.
//!
//! The reviewed order of work, which this shell reproduces:
//!
//! 1. `SET LOCAL lock_timeout = '500ms'` and `SET LOCAL statement_timeout = '5s'`
//!    before the first lock.
//! 2. The idempotency advisory lock, then the stored-receipt read.
//! 3. The sorted, deduplicated market-slice advisory locks.
//! 4. The `orderbook_orders` row locks, in the order the operation declares.
//! 5. The actor user rows, then the organization rows, each by ascending id.
//! 6. The outbox events, then the receipt.
//! 7. `COMMIT`. A failed commit is ambiguous and is never replayed.
//!
//! [`WriteSession`] is the one database port. It is a port rather than an
//! exposed connection so a wider route, a test or a future command cannot lock
//! or write an order-book row directly.

// No business command is enabled in Task 20. Keep the compiled PostgreSQL
// shell private until a separately reviewed command can use it.
#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use super::command::{
    CommandContext, Operation, OrderCommand, OutboxEvent, Receipt, ReceiptRecord,
    command_request_hash,
};
use super::locks::{
    MARKET_LOCK_TIMEOUT, MARKET_STATEMENT_TIMEOUT, idempotency_lock_key, normalize_idempotency_key,
    ordered_slice_lock_keys,
};
use crate::{ApiError, ApiErrorKind, AppState};

/// The reviewed attempt bound. The second attempt starts in a fresh transaction
/// on the same request-scoped session.
pub const DEFAULT_MARKET_TRANSACTION_ATTEMPTS: usize = 2;

/// The bound the reviewed retry factory itself enforces.
pub const MAX_MARKET_TRANSACTION_ATTEMPTS: usize = 3;

pub const RETRYABLE_SQLSTATES: [&str; 2] = ["40001", "40P01"];
pub const LOCK_BUSY_SQLSTATE: &str = "55P03";
pub const LOCK_TIMEOUT_SQLSTATE: &str = "57014";

pub const MARKET_ORGANIZATION_REQUIRED: &str = "Market organization is required";
pub const REPLAY_CONFLICT_MESSAGE: &str =
    "Idempotency-Key was reused with a different request or actor context";
pub const SLICE_BUSY_MESSAGE: &str = "Market slice is busy; retry the request";
pub const SLICE_TIMEOUT_MESSAGE: &str = "Market slice lock timed out; retry the request";
pub const IDEMPOTENCY_BUSY_MESSAGE: &str = "Idempotency key is busy; retry the same request";
pub const IDEMPOTENCY_TIMEOUT_MESSAGE: &str = "Idempotency lock timed out; retry the request";
pub const SERIALIZATION_FAILED_MESSAGE: &str =
    "Market transaction serialization failed; retry the request";
pub const ORDER_NOT_FOUND_MESSAGE: &str = "Order not found";

/// One owned future the port returns. A boxed future keeps the shell generic
/// over the session type without an async trait or a refinement obligation.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One database failure as the port reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteError {
    /// The PostgreSQL SQLSTATE, when the server produced one.
    pub sqlstate: Option<String>,
    /// A failed `COMMIT`. The transaction outcome is unknown, so the command is
    /// never replayed and the caller must not assume it did not commit.
    pub ambiguous_commit: bool,
}

impl WriteError {
    pub const fn other() -> Self {
        Self {
            sqlstate: None,
            ambiguous_commit: false,
        }
    }

    pub const fn ambiguous() -> Self {
        Self {
            sqlstate: None,
            ambiguous_commit: true,
        }
    }

    /// SQLSTATE 40001 and 40P01 are the only failures a full rerun may repeat.
    pub fn is_retryable(&self) -> bool {
        RETRYABLE_SQLSTATES.contains(&self.sqlstate.as_deref().unwrap_or_default())
    }
}

/// One committed command result, or the stored result a replay returned.
#[derive(Clone, Debug, PartialEq)]
pub struct CommittedCommand {
    pub response_status: u16,
    pub response_body: Value,
    /// `true` when a stored receipt answered this request and nothing ran again.
    pub replayed: bool,
}

/// The one database port of the order-book write boundary.
///
/// Each method is one reviewed statement group, so the shell never composes
/// SQL and no caller can acquire a lock or write a row on its own. The live
/// PostgreSQL session implements it, and the boundary tests drive the shell
/// through a recording double.
trait WriteSession {
    /// Set the transaction-local lock and statement bounds.
    fn lock_bounds<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Take the transaction-scoped idempotency advisory lock.
    fn lock_idempotency<'a>(&'a mut self, lock_key: i64) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Take one transaction-scoped market-slice advisory lock.
    fn lock_slice<'a>(&'a mut self, lock_key: i64) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Read the stored receipt for one actor, operation and key.
    fn load_receipt<'a>(
        &'a mut self,
        actor_user_id: Uuid,
        operation: &'a str,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, Result<Option<Receipt>, WriteError>>;
    /// Lock one `orderbook_orders` row. `false` means the row does not exist.
    fn lock_order_row<'a>(&'a mut self, order_id: Uuid) -> BoxFuture<'a, Result<bool, WriteError>>;
    /// Lock one actor user row.
    fn lock_actor_user<'a>(&'a mut self, user_id: Uuid) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Lock one organization row.
    fn lock_organization<'a>(
        &'a mut self,
        organization_id: Uuid,
    ) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Insert one participant-scoped outbox event.
    fn insert_outbox<'a>(
        &'a mut self,
        event: &'a OutboxEvent,
    ) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Insert the receipt for a successful command.
    fn insert_receipt<'a>(
        &'a mut self,
        receipt: &'a ReceiptRecord,
    ) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Commit. A failure here is ambiguous.
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>>;
    /// Roll back the current attempt.
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>>;
}

/// Which lock the attempt was taking, so a contention failure maps to its own
/// reviewed message instead of one shared refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockPhase {
    Transaction,
    Idempotency,
    Slice,
    Row,
}

/// The per-attempt projection of one command.
struct WritePlan {
    operation: Operation,
    actor_user_id: Uuid,
    effective_organization_id: Uuid,
    support_context_id: Option<Uuid>,
    idempotency_key: Option<String>,
    request_hash: String,
    resource_id: Uuid,
}

enum AttemptOutcome {
    Committed(CommittedCommand),
    Replayed(CommittedCommand),
}

enum AttemptFailure {
    /// A failed commit: never replayed, never rolled back.
    Ambiguous,
    OrderNotFound,
    ReplayConflict,
    Database {
        error: WriteError,
        phase: LockPhase,
    },
}

/// Run one command to a committed result, or to its stored replay, through the
/// one write boundary.
pub async fn execute_order_command(
    state: &AppState,
    context: CommandContext,
    command: OrderCommand,
) -> Result<CommittedCommand, ApiError> {
    let _ = (state, context, command);
    // Market HTTP writes stay on Python. A caller cannot manufacture a receipt
    // or an event for an operation this phase does not implement.
    Err(expected(
        StatusCode::NOT_IMPLEMENTED,
        "Rust order commands are not enabled",
    ))
}

/// Private transaction scaffold. No production command uses it in Task 20.
/// Tests exercise the lock, retry, receipt and outbox protocol.
async fn run_write_unit<S, F>(
    mut opener: F,
    context: CommandContext,
    command: OrderCommand,
) -> Result<CommittedCommand, ApiError>
where
    S: WriteSession + Send + 'static,
    F: FnMut() -> BoxFuture<'static, Result<S, WriteError>> + Send + 'static,
{
    let effective_organization_id = context
        .effective_organization_id
        .ok_or_else(|| expected(StatusCode::FORBIDDEN, MARKET_ORGANIZATION_REQUIRED))?;
    let idempotency_key = match context.idempotency_key.as_deref() {
        Some(raw) => Some(normalize_idempotency_key(raw)?),
        None => None,
    };
    let plan = WritePlan {
        operation: command.operation,
        actor_user_id: context.actor_user_id,
        effective_organization_id,
        support_context_id: context.support_context_id,
        idempotency_key,
        request_hash: command_request_hash(&command.payload),
        resource_id: command.resource_id,
    };
    let slice_keys = ordered_slice_lock_keys(&command.locks.slices)?;

    for attempt in 0..DEFAULT_MARKET_TRANSACTION_ATTEMPTS {
        let mut session = match opener().await {
            Ok(session) => session,
            Err(error) => return Err(map_database_failure(LockPhase::Transaction, &error)),
        };
        match run_attempt(&mut session, &plan, &slice_keys, &command).await {
            Ok(AttemptOutcome::Committed(committed)) => return Ok(committed),
            Ok(AttemptOutcome::Replayed(committed)) => {
                // A replay wrote nothing, so end the transaction without
                // committing: no outbox event can be emitted a second time.
                let _ = session.rollback().await;
                return Ok(committed);
            }
            Err(AttemptFailure::Ambiguous) => return Err(ApiError::database()),
            Err(AttemptFailure::OrderNotFound) => {
                let _ = session.rollback().await;
                return Err(expected(StatusCode::NOT_FOUND, ORDER_NOT_FOUND_MESSAGE));
            }
            Err(AttemptFailure::ReplayConflict) => {
                let _ = session.rollback().await;
                return Err(expected(StatusCode::CONFLICT, REPLAY_CONFLICT_MESSAGE));
            }
            Err(AttemptFailure::Database { error, phase }) => {
                // Exactly one rollback per failed attempt.
                let _ = session.rollback().await;
                if error.is_retryable() {
                    if attempt + 1 >= DEFAULT_MARKET_TRANSACTION_ATTEMPTS {
                        return Err(expected(
                            StatusCode::SERVICE_UNAVAILABLE,
                            SERIALIZATION_FAILED_MESSAGE,
                        ));
                    }
                    continue;
                }
                return Err(map_database_failure(phase, &error));
            }
        }
    }
    Err(expected(
        StatusCode::SERVICE_UNAVAILABLE,
        SERIALIZATION_FAILED_MESSAGE,
    ))
}

async fn run_attempt<S: WriteSession>(
    session: &mut S,
    plan: &WritePlan,
    slice_keys: &[i64],
    command: &OrderCommand,
) -> Result<AttemptOutcome, AttemptFailure> {
    session
        .lock_bounds()
        .await
        .map_err(|error| AttemptFailure::Database {
            error,
            phase: LockPhase::Transaction,
        })?;

    if let Some(key) = plan.idempotency_key.as_deref() {
        let lock_key = idempotency_lock_key(plan.actor_user_id, plan.operation.as_str(), key);
        session
            .lock_idempotency(lock_key)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Idempotency,
            })?;
        let stored = session
            .load_receipt(plan.actor_user_id, plan.operation.as_str(), key)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Idempotency,
            })?;
        if let Some(stored) = stored {
            if stored.request_hash != plan.request_hash
                || stored.effective_organization_id != plan.effective_organization_id
                || stored.support_context_id != plan.support_context_id
                || stored.resource_type != plan.operation.resource_type()
                || stored.resource_id != plan.resource_id
            {
                return Err(AttemptFailure::ReplayConflict);
            }
            return Ok(AttemptOutcome::Replayed(CommittedCommand {
                response_status: stored.response_status,
                response_body: stored.response_body,
                replayed: true,
            }));
        }
    }

    for lock_key in slice_keys {
        session
            .lock_slice(*lock_key)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Slice,
            })?;
    }

    for order_id in &command.locks.order_rows {
        if !session
            .lock_order_row(*order_id)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Row,
            })?
        {
            return Err(AttemptFailure::OrderNotFound);
        }
    }

    let mut actor_users = command.locks.actor_users.clone();
    actor_users.sort();
    actor_users.dedup();
    for user_id in actor_users {
        session
            .lock_actor_user(user_id)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Row,
            })?;
    }

    let mut organizations = command.locks.organizations.clone();
    organizations.sort();
    organizations.dedup();
    for organization_id in organizations {
        session
            .lock_organization(organization_id)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Row,
            })?;
    }

    for event in &command.events {
        session
            .insert_outbox(event)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Row,
            })?;
    }
    if let Some(key) = plan.idempotency_key.as_deref() {
        let receipt = ReceiptRecord {
            id: Uuid::new_v4(),
            actor_user_id: plan.actor_user_id,
            effective_organization_id: plan.effective_organization_id,
            support_context_id: plan.support_context_id,
            operation: plan.operation,
            idempotency_key: key.to_owned(),
            request_hash: plan.request_hash.clone(),
            resource_id: plan.resource_id,
            response_status: command.response_status,
            response_body: command.response_body.clone(),
        };
        session
            .insert_receipt(&receipt)
            .await
            .map_err(|error| AttemptFailure::Database {
                error,
                phase: LockPhase::Row,
            })?;
    }

    session.commit().await.map_err(|error| {
        if error.ambiguous_commit {
            AttemptFailure::Ambiguous
        } else {
            AttemptFailure::Database {
                error,
                phase: LockPhase::Row,
            }
        }
    })?;
    Ok(AttemptOutcome::Committed(CommittedCommand {
        response_status: command.response_status,
        response_body: command.response_body.clone(),
        replayed: false,
    }))
}

fn expected(status: StatusCode, detail: &str) -> ApiError {
    ApiError {
        status,
        detail: json!(detail),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn map_database_failure(phase: LockPhase, error: &WriteError) -> ApiError {
    match (phase, error.sqlstate.as_deref()) {
        (LockPhase::Idempotency, Some(LOCK_BUSY_SQLSTATE)) => {
            expected(StatusCode::SERVICE_UNAVAILABLE, IDEMPOTENCY_BUSY_MESSAGE)
        }
        (LockPhase::Idempotency, Some(LOCK_TIMEOUT_SQLSTATE)) => {
            expected(StatusCode::SERVICE_UNAVAILABLE, IDEMPOTENCY_TIMEOUT_MESSAGE)
        }
        (LockPhase::Slice, Some(LOCK_BUSY_SQLSTATE)) => {
            expected(StatusCode::CONFLICT, SLICE_BUSY_MESSAGE)
        }
        (LockPhase::Slice, Some(LOCK_TIMEOUT_SQLSTATE)) => {
            expected(StatusCode::SERVICE_UNAVAILABLE, SLICE_TIMEOUT_MESSAGE)
        }
        _ => ApiError::database(),
    }
}

/// The live session. It owns the admission permit and the connection, so a
/// cancelled caller cannot leave an unowned row lock behind.
struct PgWriteSession {
    transaction: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    _permit: OwnedSemaphorePermit,
    #[cfg(feature = "contract-tests")]
    fixture: Option<Value>,
}

impl PgWriteSession {
    fn connection(&mut self) -> Result<&mut PgConnection, WriteError> {
        self.transaction
            .as_deref_mut()
            .ok_or_else(WriteError::other)
    }

    /// SQLx owns rollback-on-drop, including panic and task cancellation paths.
    async fn finish(&mut self, commit: bool) -> Result<(), WriteError> {
        let Some(transaction) = self.transaction.take() else {
            return Err(WriteError::other());
        };
        if commit {
            // Never return an uncertain commit connection to the pool.
            transaction
                .commit()
                .await
                .map_err(|_| WriteError::ambiguous())
        } else {
            transaction
                .rollback()
                .await
                .map_err(|error| write_error(&error))
        }
    }
}

impl WriteSession for PgWriteSession {
    fn lock_bounds<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            // Values come from the reviewed constants, so the executed text and
            // the pinned spec text cannot drift apart.
            let statements = [
                format!("SET LOCAL lock_timeout = '{MARKET_LOCK_TIMEOUT}'"),
                format!("SET LOCAL statement_timeout = '{MARKET_STATEMENT_TIMEOUT}'"),
            ];
            let connection = self.connection()?;
            for statement in statements {
                sqlx::query(sqlx::AssertSqlSafe(statement))
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| write_error(&error))?;
            }
            Ok(())
        })
    }

    fn lock_idempotency<'a>(&'a mut self, lock_key: i64) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            let connection = self.connection()?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(lock_key)
                .execute(&mut *connection)
                .await
                .map_err(|error| write_error(&error))?;
            Ok(())
        })
    }

    fn lock_slice<'a>(&'a mut self, lock_key: i64) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            let connection = self.connection()?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(lock_key)
                .execute(&mut *connection)
                .await
                .map_err(|error| write_error(&error))?;
            Ok(())
        })
    }

    fn load_receipt<'a>(
        &'a mut self,
        actor_user_id: Uuid,
        operation: &'a str,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, Result<Option<Receipt>, WriteError>> {
        Box::pin(async move {
            let connection = self.connection()?;
            let row = sqlx::query(
                "SELECT request_hash, effective_organization_id, support_context_id, resource_type, \
                 resource_id, response_status, response_body::text AS response_body \
                 FROM market_command_results \
                 WHERE actor_user_id = $1 AND operation = $2 AND idempotency_key = $3",
            )
            .bind(actor_user_id)
            .bind(operation)
            .bind(idempotency_key)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| write_error(&error))?;
            let Some(row) = row else {
                return Ok(None);
            };
            let response_status: i16 = row
                .try_get("response_status")
                .map_err(|error| write_error(&error))?;
            let response_body: Option<String> = row
                .try_get("response_body")
                .map_err(|error| write_error(&error))?;
            Ok(Some(Receipt {
                request_hash: row
                    .try_get("request_hash")
                    .map_err(|error| write_error(&error))?,
                effective_organization_id: row
                    .try_get("effective_organization_id")
                    .map_err(|error| write_error(&error))?,
                support_context_id: row
                    .try_get("support_context_id")
                    .map_err(|error| write_error(&error))?,
                resource_type: row
                    .try_get("resource_type")
                    .map_err(|error| write_error(&error))?,
                resource_id: row
                    .try_get("resource_id")
                    .map_err(|error| write_error(&error))?,
                response_status: u16::try_from(response_status).map_err(|_| WriteError::other())?,
                response_body: decode_body(response_body)?,
            }))
        })
    }

    fn lock_order_row<'a>(&'a mut self, order_id: Uuid) -> BoxFuture<'a, Result<bool, WriteError>> {
        Box::pin(async move {
            let connection = self.connection()?;
            let row = sqlx::query("SELECT id FROM orderbook_orders WHERE id = $1 FOR UPDATE")
                .bind(order_id)
                .fetch_optional(&mut *connection)
                .await
                .map_err(|error| write_error(&error))?;
            Ok(row.is_some())
        })
    }

    fn lock_actor_user<'a>(&'a mut self, user_id: Uuid) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            let connection = self.connection()?;
            sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .execute(&mut *connection)
                .await
                .map_err(|error| write_error(&error))?;
            Ok(())
        })
    }

    fn lock_organization<'a>(
        &'a mut self,
        organization_id: Uuid,
    ) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            let connection = self.connection()?;
            sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
                .bind(organization_id)
                .execute(&mut *connection)
                .await
                .map_err(|error| write_error(&error))?;
            Ok(())
        })
    }

    fn insert_outbox<'a>(
        &'a mut self,
        event: &'a OutboxEvent,
    ) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            let participants = serde_json::to_string(&event.participant_org_ids)
                .map_err(|_| WriteError::other())?;
            let payload = serde_json::to_string(&event.payload).map_err(|_| WriteError::other())?;
            let event_id = Uuid::new_v4();
            #[cfg(feature = "contract-tests")]
            let event_id = self
                .fixture
                .as_ref()
                .map(|v| v["id"].as_str().unwrap().parse().unwrap())
                .unwrap_or(event_id);
            let statement = "INSERT INTO market_event_outbox (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload) VALUES ($1, $2, $3, $4, $5::json, $6::json)";
            #[cfg(feature = "contract-tests")]
            let statement = if self.fixture.is_some() {
                "INSERT INTO market_event_outbox (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload, created_at) VALUES ($1, $2, $3, $4, $5::json, $6::json, '2026-10-19T00:00:00Z')"
            } else {
                statement
            };
            let connection = self.connection()?;
            sqlx::query(statement)
                .bind(event_id)
                .bind(&event.event_type)
                .bind(&event.aggregate_type)
                .bind(&event.aggregate_id)
                .bind(participants)
                .bind(payload)
                .execute(&mut *connection)
                .await
                .map_err(|error| write_error(&error))?;
            Ok(())
        })
    }

    fn insert_receipt<'a>(
        &'a mut self,
        receipt: &'a ReceiptRecord,
    ) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            let response_body =
                serde_json::to_string(&receipt.response_body).map_err(|_| WriteError::other())?;
            #[cfg(feature = "contract-tests")]
            if let Some(fixture) = self.fixture.clone() {
                return contract::insert_fixture_receipt(self, receipt, &fixture).await;
            }
            let connection = self.connection()?;
            sqlx::query(
                "INSERT INTO market_command_results (id, actor_user_id, effective_organization_id, \
                 support_context_id, operation, idempotency_key, request_hash, resource_type, \
                 resource_id, response_status, response_body) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::jsonb)",
            )
            .bind(receipt.id)
            .bind(receipt.actor_user_id)
            .bind(receipt.effective_organization_id)
            .bind(receipt.support_context_id)
            .bind(receipt.operation.as_str())
            .bind(&receipt.idempotency_key)
            .bind(&receipt.request_hash)
            .bind(receipt.operation.resource_type())
            .bind(receipt.resource_id)
            .bind(receipt.response_status as i16)
            .bind(response_body)
            .execute(&mut *connection)
            .await
            .map_err(|error| write_error(&error))?;
            Ok(())
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(self.finish(true))
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(self.finish(false))
    }
}

/// Acquire one admission slot, one connection and the command transaction.
async fn open_session(
    permits: Arc<Semaphore>,
    pool: sqlx::PgPool,
) -> Result<PgWriteSession, WriteError> {
    let permit = tokio::time::timeout(Duration::from_secs(30), permits.acquire_owned())
        .await
        .map_err(|_| WriteError::other())?
        .map_err(|_| WriteError::other())?;
    let mut connection = pool.acquire().await.map_err(|error| write_error(&error))?;
    // The scaffold is unused in production. Close its connection on every exit
    // so even a lost COMMIT acknowledgement cannot return it to the pool.
    connection.close_on_drop();
    let mut transaction = sqlx::Transaction::begin(connection, None)
        .await
        .map_err(|error| write_error(&error))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED READ WRITE")
        .execute(&mut *transaction)
        .await
        .map_err(|error| write_error(&error))?;
    Ok(PgWriteSession {
        transaction: Some(transaction),
        _permit: permit,
        #[cfg(feature = "contract-tests")]
        fixture: None,
    })
}

fn write_error(error: &sqlx::Error) -> WriteError {
    WriteError {
        sqlstate: error
            .as_database_error()
            .and_then(|failure| failure.code())
            .map(|code| code.to_string()),
        ambiguous_commit: false,
    }
}

#[cfg(test)]
#[path = "transaction_tests.rs"]
mod tests;

fn decode_body(text: Option<String>) -> Result<Value, WriteError> {
    match text {
        None => Ok(Value::Null),
        Some(text) => serde_json::from_str(&text).map_err(|_| WriteError::other()),
    }
}

#[cfg(feature = "contract-tests")]
#[path = "contract.rs"]
pub(super) mod contract;
