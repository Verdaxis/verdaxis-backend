//! `job prune-analytics`: bounded retention prune of the `user_login_days` fact.
//!
//! Interface stub: the tests in `tests/phase10_prune_postgres.rs` pin the
//! behaviour; the implementation follows in the next commit.
use crate::cli::context::RunContext;
use crate::cli::{Exit, JobArgs};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use std::path::Path;
use std::time::Duration;

/// Rows one `DELETE` statement removes at most.
pub const DEFAULT_BATCH_ROWS: i64 = 5_000;
/// Statements one run issues at most.
pub const DEFAULT_MAX_BATCHES: u32 = 200;
/// No new statement starts after this long.
pub const DEFAULT_RUN_DEADLINE: Duration = Duration::from_secs(240);

#[derive(Clone, Copy, Debug)]
pub struct PruneLimits {
    pub batch_rows: i64,
    pub max_batches: u32,
    pub deadline: Duration,
}

impl Default for PruneLimits {
    fn default() -> Self {
        Self {
            batch_rows: DEFAULT_BATCH_ROWS,
            max_batches: DEFAULT_MAX_BATCHES,
            deadline: DEFAULT_RUN_DEADLINE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    Complete,
    LimitReached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted: u64,
    pub batches: u32,
    pub completion: Completion,
    pub cutoff: NaiveDate,
}

pub struct Runtime<'a> {
    pub configured_environment: &'a str,
    pub configured_release_sha: &'a str,
    pub deploy_state: &'a Path,
    pub database: &'a str,
}

pub async fn execute(
    _job: &JobArgs,
    _runtime: &Runtime<'_>,
    _pool: &PgPool,
    now: DateTime<Utc>,
    _limits: PruneLimits,
) -> Result<PruneReport, Exit> {
    Ok(PruneReport {
        deleted: 0,
        batches: 0,
        completion: Completion::Complete,
        cutoff: now.date_naive(),
    })
}

pub async fn run(
    job: &JobArgs,
    runtime: &Runtime<'_>,
    pool: &PgPool,
    now: DateTime<Utc>,
    limits: PruneLimits,
) -> Exit {
    execute(job, runtime, pool, now, limits)
        .await
        .map_or_else(|exit| exit, |_| Exit::Ok)
}

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
