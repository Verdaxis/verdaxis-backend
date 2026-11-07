//! The actorless system writer (Phase 10 Task 17; design revision 3 in
//! `outputs/research/rust-phase10/task17-system-writer-design.md`).
//!
//! A market write normally runs as an authenticated user. The demo activity
//! job has no user: its orders are SYSTEM orders of DEMO organizations with
//! no owner. This module is the only way to write without a user, and it is
//! sealed: only the commands in [`demo`] implement [`SystemMarketCommand`].
//!
//! A system attempt runs the same core as a user attempt
//! ([`super::market_attempt`]: preview, locks, drift check, commit and its
//! outcome rules). In place of user admission it runs [`system_admission`]:
//! the locked parties are organizations only, every one of them is in the
//! command's allowed set, DEMO and APPROVED. Each command also checks what it
//! wrote (the per-table invariants of revision 3) before the commit.
use std::sync::Arc;

use sqlx::PgConnection;
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::{
    AttemptAuthority, BoxFuture, DEFAULT_MARKET_TRANSACTION_ATTEMPTS, LockedMarketTransaction,
    MarketAttemptError, MarketPreview, market_attempt,
};
use crate::order_write::locks::LockClass;
use crate::order_write::{MarketErrorCode, MarketWriteError};

pub(crate) mod demo;

mod sealed {
    /// Implemented only inside `system` (by the demo commands), so no other
    /// code can write without a user.
    pub trait Sealed {}
}

/// Proof that a locked transaction passed [`system_admission`]. It carries
/// the admitted organizations; only this module creates it.
pub(crate) struct SystemWriter {
    organizations: Vec<Uuid>,
}

impl SystemWriter {
    pub(crate) fn organizations(&self) -> &[Uuid] {
        &self.organizations
    }
}

pub(crate) trait SystemMarketCommand: sealed::Sealed + Send + Sync + 'static {
    type Output: Send + 'static;
    /// The only organizations this command may lock or write.
    fn allowed_organizations(&self) -> &[Uuid];
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>>;
    fn execute<'a, 'b: 'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'b>,
        writer: &'a SystemWriter,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>>;
}

fn refused() -> MarketAttemptError {
    MarketWriteError::new(MarketErrorCode::MarketSliceChanged).into()
}

/// The system path's admission. The plan locked organizations and no user;
/// each locked organization is allowed, DEMO and APPROVED.
///
/// A plan that locks nothing at all (a phase with nothing to change) is
/// admitted with no organization: every system statement filters on the
/// admitted organizations, so such a writer can change no row.
async fn system_admission(
    tx: &mut LockedMarketTransaction<'_>,
    allowed: &[Uuid],
) -> Result<SystemWriter, MarketAttemptError> {
    let trace = tx.plan.trace();
    if trace.is_empty() {
        return Ok(SystemWriter {
            organizations: Vec::new(),
        });
    }
    if trace.iter().any(|(class, _)| *class == LockClass::User) {
        return Err(refused());
    }
    let organizations: Vec<Uuid> = trace
        .iter()
        .filter(|(class, _)| *class == LockClass::Organization)
        .map(|(_, identity)| Uuid::parse_str(identity).map_err(|_| refused()))
        .collect::<Result<_, _>>()?;
    if organizations.is_empty() || organizations.iter().any(|id| !allowed.contains(id)) {
        return Err(refused());
    }
    for id in &organizations {
        if !tx.locked.contains(LockClass::Organization, *id) {
            return Err(refused());
        }
    }
    let admitted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM organizations \
         WHERE id = ANY($1) AND provenance = 'DEMO' AND verification_status = 'APPROVED'",
    )
    .bind(&organizations)
    .fetch_one(&mut *tx.conn)
    .await?;
    if admitted != organizations.len() as i64 {
        return Err(refused());
    }
    Ok(SystemWriter { organizations })
}

struct SystemAttempt<'c, C> {
    command: &'c C,
}

impl<C: SystemMarketCommand> AttemptAuthority for SystemAttempt<'_, C> {
    type Output = C::Output;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        self.command.preview(conn)
    }
    fn admit_and_run<'a, 'b: 'a>(
        &'a self,
        port: &'a mut LockedMarketTransaction<'b>,
        recovery_only: bool,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            // A system command records no receipt, so there is nothing to
            // recover: its phases are idempotent and simply run again.
            if recovery_only {
                return Err(MarketAttemptError::RecoveryAbsent);
            }
            let writer = system_admission(port, self.command.allowed_organizations()).await?;
            self.command.execute(port, &writer).await
        })
    }
    fn replans_missing_lock_rows(&self) -> bool {
        false
    }
    fn preview_drift_error(&self) -> MarketAttemptError {
        MarketAttemptError::PreviewChanged
    }
    fn prepare_response(&self, _value: &Self::Output) -> Result<(), MarketAttemptError> {
        Ok(())
    }
}

/// Run one system command: the shared attempt core, retried once on a
/// serialization failure, a deadlock or a changed preview (as user commands
/// are). A commit whose outcome is unknown is returned, never retried.
pub(crate) async fn run_system_market_command<C: SystemMarketCommand>(
    pool: sqlx::PgPool,
    permits: Arc<Semaphore>,
    command: C,
) -> Result<C::Output, MarketWriteError> {
    let _permit = tokio::time::timeout(std::time::Duration::from_secs(30), permits.acquire_owned())
        .await
        .map_err(|_| MarketWriteError::new(MarketErrorCode::MarketBusy))?
        .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?;
    let attempt = SystemAttempt { command: &command };
    for index in 0..DEFAULT_MARKET_TRANSACTION_ATTEMPTS {
        match market_attempt(&pool, &attempt, false).await {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable() && index + 1 < DEFAULT_MARKET_TRANSACTION_ATTEMPTS => {}
            Err(error) => return Err(error.response()),
        }
    }
    unreachable!("the last attempt returns")
}
