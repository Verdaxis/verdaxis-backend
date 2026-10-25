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
    command_request_hash, validate_receipt,
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
    #[cfg(feature = "contract-tests")]
    fn prepare_response<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async { Ok(()) })
    }
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
    Refusal(ApiError),
    ReceiptRace,
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
            Err(AttemptFailure::Ambiguous) => {
                return Err(market_refusal(
                    super::error::MarketErrorCode::CommitOutcomeUnknown,
                ));
            }
            Err(AttemptFailure::Refusal(error)) => {
                let _ = session.rollback().await;
                return Err(error);
            }
            Err(AttemptFailure::ReceiptRace) => {
                // The losing transaction includes effects: discard all of them.
                session.rollback().await.map_err(|_| ApiError::database())?;
                let mut reader = opener().await.map_err(|_| ApiError::database())?;
                let result = async {
                    let key = plan
                        .idempotency_key
                        .as_deref()
                        .ok_or_else(ApiError::database)?;
                    let receipt = reader
                        .load_receipt(plan.actor_user_id, plan.operation.as_str(), key)
                        .await
                        .map_err(|_| ApiError::database())?
                        .ok_or_else(ApiError::database)?;
                    validate_plan_receipt(&receipt, &plan)?;
                    Ok(CommittedCommand {
                        response_status: receipt.response_status,
                        response_body: receipt.response_body,
                        replayed: true,
                    })
                }
                .await;
                reader.rollback().await.map_err(|_| ApiError::database())?;
                return result;
            }
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
            validate_plan_receipt(&stored, plan).map_err(AttemptFailure::Refusal)?;
            return Ok(AttemptOutcome::Replayed(CommittedCommand {
                response_status: stored.response_status,
                response_body: stored.response_body,
                replayed: true,
            }));
        }
    }

    #[cfg(feature = "contract-tests")]
    session
        .prepare_response()
        .await
        .map_err(|error| AttemptFailure::Database {
            error,
            phase: LockPhase::Row,
        })?;

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
    // Fully own the result before COMMIT. No response read or assembly follows it.
    let prepared = CommittedCommand {
        response_status: command.response_status,
        response_body: command.response_body.clone(),
        replayed: false,
    };
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
        session.insert_receipt(&receipt).await.map_err(|error| {
            if error.sqlstate.as_deref() == Some("receipt-race") {
                AttemptFailure::ReceiptRace
            } else {
                AttemptFailure::Database {
                    error,
                    phase: LockPhase::Row,
                }
            }
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
    Ok(AttemptOutcome::Committed(prepared))
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
        #[cfg(feature = "contract-tests")]
        (_, Some("admission-refused")) => {
            expected(StatusCode::FORBIDDEN, "Fresh execution refused")
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
    #[cfg(feature = "contract-tests")]
    replay_fault: ReplayFault,
    #[cfg(feature = "contract-tests")]
    observation: Arc<ReplayObservation>,
}

impl PgWriteSession {
    fn connection(&mut self) -> Result<&mut PgConnection, WriteError> {
        #[cfg(feature = "contract-tests")]
        if self
            .observation
            .committed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.observation
                .queries_after_commit
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return Err(WriteError::other());
        }
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
            transaction.commit().await.map_err(|error| {
                // A server ErrorResponse proves failure; lost transport does not.
                if error.as_database_error().is_some() {
                    write_error(&error)
                } else {
                    WriteError::ambiguous()
                }
            })
        } else {
            transaction
                .rollback()
                .await
                .map_err(|error| write_error(&error))
        }
    }
}

impl WriteSession for PgWriteSession {
    #[cfg(feature = "contract-tests")]
    fn prepare_response<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            match &self.replay_fault {
                ReplayFault::BeforeResponse => Err(WriteError::other()),
                ReplayFault::RejectFresh => Err(WriteError {
                    sqlstate: Some("admission-refused".into()),
                    ambiguous_commit: false,
                }),
                ReplayFault::ReceiptRace(barrier) => {
                    barrier.wait().await;
                    Ok(())
                }
                _ => Ok(()),
            }
        })
    }
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
            #[cfg(feature = "contract-tests")]
            if matches!(self.replay_fault, ReplayFault::ReceiptRace(_)) {
                return Ok(());
            }
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
            .map_err(|error| {
                if is_command_receipt_collision(&error) {
                    WriteError {
                        sqlstate: Some("receipt-race".into()),
                        ambiguous_commit: false,
                    }
                } else {
                    write_error(&error)
                }
            })?;
            Ok(())
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            #[cfg(feature = "contract-tests")]
            if let ReplayFault::CommitFailure(state) = self.replay_fault {
                self.finish(false).await?;
                return Err(WriteError {
                    sqlstate: Some(state.into()),
                    ambiguous_commit: false,
                });
            }
            self.finish(true).await?;
            #[cfg(feature = "contract-tests")]
            self.observation
                .committed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            // AfterCommit leaves the connection tripwire armed. Any response
            // query now fails and increments the independently observed count.
            #[cfg(feature = "contract-tests")]
            if matches!(self.replay_fault, ReplayFault::LostCommitAck) {
                return Err(WriteError::ambiguous());
            }
            Ok(())
        })
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
        #[cfg(feature = "contract-tests")]
        replay_fault: ReplayFault::None,
        #[cfg(feature = "contract-tests")]
        observation: Arc::new(ReplayObservation::default()),
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

// Stock operations borrow the caller's transaction after L5/L8 locks. The
// complete ID sets are fixed before the first movement; no late locks occur.
use super::decimal::StockQuantity;
use super::error::{MarketErrorCode, MarketWriteError};
use super::expiry::{ExpiryEffect, ExpirySlice, MAX_EXPIRY_ORDERS};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

pub(super) fn inconsistent_stock() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::InventoryInconsistent)
}

fn stock_value(value: Decimal) -> Result<Decimal, MarketWriteError> {
    StockQuantity::checked(value)
        .map(|q| q.value())
        .map_err(|_| inconsistent_stock())
}

fn stock_add(a: Decimal, b: Decimal) -> Result<Decimal, MarketWriteError> {
    stock_value(a.checked_add(b).ok_or_else(inconsistent_stock)?)
}

fn stock_sub(a: Decimal, b: Decimal) -> Result<Decimal, MarketWriteError> {
    stock_value(a.checked_sub(b).ok_or_else(inconsistent_stock)?)
}

pub(super) struct StockLedger<'a> {
    conn: &'a mut PgConnection,
    inventory_ids: Vec<Uuid>,
    order_ids: Vec<Uuid>,
}

pub(super) struct StockOrder {
    pub id: Uuid,
    pub organization: Uuid,
    pub item: Option<Uuid>,
    pub status: String,
    pub total: Decimal,
    pub remaining: Decimal,
    pub version: i32,
    pub expires_at: Option<DateTime<Utc>>,
}

impl StockOrder {
    fn live(&self) -> bool {
        matches!(self.status.as_str(), "OPEN" | "PARTIALLY_FILLED")
    }
}

impl<'a> StockLedger<'a> {
    // Only transaction.rs can construct this port. The coordinator has already
    // acquired every declared order and inventory row in ascending UUID order.
    fn new(conn: &'a mut PgConnection, inventory_ids: &[Uuid], order_ids: &[Uuid]) -> Self {
        Self {
            conn,
            inventory_ids: inventory_ids.to_vec(),
            order_ids: order_ids.to_vec(),
        }
    }

    fn owns_item(&self, id: Uuid) -> Result<(), MarketWriteError> {
        if self.inventory_ids.contains(&id) {
            Ok(())
        } else {
            Err(inconsistent_stock())
        }
    }

    async fn order(&mut self, id: Uuid) -> Result<StockOrder, MarketWriteError> {
        if !self.order_ids.contains(&id) {
            return Err(inconsistent_stock());
        }
        let row = sqlx::query("SELECT id,organization_id,inventory_item_id,status::text AS status,quantity_mt,remaining_quantity_mt,version,expires_at FROM orderbook_orders WHERE id=$1")
            .bind(id).fetch_optional(&mut *self.conn).await?.ok_or_else(inconsistent_stock)?;
        let order = StockOrder {
            id: row.try_get("id")?,
            organization: row.try_get("organization_id")?,
            item: row.try_get("inventory_item_id")?,
            status: row.try_get("status")?,
            total: row.try_get("quantity_mt")?,
            remaining: row.try_get("remaining_quantity_mt")?,
            version: row.try_get("version")?,
            expires_at: row.try_get("expires_at")?,
        };
        if let Some(item) = order.item {
            self.owns_item(item)?;
        }
        stock_value(order.remaining)?;
        super::decimal::OrderQuantity::checked(order.total).map_err(|_| inconsistent_stock())?;
        if order.remaining > order.total || order.version < 1 {
            return Err(inconsistent_stock());
        }
        Ok(order)
    }

    async fn balance(&mut self, item: Uuid) -> Result<(Decimal, Decimal), MarketWriteError> {
        self.owns_item(item)?;
        let row = sqlx::query("SELECT current_stock_mt,reserved_stock_mt,incoming_stock_mt FROM inventory_items WHERE id=$1")
            .bind(item).fetch_optional(&mut *self.conn).await?.ok_or_else(inconsistent_stock)?;
        let current = stock_value(row.try_get("current_stock_mt")?)?;
        let reserved = stock_value(row.try_get("reserved_stock_mt")?)?;
        stock_value(row.try_get("incoming_stock_mt")?)?;
        Ok((current, reserved))
    }

    async fn committed(&mut self, order: Uuid) -> Result<Decimal, MarketWriteError> {
        let values: Vec<Decimal> = sqlx::query_scalar("SELECT quantity_mt FROM trades WHERE (bid_order_id=$1 OR ask_order_id=$1) AND status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID')")
            .bind(order).fetch_all(&mut *self.conn).await?;
        let mut total = Decimal::ZERO;
        for value in values {
            super::decimal::TradeQuantity::checked(value).map_err(|_| inconsistent_stock())?;
            total = stock_add(total, value)?;
        }
        Ok(total)
    }

    async fn check_order_remainder(
        &mut self,
        order: &StockOrder,
        remaining: Decimal,
        live: bool,
    ) -> Result<(), MarketWriteError> {
        stock_value(remaining)?;
        if remaining > order.total {
            return Err(inconsistent_stock());
        }
        if live {
            let expected = stock_sub(order.total, self.committed(order.id).await?)?;
            if expected != remaining {
                return Err(inconsistent_stock());
            }
            debug_assert_eq!(expected, remaining);
        }
        Ok(())
    }

    async fn expected_reserved(&mut self, item: Uuid) -> Result<Decimal, MarketWriteError> {
        self.projected_reserved(item, None).await
    }

    async fn projected_reserved(
        &mut self,
        item: Uuid,
        change: Option<(Uuid, Decimal, &str)>,
    ) -> Result<Decimal, MarketWriteError> {
        let orders = sqlx::query("SELECT id,quantity_mt,remaining_quantity_mt,status::text AS status FROM orderbook_orders WHERE inventory_item_id=$1")
            .bind(item).fetch_all(&mut *self.conn).await?;
        let mut expected = Decimal::ZERO;
        for row in orders {
            let id: Uuid = row.try_get("id")?;
            let total: Decimal = row.try_get("quantity_mt")?;
            let stored_remaining = stock_value(row.try_get("remaining_quantity_mt")?)?;
            super::decimal::OrderQuantity::checked(total).map_err(|_| inconsistent_stock())?;
            let stored_status: String = row.try_get("status")?;
            let (remaining, status) = match change {
                Some((changed, remaining, status)) if changed == id => {
                    (stock_value(remaining)?, status)
                }
                _ => (stored_remaining, stored_status.as_str()),
            };
            if remaining > total {
                return Err(inconsistent_stock());
            }
            if matches!(status, "OPEN" | "PARTIALLY_FILLED") {
                let calculated = stock_sub(total, self.committed(id).await?)?;
                if calculated != remaining {
                    return Err(inconsistent_stock());
                }
                debug_assert_eq!(calculated, remaining);
                expected = stock_add(expected, remaining)?;
            }
        }
        let pending: Vec<Decimal> = sqlx::query_scalar("SELECT t.quantity_mt FROM trades t WHERE t.status::text='PENDING_CONFIRMATION' AND EXISTS (SELECT 1 FROM orderbook_orders o WHERE o.inventory_item_id=$1 AND (o.id=t.bid_order_id OR o.id=t.ask_order_id))")
            .bind(item).fetch_all(&mut *self.conn).await?;
        for value in pending {
            super::decimal::TradeQuantity::checked(value).map_err(|_| inconsistent_stock())?;
            expected = stock_add(expected, value)?;
        }
        Ok(expected)
    }

    /// Checks every locked item's identity and every declared live order. The
    /// per-order check also applies to orders without inventory backing.
    pub(super) async fn verify_invariants(&mut self) -> Result<(), MarketWriteError> {
        for id in self.inventory_ids.clone() {
            let (_, reserved) = self.balance(id).await?;
            let expected = self.expected_reserved(id).await?;
            if reserved != expected {
                return Err(inconsistent_stock());
            }
            debug_assert_eq!(reserved, expected);
        }
        for id in self.order_ids.clone() {
            let order = self.order(id).await?;
            self.check_order_remainder(&order, order.remaining, order.live())
                .await?;
        }
        Ok(())
    }

    async fn write_balance(
        &mut self,
        item: Uuid,
        current: Decimal,
        reserved: Decimal,
    ) -> Result<(), MarketWriteError> {
        stock_value(current)?;
        stock_value(reserved)?;
        let expected = self.expected_reserved(item).await?;
        if reserved != expected {
            return Err(inconsistent_stock());
        }
        debug_assert_eq!(reserved, expected);
        sqlx::query(
            "UPDATE inventory_items SET current_stock_mt=$2,reserved_stock_mt=$3 WHERE id=$1",
        )
        .bind(item)
        .bind(current)
        .bind(reserved)
        .execute(&mut *self.conn)
        .await?;
        self.verify_invariants().await
    }

    /// The caller has staged the matching order/trade changes in this same
    /// transaction. Check their projected identity before the inventory write.
    pub(super) async fn reserve(
        &mut self,
        item: Uuid,
        quantity: StockQuantity,
    ) -> Result<(), MarketWriteError> {
        let (current, reserved) = self.balance(item).await?;
        let next_current = stock_sub(current, quantity.value())?;
        let next_reserved = stock_add(reserved, quantity.value())?;
        self.write_balance(item, next_current, next_reserved).await
    }

    pub(super) async fn release(
        &mut self,
        item: Uuid,
        quantity: StockQuantity,
        to_current: bool,
    ) -> Result<(), MarketWriteError> {
        let (current, reserved) = self.balance(item).await?;
        let next_current = if to_current {
            stock_add(current, quantity.value())?
        } else {
            current
        };
        let next_reserved = stock_sub(reserved, quantity.value())?;
        self.write_balance(item, next_current, next_reserved).await
    }

    pub(super) async fn consume_reserved(
        &mut self,
        item: Uuid,
        quantity: StockQuantity,
    ) -> Result<(), MarketWriteError> {
        self.release(item, quantity, false).await
    }

    pub(super) async fn withdraw_order(
        &mut self,
        order: &StockOrder,
        status: &str,
    ) -> Result<(), MarketWriteError> {
        if !order.live() || !matches!(status, "CANCELLED" | "EXPIRED") {
            return Err(inconsistent_stock());
        }
        self.verify_invariants().await?;
        let version = order
            .version
            .checked_add(1)
            .ok_or_else(inconsistent_stock)?;
        // Validate the resulting stock before changing either row.
        if let Some(item) = order.item {
            let (current, reserved) = self.balance(item).await?;
            stock_add(current, order.remaining)?;
            let projected = stock_sub(reserved, order.remaining)?;
            let expected = self
                .projected_reserved(item, Some((order.id, order.remaining, status)))
                .await?;
            if projected != expected {
                return Err(inconsistent_stock());
            }
        }
        sqlx::query("UPDATE orderbook_orders SET status=$2,version=$3 WHERE id=$1")
            .bind(order.id)
            .bind(status)
            .bind(version)
            .execute(&mut *self.conn)
            .await?;
        if let Some(item) = order.item {
            self.release(
                item,
                StockQuantity::checked(order.remaining).map_err(|_| inconsistent_stock())?,
                true,
            )
            .await?;
        }
        self.verify_invariants().await
    }

    /// The caller has changed exactly one committed trade to DECLINED/CANCELLED.
    /// Restore its quantity without resurrecting a terminal or elapsed order.
    pub(super) async fn restore_order_remainder(
        &mut self,
        id: Uuid,
        quantity: StockQuantity,
        now: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        let order = self.order(id).await?;
        let remaining = stock_add(order.remaining, quantity.value())?;
        let restorable = matches!(
            order.status.as_str(),
            "OPEN" | "PARTIALLY_FILLED" | "FILLED"
        );
        self.check_order_remainder(&order, remaining, restorable)
            .await?;
        let elapsed = restorable && order.expires_at.is_some_and(|at| at <= now);
        let release = if elapsed {
            remaining
        } else if !restorable {
            quantity.value()
        } else {
            Decimal::ZERO
        };
        if let Some(item) = order.item {
            let (current, reserved) = self.balance(item).await?;
            stock_add(current, release)?;
            stock_sub(reserved, release)?;
        }
        let status = if elapsed {
            "EXPIRED"
        } else if restorable {
            if remaining == order.total {
                "OPEN"
            } else if remaining.is_zero() {
                "FILLED"
            } else {
                "PARTIALLY_FILLED"
            }
        } else {
            order.status.as_str()
        };
        if let Some(item) = order.item {
            let (_, reserved) = self.balance(item).await?;
            let projected = stock_sub(reserved, release)?;
            let expected = self
                .projected_reserved(item, Some((id, remaining, status)))
                .await?;
            if projected != expected {
                return Err(inconsistent_stock());
            }
        }
        let version = order
            .version
            .checked_add(1)
            .ok_or_else(inconsistent_stock)?;
        sqlx::query(
            "UPDATE orderbook_orders SET remaining_quantity_mt=$2,status=$3,version=$4 WHERE id=$1",
        )
        .bind(id)
        .bind(remaining)
        .bind(status)
        .bind(version)
        .execute(&mut *self.conn)
        .await?;
        if let Some(item) = order.item
            && !release.is_zero()
        {
            self.release(
                item,
                StockQuantity::checked(release).map_err(|_| inconsistent_stock())?,
                true,
            )
            .await?;
        }
        self.verify_invariants().await
    }

    pub(super) async fn expire_slice(
        &mut self,
        slice: &ExpirySlice<'_>,
        now: DateTime<Utc>,
    ) -> Result<Vec<ExpiryEffect>, MarketWriteError> {
        super::expiry::expire_slice(self, slice, now).await
    }

    pub(super) async fn expired_orders(
        &mut self,
        slice: &ExpirySlice<'_>,
        now: DateTime<Utc>,
    ) -> Result<Vec<StockOrder>, MarketWriteError> {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE product_id=$1 AND delivery_point_id IS NOT DISTINCT FROM $2 AND availability_window=$3 AND side::text=$4 AND status::text IN ('OPEN','PARTIALLY_FILLED') AND expires_at <= $5 ORDER BY id LIMIT $6")
            .bind(slice.product_id).bind(slice.delivery_point_id).bind(slice.availability_window).bind(slice.side).bind(now).bind(MAX_EXPIRY_ORDERS as i64)
            .fetch_all(&mut *self.conn).await?;
        let mut orders = Vec::with_capacity(ids.len());
        for id in ids {
            orders.push(self.order(id).await?);
        }
        Ok(orders)
    }
}

/// A bounded snapshot of the selected expiry rows and their inventory totals.
/// Aggregate inputs stay in PostgreSQL; only one result per selected key leaves it.
pub(super) struct ExpiryBatch<'a> {
    conn: &'a mut PgConnection,
    pub orders: Vec<StockOrder>,
    balances: std::collections::BTreeMap<Uuid, (Decimal, Decimal, Decimal)>,
}

impl ExpiryBatch<'_> {
    pub(super) async fn withdraw(&mut self, order: &StockOrder) -> Result<(), MarketWriteError> {
        let version = order
            .version
            .checked_add(1)
            .ok_or_else(inconsistent_stock)?;
        let balance = if let Some(item) = order.item {
            let (current, reserved, expected) =
                *self.balances.get(&item).ok_or_else(inconsistent_stock)?;
            let next = (
                stock_add(current, order.remaining)?,
                stock_sub(reserved, order.remaining)?,
                stock_sub(expected, order.remaining)?,
            );
            if next.1 != next.2 {
                return Err(inconsistent_stock());
            }
            debug_assert_eq!(next.1, next.2);
            Some((item, next))
        } else {
            None
        };
        sqlx::query("UPDATE orderbook_orders SET status='EXPIRED',version=$2 WHERE id=$1")
            .bind(order.id)
            .bind(version)
            .execute(&mut *self.conn)
            .await?;
        if let Some((item, next)) = balance {
            sqlx::query(
                "UPDATE inventory_items SET current_stock_mt=$2,reserved_stock_mt=$3 WHERE id=$1",
            )
            .bind(item)
            .bind(next.0)
            .bind(next.1)
            .execute(&mut *self.conn)
            .await?;
            self.balances.insert(item, next);
        }
        Ok(())
    }
}

impl StockLedger<'_> {
    pub(super) async fn prepare_expiry(
        &mut self,
        slice: &ExpirySlice<'_>,
        now: DateTime<Utc>,
    ) -> Result<ExpiryBatch<'_>, MarketWriteError> {
        let rows = sqlx::query("SELECT id,organization_id,inventory_item_id,status::text AS status,quantity_mt,remaining_quantity_mt,version,expires_at FROM orderbook_orders WHERE product_id=$1 AND delivery_point_id IS NOT DISTINCT FROM $2 AND availability_window=$3 AND side::text=$4 AND status::text IN ('OPEN','PARTIALLY_FILLED') AND expires_at <= $5 ORDER BY id LIMIT $6")
            .bind(slice.product_id).bind(slice.delivery_point_id).bind(slice.availability_window).bind(slice.side).bind(now).bind(MAX_EXPIRY_ORDERS as i64)
            .fetch_all(&mut *self.conn).await?;
        let mut orders = Vec::with_capacity(rows.len());
        for row in rows {
            let order = StockOrder {
                id: row.try_get("id")?,
                organization: row.try_get("organization_id")?,
                item: row.try_get("inventory_item_id")?,
                status: row.try_get("status")?,
                total: row.try_get("quantity_mt")?,
                remaining: row.try_get("remaining_quantity_mt")?,
                version: row.try_get("version")?,
                expires_at: row.try_get("expires_at")?,
            };
            if !self.order_ids.contains(&order.id) {
                return Err(inconsistent_stock());
            }
            if let Some(item) = order.item {
                self.owns_item(item)?;
            }
            stock_value(order.remaining)?;
            super::decimal::OrderQuantity::checked(order.total)
                .map_err(|_| inconsistent_stock())?;
            if order.remaining > order.total || order.version < 1 || order.version == i32::MAX {
                return Err(inconsistent_stock());
            }
            orders.push(order);
        }
        let ids: Vec<Uuid> = orders.iter().map(|order| order.id).collect();
        let items: Vec<Uuid> = orders
            .iter()
            .filter_map(|order| order.item)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        // Each aggregate returns one row per selected order, irrespective of its
        // trade history. No LIMIT truncates the quantity being validated.
        let commitments = sqlx::query("SELECT o.id,COALESCE(sum(t.quantity_mt),0) AS committed,COALESCE(bool_and(t.quantity_mt>0 AND t.quantity_mt<=100000 AND scale(t.quantity_mt)<=2),true) AS valid FROM orderbook_orders o LEFT JOIN trades t ON (t.bid_order_id=o.id OR t.ask_order_id=o.id) AND t.status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID') WHERE o.id=ANY($1) GROUP BY o.id")
            .bind(&ids).fetch_all(&mut *self.conn).await?;
        let mut committed = std::collections::BTreeMap::new();
        for row in commitments {
            if !row.try_get::<bool, _>("valid")? {
                return Err(inconsistent_stock());
            }
            committed.insert(
                row.try_get::<Uuid, _>("id")?,
                stock_value(row.try_get("committed")?)?,
            );
        }
        for order in &orders {
            let expected = stock_sub(
                order.total,
                *committed.get(&order.id).ok_or_else(inconsistent_stock)?,
            )?;
            if expected != order.remaining {
                return Err(inconsistent_stock());
            }
            debug_assert_eq!(expected, order.remaining);
        }
        // Existing indexes and the selected inventory keys bound the result.
        // Pending quantities on terminal orders remain part of the reservation.
        let totals = sqlx::query("SELECT i.id,i.current_stock_mt,i.reserved_stock_mt,i.incoming_stock_mt,COALESCE(l.remaining,0)+COALESCE(p.pending,0) AS expected,COALESCE(l.valid,true) AND COALESCE(p.valid,true) AS valid FROM inventory_items i LEFT JOIN LATERAL (SELECT sum(o.remaining_quantity_mt) AS remaining,bool_and(o.remaining_quantity_mt>=0 AND o.remaining_quantity_mt<=o.quantity_mt AND o.quantity_mt>0 AND o.quantity_mt<=100000 AND scale(o.remaining_quantity_mt)<=2 AND scale(o.quantity_mt)<=2 AND o.remaining_quantity_mt=o.quantity_mt-COALESCE((SELECT sum(c.quantity_mt) FROM trades c WHERE (c.bid_order_id=o.id OR c.ask_order_id=o.id) AND c.status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID')),0)) AS valid FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.status::text IN ('OPEN','PARTIALLY_FILLED')) l ON true LEFT JOIN LATERAL (SELECT sum(t.quantity_mt) AS pending,bool_and(t.quantity_mt>0 AND t.quantity_mt<=100000 AND scale(t.quantity_mt)<=2) AS valid FROM trades t WHERE t.status::text='PENDING_CONFIRMATION' AND EXISTS (SELECT 1 FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND (o.id=t.bid_order_id OR o.id=t.ask_order_id))) p ON true WHERE i.id=ANY($1)")
            .bind(&items).fetch_all(&mut *self.conn).await?;
        let mut balances = std::collections::BTreeMap::new();
        for row in totals {
            let id: Uuid = row.try_get("id")?;
            let current = stock_value(row.try_get("current_stock_mt")?)?;
            let reserved = stock_value(row.try_get("reserved_stock_mt")?)?;
            stock_value(row.try_get("incoming_stock_mt")?)?;
            let expected = stock_value(row.try_get("expected")?)?;
            if !row.try_get::<bool, _>("valid")? || reserved != expected {
                return Err(inconsistent_stock());
            }
            debug_assert_eq!(reserved, expected);
            balances.insert(id, (current, reserved, expected));
        }
        if balances.len() != items.len() {
            return Err(inconsistent_stock());
        }
        // Validate every projected movement before issuing the first write.
        let mut projected = balances.clone();
        for order in &orders {
            if let Some(item) = order.item {
                let (current, reserved, expected) =
                    projected.get_mut(&item).ok_or_else(inconsistent_stock)?;
                *current = stock_add(*current, order.remaining)?;
                *reserved = stock_sub(*reserved, order.remaining)?;
                *expected = stock_sub(*expected, order.remaining)?;
                if *reserved != *expected {
                    return Err(inconsistent_stock());
                }
            }
        }
        Ok(ExpiryBatch {
            conn: self.conn,
            orders,
            balances,
        })
    }
}

#[cfg(feature = "contract-tests")]
pub mod stock_contract {
    use super::*;
    use sqlx::Connection;

    pub async fn lock(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), MarketWriteError> {
        sqlx::query("SELECT id FROM inventory_items WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(ids)
            .fetch_all(conn)
            .await?;
        Ok(())
    }

    async fn ledger(conn: &mut PgConnection) -> Result<StockLedger<'_>, MarketWriteError> {
        let orders: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM orderbook_orders ORDER BY id FOR UPDATE")
                .fetch_all(&mut *conn)
                .await?;
        let items: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM inventory_items ORDER BY id FOR UPDATE")
                .fetch_all(&mut *conn)
                .await?;
        Ok(StockLedger::new(conn, &items, &orders))
    }

    pub async fn verify(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), MarketWriteError> {
        let mut port = ledger(conn).await?;
        if ids.iter().any(|id| !port.inventory_ids.contains(id)) {
            return Err(inconsistent_stock());
        }
        port.verify_invariants().await
    }

    pub async fn reserve(
        conn: &mut PgConnection,
        id: Uuid,
        q: StockQuantity,
    ) -> Result<(), MarketWriteError> {
        let mut tx = conn.begin().await?;
        let mut port = ledger(&mut tx).await?;
        port.inventory_ids.retain(|item| *item == id);
        let orders: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM orderbook_orders WHERE inventory_item_id=$1 ORDER BY id",
        )
        .bind(id)
        .fetch_all(&mut *port.conn)
        .await?;
        port.order_ids = orders;
        port.reserve(id, q).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn consume(
        conn: &mut PgConnection,
        id: Uuid,
        q: StockQuantity,
    ) -> Result<(), MarketWriteError> {
        let mut tx = conn.begin().await?;
        ledger(&mut tx).await?.consume_reserved(id, q).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn withdraw(
        conn: &mut PgConnection,
        id: Uuid,
        status: &str,
    ) -> Result<(), MarketWriteError> {
        let mut tx = conn.begin().await?;
        let mut port = ledger(&mut tx).await?;
        let order = port.order(id).await?;
        port.withdraw_order(&order, status).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn restore(
        conn: &mut PgConnection,
        id: Uuid,
        q: StockQuantity,
        now: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        let mut tx = conn.begin().await?;
        ledger(&mut tx)
            .await?
            .restore_order_remainder(id, q, now)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn expire(
        conn: &mut PgConnection,
        product: Uuid,
        delivery: Option<Uuid>,
        window: &str,
        side: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<ExpiryEffect>, MarketWriteError> {
        let mut tx = conn.begin().await?;
        let order_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE product_id=$1 AND delivery_point_id IS NOT DISTINCT FROM $2 AND availability_window=$3 AND side::text=$4 AND status::text IN ('OPEN','PARTIALLY_FILLED') AND expires_at<=$5 ORDER BY id LIMIT 100 FOR UPDATE")
            .bind(product).bind(delivery).bind(window).bind(side).bind(now).fetch_all(&mut *tx).await?;
        let item_ids: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT inventory_item_id FROM orderbook_orders WHERE id=ANY($1) AND inventory_item_id IS NOT NULL")
            .bind(&order_ids).fetch_all(&mut *tx).await?;
        lock(&mut tx, &item_ids).await?;
        let result = super::super::expiry::expire_slice(
            &mut StockLedger::new(&mut tx, &item_ids, &order_ids),
            &ExpirySlice {
                product_id: product,
                delivery_point_id: delivery,
                availability_window: window,
                side,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}

#[cfg(feature = "contract-tests")]
#[path = "contract.rs"]
pub(super) mod contract;

fn market_refusal(code: super::error::MarketErrorCode) -> ApiError {
    let error = super::error::MarketWriteError::new(code);
    ApiError {
        status: error.status,
        detail: json!({"code":error.code.as_str(),"message":error.message}),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn validate_plan_receipt(receipt: &Receipt, plan: &WritePlan) -> Result<(), ApiError> {
    let context = CommandContext {
        actor_user_id: plan.actor_user_id,
        effective_organization_id: Some(plan.effective_organization_id),
        support_context_id: plan.support_context_id,
        idempotency_key: plan.idempotency_key.clone(),
    };
    validate_receipt(
        receipt,
        &context,
        plan.operation,
        plan.resource_id,
        &plan.request_hash,
    )
    .map_err(|error| market_refusal(error.code))
}

fn is_command_receipt_collision(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|failure| {
        failure.code().as_deref() == Some("23505")
            && failure.constraint() == Some("uq_market_command_results_actor_operation_key")
    })
}

#[cfg(feature = "contract-tests")]
#[derive(Clone, Default)]
pub(super) enum ReplayFault {
    #[default]
    None,
    RejectFresh,
    BeforeResponse,
    AfterCommit,
    LostCommitAck,
    ReceiptRace(Arc<tokio::sync::Barrier>),
    CommitFailure(&'static str),
}

#[cfg(feature = "contract-tests")]
#[derive(Default)]
struct ReplayObservation {
    committed: std::sync::atomic::AtomicBool,
    queries_after_commit: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "contract-tests")]
pub(super) struct ReplayProbe {
    pub result: Result<CommittedCommand, ApiError>,
    pub attempts: usize,
    pub queries_after_commit: usize,
}

#[cfg(feature = "contract-tests")]
pub(super) async fn replay_probe(
    pool: sqlx::PgPool,
    context: CommandContext,
    command: OrderCommand,
    fault: ReplayFault,
) -> ReplayProbe {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let observation = Arc::new(ReplayObservation::default());
    let observed = observation.clone();
    let opener = move || -> BoxFuture<'static, Result<PgWriteSession, WriteError>> {
        let pool = pool.clone();
        let mut fault = fault.clone();
        let observation = observed.clone();
        let attempt = count.fetch_add(1, Ordering::SeqCst);
        if attempt > 0 {
            fault = ReplayFault::None;
        }
        Box::pin(async move {
            let mut session = open_session(Arc::new(Semaphore::new(1)), pool).await?;
            session.replay_fault = fault;
            session.observation = observation;
            Ok(session)
        })
    };
    let result = run_write_unit(opener, context, command).await;
    ReplayProbe {
        result,
        attempts: attempts.load(Ordering::SeqCst),
        queries_after_commit: observation.queries_after_commit.load(Ordering::SeqCst),
    }
}

/// Caller supplies the recorded organization after ownership authorization;
/// current organization membership is not publication replay authority.
pub(super) async fn load_inventory_replay(
    connection: &mut PgConnection,
    actor: Uuid,
    organization: Uuid,
    key: &str,
    hash: &str,
) -> Result<Option<Uuid>, super::error::MarketWriteError> {
    let row = sqlx::query("SELECT id, owner_user_id, created_by_actor_user_id, idempotency_request_hash FROM orderbook_orders WHERE organization_id=$1 AND idempotency_operation='inventory.publish' AND idempotency_key=$2")
        .bind(organization).bind(key).fetch_optional(connection).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored_hash: Option<String> = row.try_get("idempotency_request_hash")?;
    super::command::validate_inventory_replay(
        actor,
        row.try_get("owner_user_id")?,
        row.try_get("created_by_actor_user_id")?,
        stored_hash.as_deref().unwrap_or_default(),
        hash,
    )?;
    Ok(Some(row.try_get("id")?))
}

#[cfg(feature = "contract-tests")]
pub(super) async fn inventory_replay_probe(
    pool: &sqlx::PgPool,
    actor: Uuid,
    organization: Uuid,
    key: &str,
    hash: &str,
) -> Result<Option<Uuid>, super::error::MarketWriteError> {
    let mut tx = pool.begin().await?;
    let result = load_inventory_replay(&mut tx, actor, organization, key, hash).await;
    tx.rollback().await?;
    result
}

/// Typed market commands run only inside the sealed, organization-first owner.
pub trait MarketCommand: Send + Sync + 'static {
    type Output: Send + 'static;
    fn prepare_response(&self, _value: &Self::Output) -> Result<(), MarketAttemptError> {
        Ok(())
    }
    /// Authorize the recorded resource/context and replay before fresh admission.
    /// This hook must not mutate. Recovery invokes only this hook.
    fn replay<'a>(
        &'a self,
        _tx: &'a mut LockedMarketTransaction<'_>,
        _actor: &'a super::AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
        Box::pin(async { Ok(None) })
    }
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>>;
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a super::AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketPreview {
    pub parties: super::locks::PartyLockPreview,
    pub locks: super::locks::OperationLockPlan,
}

pub struct LockedMarketTransaction<'a> {
    conn: &'a mut PgConnection,
    plan: &'a super::locks::OperationLockPlan,
    locked: &'a super::locks::Locked,
}
impl LockedMarketTransaction<'_> {
    /// Borrow only stock rows in the owner's complete locked sets (A's bridge).
    pub(super) fn stock_ledger(&mut self) -> StockLedger<'_> {
        StockLedger::new(self.conn, self.plan.inventory_ids(), self.plan.order_ids())
    }
    pub(super) async fn require_execution(
        &mut self,
        party: &super::authority::admission::PartyRef,
        now: chrono::DateTime<chrono::Utc>,
        gate_enabled: bool,
    ) -> Result<(), super::MarketWriteError> {
        super::authority::admission::require_execution(
            self.conn,
            self.locked,
            party,
            now,
            gate_enabled,
        )
        .await
    }
}

#[derive(Debug)]
pub enum MarketAttemptError {
    Database(sqlx::Error),
    Refusal(super::MarketWriteError),
    PreviewChanged,
}
impl From<sqlx::Error> for MarketAttemptError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<super::MarketWriteError> for MarketAttemptError {
    fn from(error: super::MarketWriteError) -> Self {
        Self::Refusal(error)
    }
}
impl MarketAttemptError {
    fn retryable(&self) -> bool {
        match self {
            Self::Database(error) => error
                .as_database_error()
                .and_then(|error| error.code())
                .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01")),
            Self::PreviewChanged => true,
            Self::Refusal(error) => error.is_retryable(),
        }
    }
    fn response(self) -> super::MarketWriteError {
        match self {
            Self::Database(error) => error.into(),
            Self::Refusal(error) => error,
            Self::PreviewChanged => {
                super::MarketWriteError::new(super::MarketErrorCode::TransactionRetryExhausted)
            }
        }
    }
}

/// Cancellation does not release the permit or connection while SQL still runs.
/// Preview drift and genuine aborts rebuild discovery, at most twice.
pub async fn run_market_command<C: MarketCommand>(
    pool: sqlx::PgPool,
    permits: Arc<Semaphore>,
    access: crate::auth::VerifiedAccess,
    command: C,
    class: super::authority::admission::AdmissionClass,
) -> Result<C::Output, super::MarketWriteError> {
    tokio::spawn(async move {
        let _permit = tokio::time::timeout(Duration::from_secs(30), permits.acquire_owned())
            .await
            .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::MarketBusy))?
            .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::DatabaseError))?;
        let access = Arc::new(access);
        for attempt in 0..DEFAULT_MARKET_TRANSACTION_ATTEMPTS {
            match market_attempt(&pool, &access, &command, class, false).await {
                Ok(value) => return Ok(value),
                Err(MarketAttemptError::Database(error))
                    if is_command_receipt_collision(&error) =>
                {
                    return market_attempt(&pool, &access, &command, class, true)
                        .await
                        .map_err(MarketAttemptError::response);
                }
                Err(error)
                    if error.retryable() && attempt + 1 < DEFAULT_MARKET_TRANSACTION_ATTEMPTS =>
                {
                    continue;
                }
                Err(error) => return Err(error.response()),
            }
        }
        unreachable!()
    })
    .await
    .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::DatabaseError))?
}

async fn market_attempt<C: MarketCommand>(
    pool: &sqlx::PgPool,
    access: &Arc<crate::auth::VerifiedAccess>,
    command: &C,
    class: super::authority::admission::AdmissionClass,
    recovery_only: bool,
) -> Result<C::Output, MarketAttemptError> {
    use sqlx::Connection;
    let mut connection = pool.acquire().await?;
    connection.close_on_drop();
    let mut preview_tx = connection
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY")
        .await?;
    let preview = command.preview(&mut preview_tx, access.subject).await;
    if preview_tx.rollback().await.is_err() || connection.ping().await.is_err() {
        let _ = connection.close().await;
        return Err(super::MarketWriteError::new(super::MarketErrorCode::DatabaseError).into());
    }
    let preview = preview?;
    if !preview.locks.matches_parties(&preview.parties) {
        return Err(MarketAttemptError::PreviewChanged);
    }
    let mut tx = connection
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await?;
    let result = async {
        let locked = preview.locks.acquire(&mut tx).await?;
        // Discovery includes all resource identities, party membership and slice sets.
        // A new set never adds a lock here: it aborts the complete attempt.
        let current = command.preview(&mut tx, access.subject).await?;
        if current != preview {
            return Err(MarketAttemptError::PreviewChanged);
        }
        let actor = super::authority::admission::authenticate_shared(
            &mut tx,
            &locked,
            access.clone(),
            super::authority::admission::AdmissionClass::Cleanup,
        )
        .await?;
        let mut port = LockedMarketTransaction {
            conn: &mut tx,
            plan: &preview.locks,
            locked: &locked,
        };
        if let Some(value) = command.replay(&mut port, &actor).await? {
            return Ok(value);
        }
        if recovery_only {
            return Err(
                super::MarketWriteError::new(super::MarketErrorCode::IdempotencyConflict).into(),
            );
        }
        let actor = super::authority::admission::authenticate_shared(
            port.conn,
            &locked,
            access.clone(),
            class,
        )
        .await?;
        command.execute(&mut port, &actor).await
    }
    .await;
    match result {
        Err(error) => {
            if tx.rollback().await.is_err() || connection.ping().await.is_err() {
                let _ = connection.close().await;
                return Err(
                    super::MarketWriteError::new(super::MarketErrorCode::DatabaseError).into(),
                );
            }
            let _ = connection.close().await;
            Err(error)
        }
        Ok(value) => {
            if let Err(error) = command.prepare_response(&value) {
                tx.rollback().await?;
                let _ = connection.close().await;
                return Err(error);
            }
            if let Err(error) = tx.commit().await {
                let retryable = error
                    .as_database_error()
                    .and_then(|error| error.code())
                    .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01"));
                let _ = connection.close().await;
                return Err(if retryable || error.as_database_error().is_some() {
                    MarketAttemptError::Database(error)
                } else {
                    super::MarketWriteError::new(super::MarketErrorCode::CommitOutcomeUnknown)
                        .into()
                });
            }
            // No response query or failure can turn this known commit into a rerun.
            let _ = connection.close().await;
            Ok(value)
        }
    }
}

#[cfg(feature = "contract-tests")]
impl LockedMarketTransaction<'_> {
    pub async fn test_stage_effect(&mut self) -> Result<(), super::MarketWriteError> {
        sqlx::query("UPDATE lock_effects SET value=value+1 WHERE id=1")
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
    pub async fn test_server_abort(
        &mut self,
        state: &str,
    ) -> Result<(), super::MarketWriteError> {
        let statement = match state {
            "40001" => "DO $$ BEGIN RAISE EXCEPTION 'test abort' USING ERRCODE='40001'; END $$",
            "40P01" => "DO $$ BEGIN RAISE EXCEPTION 'test abort' USING ERRCODE='40P01'; END $$",
            "55P03" => "DO $$ BEGIN RAISE EXCEPTION 'test abort' USING ERRCODE='55P03'; END $$",
            "57014" => "DO $$ BEGIN RAISE EXCEPTION 'test abort' USING ERRCODE='57014'; END $$",
            _ => {
                return Err(super::MarketWriteError::new(
                    super::MarketErrorCode::ValidationFailed,
                ));
            }
        };
        sqlx::raw_sql(statement).execute(&mut *self.conn).await?;
        Ok(())
    }
}
#[cfg(feature = "contract-tests")]
pub async fn install_acquisition_abort(conn: &mut PgConnection, state: &str) {
    let state = match state {
        "40001" => "40001",
        "40P01" => "40P01",
        "55P03" => "55P03",
        "57014" => "57014",
        _ => panic!("invalid test state"),
    };
    sqlx::raw_sql("CREATE SEQUENCE acquisition_attempt; CREATE FUNCTION acquisition_probe() RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$ BEGIN IF nextval('acquisition_attempt')=1 THEN RAISE EXCEPTION 'acquisition abort' USING ERRCODE='40001'; END IF; RETURN true; END $$; ALTER TABLE orderbook_orders ENABLE ROW LEVEL SECURITY; CREATE POLICY acquire_probe ON orderbook_orders USING(acquisition_probe()); GRANT USAGE,SELECT ON SEQUENCE acquisition_attempt TO rust_app;").execute(&mut *conn).await.unwrap();
    let body = format!(
        "CREATE OR REPLACE FUNCTION acquisition_probe() RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$ BEGIN IF nextval('acquisition_attempt')=1 THEN RAISE EXCEPTION 'acquisition abort' USING ERRCODE='{state}'; END IF; RETURN true; END $$"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(body))
        .execute(conn)
        .await
        .unwrap();
}

#[cfg(feature = "contract-tests")]
pub async fn protocol_hold_key(conn: &mut PgConnection, key: i64) {
    use sqlx::Connection;
    let mut tx = conn.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *tx)
        .await
        .unwrap();
    std::fs::write(std::env::var("RUST_PROTOCOL_READY").unwrap(), "locked").unwrap();
    let release = std::env::var("RUST_PROTOCOL_RELEASE").unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !std::path::Path::new(&release).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tx.rollback().await.unwrap();
}

#[cfg(feature = "contract-tests")]
pub async fn protocol_preview(
    conn: &mut PgConnection,
) -> (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>, Vec<super::locks::SliceKey>) {
    let organizations = sqlx::query_scalar("SELECT id FROM organizations ORDER BY id LIMIT 200")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let users = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 200")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let rows=sqlx::query("SELECT id,product_id,delivery_point_id,availability_window FROM orderbook_orders ORDER BY id LIMIT 100").fetch_all(conn).await.unwrap();
    let orders = rows.iter().map(|row| row.get("id")).collect();
    let slices = rows
        .iter()
        .map(|row| super::locks::SliceKey {
            product_id: row.get("product_id"),
            delivery_point_id: row.get("delivery_point_id"),
            availability_window: row.get("availability_window"),
        })
        .collect();
    (organizations, users, orders, slices)
}

#[cfg(feature = "contract-tests")]
pub async fn run_paused_market_probe(
    pool: sqlx::PgPool,
    permits: Arc<Semaphore>,
    access: crate::auth::VerifiedAccess,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
) -> Result<(), super::MarketWriteError> {
    struct Paused {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    impl MarketCommand for Paused {
        type Output = ();
        fn preview<'a>(
            &'a self,
            _conn: &'a mut PgConnection,
            actor: Uuid,
        ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
            Box::pin(async move {
                let parties = super::locks::PartyLockPreview::new(
                    [Uuid::from_u128(10), Uuid::from_u128(20)],
                    [Uuid::from_u128(1), Uuid::from_u128(2)],
                    actor,
                    Value::Null,
                );
                let mut locks =
                    super::locks::OperationLockPlan::new(super::locks::MarketOperation::Cancel);
                locks.parties(&parties)?;
                Ok(MarketPreview { parties, locks })
            })
        }
        fn execute<'a>(
            &'a self,
            tx: &'a mut LockedMarketTransaction<'_>,
            _actor: &'a super::AuthenticatedActor,
        ) -> BoxFuture<'a, Result<(), MarketAttemptError>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                tx.test_stage_effect().await?;
                Ok(())
            })
        }
    }
    run_market_command(
        pool,
        permits,
        access,
        Paused { entered, release },
        super::authority::admission::AdmissionClass::Cleanup,
    )
    .await
}

impl LockedMarketTransaction<'_> {
    pub(super) async fn replay_receipt(
        &mut self,
        context: &super::WriteContext,
        operation: Operation,
        resource_id: Uuid,
        key: &str,
        hash: &str,
    ) -> Result<Option<CommittedCommand>, super::MarketWriteError> {
        let normalized = normalize_idempotency_key(key)
            .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::ValidationFailed))?;
        let row=sqlx::query("SELECT request_hash,effective_organization_id,support_context_id,resource_type,resource_id,response_status,response_body::text AS response_body FROM market_command_results WHERE actor_user_id=$1 AND operation=$2 AND idempotency_key=$3")
            .bind(context.actor().user_id()).bind(operation.as_str()).bind(normalized).fetch_optional(&mut *self.conn).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let status: i16 = row.try_get("response_status")?;
        let body: Option<String> = row.try_get("response_body")?;
        let receipt = Receipt {
            request_hash: row.try_get("request_hash")?,
            effective_organization_id: row.try_get("effective_organization_id")?,
            support_context_id: row.try_get("support_context_id")?,
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
            response_status: u16::try_from(status)
                .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::DatabaseError))?,
            response_body: decode_body(body)
                .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::DatabaseError))?,
        };
        let command_context = CommandContext {
            actor_user_id: context.actor().user_id(),
            effective_organization_id: Some(context.effective_organization_id()),
            support_context_id: context.support_context_id(),
            idempotency_key: None,
        };
        validate_receipt(&receipt, &command_context, operation, resource_id, hash)?;
        Ok(Some(CommittedCommand {
            response_status: receipt.response_status,
            response_body: receipt.response_body,
            replayed: true,
        }))
    }

    pub(super) async fn record_receipt(
        &mut self,
        context: &super::WriteContext,
        record: &ReceiptRecord,
    ) -> Result<(), MarketAttemptError> {
        if record.actor_user_id != context.actor().user_id()
            || record.effective_organization_id != context.effective_organization_id()
            || record.support_context_id != context.support_context_id()
            || !(200..300).contains(&record.response_status)
        {
            return Err(
                super::MarketWriteError::new(super::MarketErrorCode::IdempotencyConflict).into(),
            );
        }
        let body = serde_json::to_string(&record.response_body)
            .map_err(|_| super::MarketWriteError::new(super::MarketErrorCode::ValidationFailed))?;
        sqlx::query("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,support_context_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::jsonb)")
            .bind(record.id).bind(record.actor_user_id).bind(record.effective_organization_id).bind(record.support_context_id).bind(record.operation.as_str()).bind(&record.idempotency_key).bind(&record.request_hash).bind(record.operation.resource_type()).bind(record.resource_id).bind(record.response_status as i16).bind(body).execute(&mut *self.conn).await?;
        Ok(())
    }

    pub(super) async fn replay_inventory(
        &mut self,
        context: &super::WriteContext,
        key: &str,
        hash: &str,
    ) -> Result<Option<Uuid>, super::MarketWriteError> {
        load_inventory_replay(
            self.conn,
            context.actor().user_id(),
            context.effective_organization_id(),
            key,
            hash,
        )
        .await
    }
}

impl LockedMarketTransaction<'_> {
    pub(super) async fn cleanup_context(
        &mut self,
        actor: &super::AuthenticatedActor,
        trade: Uuid,
        side: super::authority::TradeSide,
    ) -> Result<super::WriteContext, super::MarketWriteError> {
        let access = crate::auth::VerifiedAccess {
            subject: actor.user_id(),
            issued_at: actor.access().issued_at,
            issuance_out_of_range: actor.access().issuance_out_of_range,
        };
        super::authority::admission::cleanup_context_for_trade(
            self.conn,
            self.locked,
            access,
            trade,
            side,
        )
        .await
    }
}

#[cfg(all(test, feature = "contract-tests"))]
#[path = "ordered_contract.rs"]
mod ordered_contract;

#[cfg(all(test, feature = "contract-tests"))]
pub(super) async fn ordered_receipt_case(name: &str) -> Value {
    ordered_contract::case(name).await
}
