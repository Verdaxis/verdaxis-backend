//! Sequence assignment on one elected physical PostgreSQL session.
//!
//! The election actor owns one database session at a time. That session
//! listens for wake notifications. For each assignment batch it takes the
//! shared sequencer advisory lock, gives sequence values to committed outbox
//! rows in one transaction and releases the lock again, all on the same
//! physical connection. Python dispatchers contend for the same lock key, so
//! either runtime can sequence, but never both at the same time.
//!
//! No leadership state outlives a batch. The pool behind a session is sealed
//! as soon as its only connection is open, so the SQLx listener cannot
//! reconnect behind the actor: a lost connection is an error, and a new
//! session must be opened, attested and elected again. A session that fails in
//! a way that leaves its lock or transaction state unknown is closed and never
//! used again.

use std::{ops::ControlFlow, sync::Arc, time::Duration};

use sqlx::{
    Acquire, ConnectOptions, Connection, PgConnection, PgPool, Row,
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
};
use tokio::{
    sync::{Notify, watch},
    time::{Instant, sleep, timeout, timeout_at},
};

use crate::{Config, runtime::config::database_target};

/// Session-level advisory lock key shared with the Python dispatcher.
pub const SEQUENCER_LOCK_KEY: i64 = 7_264_537_260_226_625;
/// Producers and sequencers notify this channel after they commit.
pub const WAKE_CHANNEL: &str = "verdaxis_market_events";
/// Most rows one assignment batch sequences.
pub const ASSIGN_BATCH_LIMIT: u32 = 500;
/// Longest wait without a notification. The outbox table, not NOTIFY, is
/// authoritative, so a lost notification delays assignment by this much at most.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Pause after a failed attempt before the next one.
pub const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Consecutive failed attempts after which the actor stops.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 5;
/// Attempts without a pause for a serialization failure or a deadlock: the
/// first attempt and one immediate retry.
pub const IMMEDIATE_ATTEMPTS: u32 = 2;

/// Bound for opening and attesting a session.
const CONNECT_DEADLINE: Duration = Duration::from_secs(2);
/// Client bound for one statement: the server statement timeout below plus
/// one second for the network.
const STATEMENT_DEADLINE: Duration = Duration::from_secs(6);
/// Bound for closing a discarded session gracefully.
const CLOSE_DEADLINE: Duration = Duration::from_secs(2);
/// Buffered notifications folded into one turn before a batch runs.
const MAX_WAKES_PER_TURN: usize = 32;
/// Identifies election sessions in `pg_stat_activity`.
const APPLICATION_NAME: &str = "verdaxis-stream-election";
/// Server-side bounds for every election session. Values are milliseconds.
const SESSION_OPTIONS: [(&str, &str); 4] = [
    ("statement_timeout", "5000"),
    ("lock_timeout", "500"),
    ("idle_in_transaction_session_timeout", "10000"),
    // The session writes sequence assignments, unlike the read pools.
    ("default_transaction_read_only", "off"),
];

const ATTESTATION_SQL: &str = "SELECT current_database()::text, current_user::text, \
    current_setting('TimeZone'), pg_backend_pid(), \
    NOT rol.rolcanlogin OR rol.rolinherit OR rol.rolsuper OR rol.rolcreatedb \
    OR rol.rolcreaterole OR rol.rolreplication OR rol.rolbypassrls \
    OR EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members AS membership \
    WHERE membership.member = rol.oid OR membership.roleid = rol.oid) \
    FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user";
const SET_TIME_ZONE_SQL: &str = "SELECT set_config('TimeZone', $1, false)";
const PENDING_SQL: &str =
    "SELECT EXISTS (SELECT 1 FROM market_event_outbox WHERE stream_seq IS NULL)";
const TRY_LOCK_SQL: &str = "SELECT pg_try_advisory_lock($1)";
const UNLOCK_SQL: &str = "SELECT pg_advisory_unlock($1)";
const BEGIN_SQL: &str = "BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE";
const ASSIGN_SQL: &str = include_str!("../../sql/streams_assign.sql");
const WAKE_SQL: &str = "SELECT pg_notify($1, '')";

/// Bounded failure classes. They never carry database text or row data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectionError {
    /// The runtime configuration does not name a safe database target.
    Configuration,
    /// No session could be opened, attested and subscribed within its deadline.
    Connect,
    /// The session's database, role, role attributes or time zone are not the
    /// attested ones.
    Attestation,
    /// A serialization failure or deadlock rolled the batch back. The batch's
    /// lock acquisition was released and the session can run a new batch.
    Retryable,
    /// The server refused a statement. Any batch was rolled back and its lock
    /// acquisition released, and the session is discarded.
    Rejected,
    /// The connection failed, a deadline passed or the batch was abandoned.
    /// The batch outcome can be unknown; the session is discarded.
    SessionLost,
    /// The batch's lock release was refused or failed after its assignment
    /// committed. The session is discarded.
    UnlockFailed,
}

impl ElectionError {
    /// Bounded reason for logs and supervision.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Connect => "connect",
            Self::Attestation => "attestation",
            Self::Retryable => "retryable",
            Self::Rejected => "rejected",
            Self::SessionLost => "session_lost",
            Self::UnlockFailed => "unlock_failed",
        }
    }
}

impl std::fmt::Display for ElectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl std::error::Error for ElectionError {}

/// Result of one assignment batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOutcome {
    /// No committed row waited for a sequence value; the lock was not requested.
    NothingPending,
    /// Another session holds the sequencer lock; nothing was assigned.
    NotElected,
    /// The batch committed on the elected session and released its lock
    /// acquisition.
    Committed { assigned: u64 },
}

/// Why a wait for work ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Another session sent a wake notification.
    Notified,
    /// The poll interval passed without one.
    PollElapsed,
}

/// Points in a session's work where test builds can inject faults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The session is attested and listening.
    Opened,
    /// This batch acquired the sequencer lock.
    Elected,
    /// The assignment is written but not committed.
    Assigned,
    /// The assignment committed; the batch still holds its lock acquisition.
    Committed,
}

/// Fault injection on the pinned session. Compiled only for contract tests.
#[cfg(feature = "contract-tests")]
pub trait StageHook: Send + Sync {
    /// Runs at `stage` on the session's own connection. An error stands for a
    /// failure of the statement at that point.
    fn at<'a>(
        &'a self,
        stage: Stage,
        backend_pid: u32,
        connection: &'a mut PgConnection,
    ) -> futures_util::future::BoxFuture<'a, Result<(), sqlx::Error>>;
}

/// Test-only fault injection; empty in production builds.
#[derive(Clone, Default)]
struct Probe {
    #[cfg(feature = "contract-tests")]
    hook: Option<Arc<dyn StageHook>>,
}

impl Probe {
    #[cfg_attr(not(feature = "contract-tests"), allow(unused_variables))]
    async fn at(
        &self,
        stage: Stage,
        backend_pid: u32,
        connection: &mut PgConnection,
    ) -> Result<(), sqlx::Error> {
        #[cfg(feature = "contract-tests")]
        if let Some(hook) = &self.hook {
            return hook.at(stage, backend_pid, connection).await;
        }
        Ok(())
    }
}

/// Connection settings for election sessions, derived from the validated
/// runtime configuration.
#[derive(Clone)]
pub struct SessionSettings {
    connect: PgConnectOptions,
    database: String,
    user: String,
    time_zone: String,
    probe: Probe,
}

impl SessionSettings {
    pub fn from_config(config: &Config) -> Result<Self, ElectionError> {
        // SQLx reads PGOPTIONS, which could replace the session bounds below.
        if std::env::var_os("PGOPTIONS").is_some()
            || config
                .db_session_time_zone
                .parse::<chrono_tz::Tz>()
                .is_err()
        {
            return Err(ElectionError::Configuration);
        }
        let target = database_target(config).map_err(|_| ElectionError::Configuration)?;
        let connect = PgConnectOptions::new_without_pgpass()
            .host(&target.host)
            .port(target.port)
            .username(&target.user)
            .password(&target.password)
            .database(&target.database)
            .application_name(APPLICATION_NAME)
            .options(SESSION_OPTIONS)
            .disable_statement_logging();
        Ok(Self {
            connect,
            database: target.database,
            user: target.user,
            time_zone: config.db_session_time_zone.clone(),
            probe: Probe::default(),
        })
    }

    /// Runs `hook` at each stage of every session opened with these settings.
    #[cfg(feature = "contract-tests")]
    pub fn with_stage_hook(mut self, hook: Arc<dyn StageHook>) -> Self {
        self.probe.hook = Some(hook);
        self
    }
}

/// One attested physical session that listens for wakes and runs assignment
/// batches. It never exposes its connection.
pub struct ElectionSession {
    listener: PgListener,
    pool: PgPool,
    backend_pid: u32,
    probe: Probe,
    /// Set while a batch runs, and kept after any failure that leaves the
    /// session's lock or transaction state unknown. A poisoned session does no
    /// further work and must be closed.
    poisoned: bool,
}

impl ElectionSession {
    /// Open, attest and subscribe one session within the connect deadline.
    pub async fn open(settings: &SessionSettings) -> Result<Self, ElectionError> {
        timeout(CONNECT_DEADLINE, Self::establish(settings))
            .await
            .map_err(|_| ElectionError::Connect)?
    }

    async fn establish(settings: &SessionSettings) -> Result<Self, ElectionError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .acquire_timeout(CONNECT_DEADLINE)
            .max_lifetime(None)
            .idle_timeout(None)
            .test_before_acquire(false)
            .connect_lazy_with(settings.connect.clone());
        let mut listener = PgListener::connect_with(&pool)
            .await
            .map_err(|_| ElectionError::Connect)?;
        seal(&pool);
        // The sealed pool reports closed; the listener must keep waiting anyway.
        listener.ignore_pool_close_event(true);
        listener.eager_reconnect(false);
        let connection = (&mut listener)
            .acquire()
            .await
            .map_err(|_| ElectionError::Connect)?;
        let backend_pid = attest(connection, settings).await?;
        listener
            .listen(WAKE_CHANNEL)
            .await
            .map_err(|_| ElectionError::Connect)?;
        let connection = (&mut listener)
            .acquire()
            .await
            .map_err(|_| ElectionError::Connect)?;
        settings
            .probe
            .at(Stage::Opened, backend_pid, connection)
            .await
            .map_err(|_| ElectionError::Connect)?;
        Ok(Self {
            listener,
            pool,
            backend_pid,
            probe: settings.probe.clone(),
            poisoned: false,
        })
    }

    /// Inspect the reconnect fence in contract tests without exposing the pool.
    #[cfg(feature = "contract-tests")]
    pub fn pool_is_closed(&self) -> bool {
        self.pool.is_closed()
    }

    /// PostgreSQL backend process of this session, for diagnostics.
    pub fn backend_pid(&self) -> u32 {
        self.backend_pid
    }

    /// Run one assignment batch: elect this session for the batch, assign
    /// sequence values in one transaction and release the lock acquisition,
    /// all on this session's connection.
    pub async fn assign_one_batch(&mut self) -> Result<AssignOutcome, ElectionError> {
        if self.poisoned {
            return Err(ElectionError::SessionLost);
        }
        // Stays set if this future is dropped before the batch ends.
        self.poisoned = true;
        let result = self.run_batch().await;
        self.poisoned = !matches!(result, Ok(_) | Err(ElectionError::Retryable));
        result
    }

    async fn run_batch(&mut self) -> Result<AssignOutcome, ElectionError> {
        let backend_pid = self.backend_pid;
        let probe = &self.probe;
        // The sealed pool makes this fail, rather than reconnect, after the
        // connection is lost.
        let connection = (&mut self.listener)
            .acquire()
            .await
            .map_err(|_| ElectionError::SessionLost)?;
        let pending: bool =
            statement(sqlx::query_scalar(PENDING_SQL).fetch_one(&mut *connection)).await?;
        if !pending {
            return Ok(AssignOutcome::NothingPending);
        }
        let elected: bool = statement(
            sqlx::query_scalar(TRY_LOCK_SQL)
                .bind(SEQUENCER_LOCK_KEY)
                .fetch_one(&mut *connection),
        )
        .await?;
        if !elected {
            return Ok(AssignOutcome::NotElected);
        }
        // From here this batch holds one acquisition of the session lock.
        probe
            .at(Stage::Elected, backend_pid, connection)
            .await
            .map_err(|_| ElectionError::SessionLost)?;
        let assigned = match assign_in_transaction(connection, probe, backend_pid).await {
            Ok(assigned) => assigned,
            Err(TransactionFailure::RolledBack(error)) => {
                release(connection).await?;
                return Err(error);
            }
            // Ending the session releases the lock; no other connection may.
            Err(TransactionFailure::Unknown) => return Err(ElectionError::SessionLost),
        };
        probe
            .at(Stage::Committed, backend_pid, connection)
            .await
            .map_err(|_| ElectionError::SessionLost)?;
        release(connection).await?;
        Ok(AssignOutcome::Committed { assigned })
    }

    /// Wait until another session sends a wake notification or `poll` passes.
    /// This session's own notifications come from batches that already ran,
    /// so they do not end the wait.
    pub async fn wait_for_wake(&mut self, poll: Duration) -> Result<Wake, ElectionError> {
        if self.poisoned {
            return Err(ElectionError::SessionLost);
        }
        let deadline = Instant::now() + poll;
        loop {
            let notification = match timeout_at(deadline, self.listener.try_recv()).await {
                Err(_) => return Ok(Wake::PollElapsed),
                Ok(Ok(Some(notification))) => notification,
                // `None` means the connection was lost; the sealed pool
                // cannot replace it.
                Ok(Ok(None) | Err(_)) => {
                    self.poisoned = true;
                    return Err(ElectionError::SessionLost);
                }
            };
            if notification.process_id() != self.backend_pid {
                return Ok(Wake::Notified);
            }
        }
    }

    /// Consume notifications that arrived before a batch, which covers them.
    /// Returns whether any came from another session.
    fn take_buffered_wakes(&mut self) -> bool {
        let mut foreign = false;
        for _ in 0..MAX_WAKES_PER_TURN {
            let Some(notification) = self.listener.next_buffered() else {
                break;
            };
            foreign |= notification.process_id() != self.backend_pid;
        }
        foreign
    }

    /// Close the physical session. Ending the session releases every advisory
    /// lock it still holds, and the sealed pool never hands the connection out
    /// again.
    pub async fn close(self) {
        let Self { listener, pool, .. } = self;
        // Dropping the listener returns its connection to the sealed pool,
        // which closes it; awaiting the pool's close waits for that.
        drop(listener);
        let _ = timeout(CLOSE_DEADLINE, pool.close()).await;
    }
}

/// Mark the session pool closed. Its one connection stays usable, but the
/// pool opens no other, so a lost connection surfaces as an error instead of a
/// silent reconnect. The returned future would wait for that connection, so
/// it is dropped here; `ElectionSession::close` awaits the pool's close later.
fn seal(pool: &PgPool) {
    let _pending_close = pool.close();
}

/// Set and verify the session time zone, check the database identity and
/// role attributes, and return the backend process id.
async fn attest(
    connection: &mut PgConnection,
    settings: &SessionSettings,
) -> Result<u32, ElectionError> {
    sqlx::query(SET_TIME_ZONE_SQL)
        .bind(&settings.time_zone)
        .execute(&mut *connection)
        .await
        .map_err(|_| ElectionError::Connect)?;
    let row = sqlx::query(ATTESTATION_SQL)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|_| ElectionError::Connect)?
        .ok_or(ElectionError::Attestation)?;
    let invalid = |_| ElectionError::Attestation;
    let database: String = row.try_get(0).map_err(invalid)?;
    let user: String = row.try_get(1).map_err(invalid)?;
    let time_zone: String = row.try_get(2).map_err(invalid)?;
    let backend_pid: i32 = row.try_get(3).map_err(invalid)?;
    let unsafe_role: bool = row.try_get(4).map_err(invalid)?;
    if database != settings.database
        || user != settings.user
        || time_zone != settings.time_zone
        || unsafe_role
    {
        return Err(ElectionError::Attestation);
    }
    u32::try_from(backend_pid).map_err(|_| ElectionError::Attestation)
}

enum TransactionFailure {
    /// The server reported the failure and the rollback completed.
    RolledBack(ElectionError),
    /// The transaction may or may not have committed.
    Unknown,
}

async fn assign_in_transaction(
    connection: &mut PgConnection,
    probe: &Probe,
    backend_pid: u32,
) -> Result<u64, TransactionFailure> {
    let mut transaction = statement(connection.begin_with(BEGIN_SQL))
        .await
        .map_err(|_| TransactionFailure::Unknown)?;
    match write_assignment(&mut transaction, probe, backend_pid).await {
        Ok(assigned) => {
            statement(transaction.commit())
                .await
                .map_err(|_| TransactionFailure::Unknown)?;
            Ok(assigned)
        }
        Err(ElectionError::SessionLost) => Err(TransactionFailure::Unknown),
        Err(error) => {
            // The server aborted the transaction; confirm the rollback before
            // the lock acquisition is released.
            statement(transaction.rollback())
                .await
                .map_err(|_| TransactionFailure::Unknown)?;
            Err(TransactionFailure::RolledBack(error))
        }
    }
}

async fn write_assignment(
    connection: &mut PgConnection,
    probe: &Probe,
    backend_pid: u32,
) -> Result<u64, ElectionError> {
    let assigned = statement(
        sqlx::query(ASSIGN_SQL)
            .bind(i64::from(ASSIGN_BATCH_LIMIT))
            .execute(&mut *connection),
    )
    .await?
    .rows_affected();
    if assigned > 0 {
        // PostgreSQL delivers the wake only if this transaction commits.
        statement(
            sqlx::query(WAKE_SQL)
                .bind(WAKE_CHANNEL)
                .execute(&mut *connection),
        )
        .await?;
    }
    probe
        .at(Stage::Assigned, backend_pid, connection)
        .await
        .map_err(|error| classify(&error))?;
    Ok(assigned)
}

/// Release this batch's single lock acquisition on the same session.
async fn release(connection: &mut PgConnection) -> Result<(), ElectionError> {
    let released: bool = statement(
        sqlx::query_scalar(UNLOCK_SQL)
            .bind(SEQUENCER_LOCK_KEY)
            .fetch_one(&mut *connection),
    )
    .await
    .map_err(|_| ElectionError::UnlockFailed)?;
    if released {
        Ok(())
    } else {
        Err(ElectionError::UnlockFailed)
    }
}

/// Bound one database round trip on the client side and classify a failure.
async fn statement<T>(
    work: impl Future<Output = Result<T, sqlx::Error>>,
) -> Result<T, ElectionError> {
    match timeout(STATEMENT_DEADLINE, work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(classify(&error)),
        Err(_) => Err(ElectionError::SessionLost),
    }
}

fn classify(error: &sqlx::Error) -> ElectionError {
    let sqlx::Error::Database(database) = error else {
        return ElectionError::SessionLost;
    };
    match database.code().as_deref() {
        Some("40001" | "40P01") => ElectionError::Retryable,
        // Connection exceptions, operator intervention and idle-session
        // timeouts end the session.
        Some(code) if code.starts_with("08") || code.starts_with("57P") || code == "25P03" => {
            ElectionError::SessionLost
        }
        _ => ElectionError::Rejected,
    }
}

/// What the election actor does after a failed attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextStep {
    RetryNow,
    RetryAfter(Duration),
    Stop { attempts: u32 },
}

/// Consecutive-failure policy of the election actor. Lock contention is not a
/// failure; any successful batch resets the count.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailureBudget {
    consecutive: u32,
    immediate_retries: u32,
}

impl FailureBudget {
    pub fn record_success(&mut self) {
        *self = Self::default();
    }

    pub fn record_failure(&mut self, error: ElectionError) -> NextStep {
        self.consecutive = self.consecutive.saturating_add(1);
        if self.consecutive >= MAX_CONSECUTIVE_FAILURES {
            return NextStep::Stop {
                attempts: self.consecutive,
            };
        }
        // A rolled-back serialization failure or deadlock is retried at once
        // as a fresh transaction under a fresh lock acquisition.
        if error == ElectionError::Retryable && self.immediate_retries + 1 < IMMEDIATE_ATTEMPTS {
            self.immediate_retries += 1;
            return NextStep::RetryNow;
        }
        self.immediate_retries = 0;
        NextStep::RetryAfter(RETRY_DELAY)
    }

    /// Failed attempts since the last success.
    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }
}

/// Why the election actor stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorExit {
    /// Shutdown was requested. The session is closed.
    Shutdown,
    /// Consecutive failures reached the limit. The session is closed; the
    /// supervisor decides about a restart.
    Exhausted { attempts: u32 },
}

/// Run the election actor until `shutdown` turns true (or its sender is
/// dropped) or failures exhaust the budget. `hub_wake` is notified after this
/// actor commits an assignment and when another session sends a wake.
pub async fn run_election(
    settings: SessionSettings,
    hub_wake: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) -> ActorExit {
    let mut actor = Actor {
        settings,
        hub_wake,
        session: None,
        failures: FailureBudget::default(),
    };
    let exit = actor.run(&mut shutdown).await;
    actor.discard_session().await;
    exit
}

/// Pace of the actor after a successful batch.
enum Pace {
    /// The batch was full; run the next one at once.
    Backlog,
    /// Wait for a wake or the poll interval.
    Wait,
}

struct Actor {
    settings: SessionSettings,
    hub_wake: Arc<Notify>,
    session: Option<ElectionSession>,
    failures: FailureBudget,
}

impl Actor {
    async fn run(&mut self, shutdown: &mut watch::Receiver<bool>) -> ActorExit {
        loop {
            let attempt = tokio::select! {
                biased;
                () = stop_requested(shutdown) => return ActorExit::Shutdown,
                attempt = self.assign_turn() => attempt,
            };
            let pace = match attempt {
                Ok(pace) => {
                    self.failures.record_success();
                    pace
                }
                Err(error) => match self.after_failure(error, shutdown).await {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(exit) => return exit,
                },
            };
            if matches!(pace, Pace::Backlog) {
                continue;
            }
            let waited = tokio::select! {
                biased;
                () = stop_requested(shutdown) => return ActorExit::Shutdown,
                waited = self.wait_turn() => waited,
            };
            if let Err(error) = waited
                && let ControlFlow::Break(exit) = self.after_failure(error, shutdown).await
            {
                return exit;
            }
        }
    }

    async fn assign_turn(&mut self) -> Result<Pace, ElectionError> {
        let session = match self.session.take() {
            Some(session) => session,
            None => ElectionSession::open(&self.settings).await?,
        };
        let session = self.session.insert(session);
        if session.take_buffered_wakes() {
            self.hub_wake.notify_one();
        }
        match session.assign_one_batch().await? {
            AssignOutcome::Committed { assigned } => {
                self.hub_wake.notify_one();
                if assigned >= u64::from(ASSIGN_BATCH_LIMIT) {
                    Ok(Pace::Backlog)
                } else {
                    Ok(Pace::Wait)
                }
            }
            AssignOutcome::NotElected | AssignOutcome::NothingPending => Ok(Pace::Wait),
        }
    }

    async fn wait_turn(&mut self) -> Result<(), ElectionError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        if session.wait_for_wake(POLL_INTERVAL).await? == Wake::Notified {
            self.hub_wake.notify_one();
        }
        Ok(())
    }

    async fn after_failure(
        &mut self,
        error: ElectionError,
        shutdown: &mut watch::Receiver<bool>,
    ) -> ControlFlow<ActorExit> {
        if error != ElectionError::Retryable {
            self.discard_session().await;
        }
        let next = self.failures.record_failure(error);
        tracing::warn!(
            reason = "assignment_failed",
            class = error.reason(),
            attempt = self.failures.consecutive(),
            "stream sequencer attempt failed"
        );
        match next {
            NextStep::RetryNow => ControlFlow::Continue(()),
            NextStep::RetryAfter(delay) => tokio::select! {
                biased;
                () = stop_requested(shutdown) => ControlFlow::Break(ActorExit::Shutdown),
                () = sleep(delay) => ControlFlow::Continue(()),
            },
            NextStep::Stop { attempts } => {
                tracing::error!(
                    reason = "assignment_failed",
                    attempts,
                    "stream sequencer stopped after repeated failures"
                );
                ControlFlow::Break(ActorExit::Exhausted { attempts })
            }
        }
    }

    async fn discard_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.close().await;
        }
    }
}

/// Resolves once shutdown is requested or the sender is dropped.
async fn stop_requested(shutdown: &mut watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stop| *stop).await;
}
