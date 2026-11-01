//! `job prune-analytics`: bounded retention prune of the `user_login_days` fact.
//!
//! The retention rule is exactly the reader's: 800 UTC calendar dates, today
//! plus the previous 799 (`analytics::retention`). Rows dated before
//! `today - 799` are deleted; every other row, and every row of
//! `user_status_transitions` (durable business history, never pruned), is left
//! alone. The delete re-states the date predicate in the statement that
//! deletes, so no batch can remove a row inside the window.
//!
//! Work is bounded three ways: rows per statement, statements per run, and a
//! wall-clock deadline that sits inside the unit's start timeout. Each delete
//! also carries a lock-wait limit and a statement time limit that ends at the
//! run deadline. A run that stops at a row or statement bound reports
//! `batch_limit_reached` and exits 0; the rows left are older than the window
//! and the next daily run continues from the oldest.
//!
//! Refusals come before the first statement, in this order: job kind,
//! `--apply`, the expected release format, an absolute deploy-state path, the
//! configured environment and release against the command's, absent deploy
//! state, then the attested database and role on the connection that deletes.
use crate::analytics::retention::login_retention_cutoff;
use crate::cli::context::RunContext;
use crate::cli::gate::{require_apply, require_job_preflight, require_target};
use crate::cli::{Exit, JobArgs, JobKind, RETRY_DELAYS};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Connection, PgConnection, PgPool};
use std::path::Path;
use std::time::{Duration, Instant};

/// Rows one `DELETE` statement removes at most.
pub const DEFAULT_BATCH_ROWS: i64 = 5_000;
/// Statements one run issues at most: one million rows per run.
pub const DEFAULT_MAX_BATCHES: u32 = 200;
/// No new statement starts after this long. The unit allows 300 s to start and
/// finish; the margin covers the statement already in flight.
pub const DEFAULT_RUN_DEADLINE: Duration = Duration::from_secs(240);

/// How long one delete waits for a table or row lock before it gives up. This is
/// the application's own lock timeout (`DB_LOCK_TIMEOUT_MS`).
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(3);

/// SQLSTATE classes a delete may retry: serialization failure and deadlock.
const RETRYABLE_SQLSTATES: [&str; 2] = ["40001", "40P01"];

/// Oldest rows first, so a capped run always removes the longest-expired data.
/// `SKIP LOCKED` lets an overlapping run or a locked row pass without a wait.
/// The outer predicate repeats the retention rule so that deleting never
/// depends on the selection alone.
const DELETE_BATCH: &str = "\
    DELETE FROM user_login_days \
    WHERE activity_date < $1 \
      AND id IN ( \
        SELECT id FROM user_login_days \
        WHERE activity_date < $1 \
        ORDER BY activity_date, user_id \
        LIMIT $2 \
        FOR UPDATE SKIP LOCKED \
      )";

#[derive(Clone, Copy, Debug)]
pub struct PruneLimits {
    pub batch_rows: i64,
    pub max_batches: u32,
    pub deadline: Duration,
    pub lock_timeout: Duration,
}

impl Default for PruneLimits {
    fn default() -> Self {
        Self {
            batch_rows: DEFAULT_BATCH_ROWS,
            max_batches: DEFAULT_MAX_BATCHES,
            deadline: DEFAULT_RUN_DEADLINE,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// The last statement removed fewer rows than its limit: nothing expired is left.
    Complete,
    /// A run bound stopped the loop; expired rows may remain for the next run.
    LimitReached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted: u64,
    pub batches: u32,
    pub completion: Completion,
    /// First retained UTC date; rows before it were eligible.
    pub cutoff: NaiveDate,
}

/// What the deployed runtime says about itself, and the database it must write.
pub struct Runtime<'a> {
    pub configured_environment: &'a str,
    pub configured_release_sha: &'a str,
    pub deploy_state: &'a Path,
    pub database: &'a str,
}

/// A full lowercase 40-hex commit SHA, the only release identity a job accepts.
fn is_full_release_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_retryable(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| RETRYABLE_SQLSTATES.contains(&code.as_ref()))
}

/// One delete in its own transaction, with the lock wait and the statement time
/// limited for that transaction only (`set_config(.., true)` is `SET LOCAL`), so
/// the pooled connection keeps its own settings. A statement that is still
/// running when the run deadline arrives is cancelled by the server.
async fn delete_once(
    conn: &mut PgConnection,
    cutoff: NaiveDate,
    batch_rows: i64,
    lock_timeout: Duration,
    run_ends: Instant,
) -> Result<u64, sqlx::Error> {
    // Zero disables a PostgreSQL timeout, so each limit is at least 1 ms.
    let lock_ms = lock_timeout.as_millis().max(1);
    let statement_ms = run_ends
        .saturating_duration_since(Instant::now())
        .as_millis()
        .max(1);
    let mut tx = conn.begin().await?;
    sqlx::query(
        "SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)",
    )
    .bind(format!("{lock_ms}ms"))
    .bind(format!("{statement_ms}ms"))
    .execute(&mut *tx)
    .await?;
    let done = sqlx::query(DELETE_BATCH)
        .bind(cutoff)
        .bind(batch_rows)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(done.rows_affected())
}

/// One bounded delete. A serialization failure or deadlock rolls the statement
/// back and may be repeated: the delete is idempotent. Three attempts at most,
/// on the delays of the job retry policy. Any other error is final, including
/// a lock wait or statement time limit.
async fn delete_batch(
    conn: &mut PgConnection,
    cutoff: NaiveDate,
    batch_rows: i64,
    limits: PruneLimits,
    run_ends: Instant,
) -> Result<u64, Exit> {
    let mut retries = 0;
    loop {
        match delete_once(conn, cutoff, batch_rows, limits.lock_timeout, run_ends).await {
            Ok(removed) => return Ok(removed),
            Err(error) if is_retryable(&error) && retries < RETRY_DELAYS.len() => {
                tokio::time::sleep(RETRY_DELAYS[retries]).await;
                retries += 1;
            }
            Err(_) => return Err(Exit::Dependency),
        }
    }
}

async fn prune_login_days(
    conn: &mut PgConnection,
    cutoff: NaiveDate,
    limits: PruneLimits,
) -> Result<PruneReport, Exit> {
    let started = Instant::now();
    let batch_rows = limits.batch_rows.max(1);
    let mut deleted = 0_u64;
    let mut batches = 0_u32;
    loop {
        if batches >= limits.max_batches || started.elapsed() >= limits.deadline {
            return Ok(PruneReport {
                deleted,
                batches,
                completion: Completion::LimitReached,
                cutoff,
            });
        }
        let removed =
            delete_batch(conn, cutoff, batch_rows, limits, started + limits.deadline).await?;
        batches += 1;
        deleted += removed;
        if removed < batch_rows as u64 {
            return Ok(PruneReport {
                deleted,
                batches,
                completion: Completion::Complete,
                cutoff,
            });
        }
    }
}

/// Run the gates and the prune. The cutoff is fixed once from `now`'s UTC date,
/// so a run that crosses midnight never widens its own deletion.
pub async fn execute(
    job: &JobArgs,
    runtime: &Runtime<'_>,
    pool: &PgPool,
    now: DateTime<Utc>,
    limits: PruneLimits,
) -> Result<PruneReport, Exit> {
    if job.kind != JobKind::PruneAnalytics {
        return Err(Exit::Usage);
    }
    require_apply(job.apply)?;
    // The expected pair is checked again here because a caller can build a
    // `JobArgs` without the command-line parser; an empty expected and an empty
    // configured release would otherwise compare equal.
    if !is_full_release_sha(&job.identity.release_sha) {
        return Err(Exit::Usage);
    }
    // An empty or relative path would be resolved against the working directory
    // (or not at all), so a live deploy could go unseen. The shared gate refuses
    // it too; this check keeps the job safe on its own.
    if !runtime.deploy_state.is_absolute() {
        return Err(Exit::Precondition);
    }
    require_job_preflight(
        runtime.configured_environment,
        runtime.configured_release_sha,
        job.identity.environment,
        &job.identity.release_sha,
        runtime.deploy_state,
    )?;
    let cutoff = login_retention_cutoff(now.date_naive());
    let mut conn = pool.acquire().await.map_err(|_| Exit::Dependency)?;
    require_target(&mut conn, job.identity.environment, runtime.database).await?;
    prune_login_days(&mut conn, cutoff, limits).await
}

fn outcome(result: &Result<PruneReport, Exit>) -> &'static str {
    match result {
        Ok(report) if report.completion == Completion::Complete => "pruned",
        Ok(_) => "batch_limit_reached",
        Err(Exit::Usage) => "usage_refused",
        Err(Exit::Precondition) => "precondition_refused",
        Err(Exit::Dependency) => "dependency_unavailable",
        Err(Exit::Ambiguous) => "ambiguous",
        Err(Exit::Ok) => "pruned",
    }
}

/// Run the job and print its one JSON line to stderr: counts only, no row data.
pub async fn run(
    job: &JobArgs,
    runtime: &Runtime<'_>,
    pool: &PgPool,
    now: DateTime<Utc>,
    limits: PruneLimits,
) -> Exit {
    let result = execute(job, runtime, pool, now, limits).await;
    let (deleted, batches) = result
        .as_ref()
        .map_or((0, 0), |report| (report.deleted, report.batches));
    eprintln!(
        "{}",
        serde_json::json!({
            "command": "job prune-analytics",
            "outcome": outcome(&result),
            "counts": {"deleted": deleted, "batches": batches},
        })
    );
    result.map_or_else(|exit| exit, |_| Exit::Ok)
}

/// The entry the command dispatch uses once `RunContext::open` supplies a
/// purpose-bound pool. The context's identity is the deployed runtime's.
pub async fn run_with_context(
    job: &JobArgs,
    context: &RunContext,
    deploy_state: &Path,
    database: &str,
) -> Exit {
    let runtime = Runtime {
        configured_environment: context.environment.as_str(),
        configured_release_sha: &context.release_sha,
        deploy_state,
        database,
    };
    run(
        job,
        &runtime,
        &context.pool,
        context.now,
        PruneLimits::default(),
    )
    .await
}
