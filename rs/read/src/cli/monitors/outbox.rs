//! Read-only probe of the market event outbox backlog (`monitor outbox`).
//!
//! Replaces `deploy/monitor/outbox_backlog_probe.py`. A wedged sequencer
//! leader holds its advisory lock but stops assigning sequence numbers; the
//! unsequenced backlog then grows with no other signal. The probe reports the
//! number of rows with `stream_seq IS NULL` and the age of the oldest one.
//!
//! It runs one read-only aggregate query in a `READ ONLY` transaction of a
//! session whose default is read-only, with a statement timeout, under one
//! overall deadline (20 s by default). It prints one JSON object on stdout and
//! exits as Python did, because the external monitor reads that contract:
//! 0 within the thresholds, 1 when a threshold is breached, 2 when no
//! trustworthy measurement was taken. The output never holds the DSN, a
//! database error text or a password; it is far below the caller's 4 KiB cap.
use crate::cli::{Exit, MonitorArgs};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection};
use std::future::Future;
use std::str::FromStr;
use std::time::Duration;

/// The one query, unchanged from Python.
pub const BACKLOG_QUERY: &str = "SELECT count(*), \
     COALESCE(EXTRACT(EPOCH FROM (now() - min(created_at))), 0)::bigint \
     FROM public.market_event_outbox WHERE stream_seq IS NULL";
/// Python's defaults.
pub const DEFAULT_MAX_PENDING: i64 = 1000;
pub const DEFAULT_MAX_AGE_SECONDS: i64 = 300;
pub const DEFAULT_QUERY_TIMEOUT_SECONDS: u64 = 20;
/// The longest query timeout the probe accepts: the caller's bound.
pub const MAX_QUERY_TIMEOUT_SECONDS: u64 = 20;
/// The external monitor refuses larger output.
pub const MAX_OUTPUT_BYTES: usize = 4096;
const PROBE: &str = "outbox_backlog";

/// The two backlog numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backlog {
    pub pending_count: i64,
    pub oldest_pending_seconds: i64,
}

/// The breach thresholds. A value equal to its threshold is not a breach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub max_pending: i64,
    pub max_age_seconds: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_pending: DEFAULT_MAX_PENDING,
            max_age_seconds: DEFAULT_MAX_AGE_SECONDS,
        }
    }
}

/// Why no trustworthy measurement was taken. Each has a fixed message; none
/// carries a DSN or a database error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    InvalidTarget,
    QueryFailed,
    TimedOut,
    InvalidResult,
}

impl ProbeError {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidTarget => "database target is invalid",
            Self::QueryFailed => "backlog query failed",
            Self::TimedOut => "backlog query timed out",
            Self::InvalidResult => "non-integer or negative backlog measurement",
        }
    }
}

/// Python's `evaluate`: the status document of one measurement.
pub fn evaluate(backlog: Backlog, thresholds: Thresholds, checked_at: DateTime<Utc>) -> Value {
    let mut breaches = Vec::new();
    if backlog.pending_count > thresholds.max_pending {
        breaches.push("pending_count");
    }
    if backlog.oldest_pending_seconds > thresholds.max_age_seconds {
        breaches.push("oldest_pending_seconds");
    }
    json!({
        "breaches": breaches,
        "checked_at": checked_at.to_rfc3339_opts(SecondsFormat::Micros, false),
        "max_age_seconds": thresholds.max_age_seconds,
        "max_pending": thresholds.max_pending,
        "ok": breaches.is_empty(),
        "oldest_pending_seconds": backlog.oldest_pending_seconds,
        "pending_count": backlog.pending_count,
        "probe": PROBE,
    })
}

/// The exit code and the stdout line for one probe result.
pub fn report(
    result: Result<Backlog, ProbeError>,
    thresholds: Thresholds,
    checked_at: DateTime<Utc>,
) -> (Exit, String) {
    match result {
        Ok(backlog) if backlog.pending_count >= 0 && backlog.oldest_pending_seconds >= 0 => {
            let status = evaluate(backlog, thresholds, checked_at);
            let exit = if status["ok"] == Value::Bool(true) {
                Exit::Ok
            } else {
                Exit::Breached
            };
            (exit, status.to_string())
        }
        Ok(_) => error_report(ProbeError::InvalidResult),
        Err(error) => error_report(error),
    }
}

fn error_report(error: ProbeError) -> (Exit, String) {
    let line = json!({"probe": PROBE, "ok": false, "error": error.message()});
    (Exit::Usage, line.to_string())
}

/// A database target parsed from a libpq DSN, in keyword form
/// (`host=127.0.0.1 port=5432 dbname=verdaxis_prod user=verdaxis_backup`) or
/// URL form. The password comes from `PGPASSFILE` or `~/.pgpass` unless the
/// DSN names one. `Debug` shows nothing of it.
#[derive(Clone)]
pub struct Target {
    options: PgConnectOptions,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Target(<redacted>)")
    }
}

impl Target {
    pub fn from_dsn(dsn: &str) -> Result<Self, ProbeError> {
        let url = if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
            url::Url::parse(dsn).map_err(|_| ProbeError::InvalidTarget)?
        } else {
            keyword_dsn_to_url(dsn)?
        };
        let options =
            PgConnectOptions::from_str(url.as_str()).map_err(|_| ProbeError::InvalidTarget)?;
        Ok(Self { options })
    }

    /// The session settings: read-only by default and a server-side statement
    /// timeout equal to the probe's deadline.
    fn session(&self, timeout: Duration) -> PgConnectOptions {
        let milliseconds = timeout.as_millis().to_string();
        self.options.clone().options([
            ("default_transaction_read_only", "on"),
            ("statement_timeout", milliseconds.as_str()),
        ])
    }
}

/// Parse a keyword DSN into a URL that sqlx reads. Only the connection keys the
/// probe needs are accepted; an unknown key or a malformed pair is refused.
fn keyword_dsn_to_url(dsn: &str) -> Result<url::Url, ProbeError> {
    let mut url = url::Url::parse("postgres://localhost").map_err(|_| ProbeError::InvalidTarget)?;
    for (key, value) in keyword_pairs(dsn)? {
        match key.as_str() {
            "host" | "hostaddr" if value.starts_with('/') => {
                url.query_pairs_mut().append_pair("host", &value);
            }
            "host" | "hostaddr" => url
                .set_host(Some(&value))
                .map_err(|_| ProbeError::InvalidTarget)?,
            "port" => {
                let port = value
                    .parse::<u16>()
                    .map_err(|_| ProbeError::InvalidTarget)?;
                url.set_port(Some(port))
                    .map_err(|_| ProbeError::InvalidTarget)?;
            }
            "dbname" => url.set_path(&format!("/{}", percent_encode(&value))),
            "user" => url
                .set_username(&percent_encode(&value))
                .map_err(|_| ProbeError::InvalidTarget)?,
            "password" => url
                .set_password(Some(&percent_encode(&value)))
                .map_err(|_| ProbeError::InvalidTarget)?,
            "sslmode" | "application_name" => {
                url.query_pairs_mut().append_pair(&key, &value);
            }
            _ => return Err(ProbeError::InvalidTarget),
        }
    }
    Ok(url)
}

fn percent_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `key=value` pairs separated by whitespace. A value may be single-quoted,
/// with `\'` and `\\` escapes, as libpq allows.
fn keyword_pairs(dsn: &str) -> Result<Vec<(String, String)>, ProbeError> {
    let mut pairs = Vec::new();
    let mut characters = dsn.chars().peekable();
    loop {
        while characters
            .next_if(|character| character.is_whitespace())
            .is_some()
        {}
        if characters.peek().is_none() {
            return Ok(pairs);
        }
        let mut key = String::new();
        while let Some(character) =
            characters.next_if(|character| *character != '=' && !character.is_whitespace())
        {
            key.push(character);
        }
        while characters
            .next_if(|character| character.is_whitespace())
            .is_some()
        {}
        if characters.next() != Some('=') || key.is_empty() {
            return Err(ProbeError::InvalidTarget);
        }
        while characters
            .next_if(|character| character.is_whitespace())
            .is_some()
        {}
        let mut value = String::new();
        if characters.next_if_eq(&'\'').is_some() {
            loop {
                match characters.next() {
                    Some('\\') => value.push(characters.next().ok_or(ProbeError::InvalidTarget)?),
                    Some('\'') => break,
                    Some(character) => value.push(character),
                    None => return Err(ProbeError::InvalidTarget),
                }
            }
        } else {
            while let Some(character) = characters.next_if(|character| !character.is_whitespace()) {
                value.push(character);
            }
        }
        pairs.push((key, value));
    }
}

/// Where the backlog numbers come from; tests replace it.
pub trait BacklogSource: Sync {
    fn measure(
        &self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Backlog, ProbeError>> + Send;
}

/// Open a session for the probe: read-only by default, with the statement
/// timeout, connected within `timeout`.
pub async fn open_read_only(
    target: &Target,
    timeout: Duration,
) -> Result<PgConnection, ProbeError> {
    match tokio::time::timeout(
        timeout,
        PgConnection::connect_with(&target.session(timeout)),
    )
    .await
    {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(_)) => Err(ProbeError::QueryFailed),
        Err(_) => Err(ProbeError::TimedOut),
    }
}

impl BacklogSource for Target {
    async fn measure(&self, timeout: Duration) -> Result<Backlog, ProbeError> {
        let work = async {
            let mut connection = open_read_only(self, timeout).await?;
            let mut transaction = connection
                .begin_with("BEGIN READ ONLY")
                .await
                .map_err(|_| ProbeError::QueryFailed)?;
            let row: (i64, i64) = sqlx::query_as(BACKLOG_QUERY)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| {
                    match error.as_database_error().and_then(|error| error.code()) {
                        // query_canceled: the statement timeout fired.
                        Some(code) if code == "57014" => ProbeError::TimedOut,
                        _ => ProbeError::QueryFailed,
                    }
                })?;
            let _ = transaction.rollback().await;
            let _ = connection.close().await;
            Ok(Backlog {
                pending_count: row.0,
                oldest_pending_seconds: row.1,
            })
        };
        tokio::time::timeout(timeout, work)
            .await
            .unwrap_or(Err(ProbeError::TimedOut))
    }
}

/// Measure with `source` and build the exit code and output line.
pub async fn probe(
    source: &impl BacklogSource,
    thresholds: Thresholds,
    timeout: Duration,
) -> (Exit, String) {
    let result = source.measure(timeout).await;
    report(result, thresholds, Utc::now())
}

/// The probe's settings from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeArgs {
    pub dsn: String,
    pub thresholds: Thresholds,
    pub timeout: Duration,
}

/// Validate `monitor outbox` flags: `--dsn` is required, the thresholds and
/// timeout are optional (timeout 1 to 20 s), and no other monitor's flag is
/// allowed.
pub fn parse_args(args: &MonitorArgs) -> Option<ProbeArgs> {
    if args.environment.is_some()
        || args.release_sha.is_some()
        || args.event.is_some()
        || args.remind.is_some()
        || args.reconcile.is_some()
        || args.rearm.is_some()
        || args.release.is_some()
        || args.telegram_disabled
        || args.state_file.is_some()
        || args.dry_run
        || args.bootstrap
        || args.show_emails
    {
        return None;
    }
    let timeout = args.query_timeout.unwrap_or(DEFAULT_QUERY_TIMEOUT_SECONDS);
    if !(1..=MAX_QUERY_TIMEOUT_SECONDS).contains(&timeout) {
        return None;
    }
    Some(ProbeArgs {
        dsn: args.dsn.clone()?,
        thresholds: Thresholds {
            max_pending: args.max_pending.unwrap_or(DEFAULT_MAX_PENDING),
            max_age_seconds: args.max_age_seconds.unwrap_or(DEFAULT_MAX_AGE_SECONDS),
        },
        timeout: Duration::from_secs(timeout),
    })
}

/// `verdaxis-read monitor outbox --dsn <dsn> [...]`.
pub async fn run_command(args: &MonitorArgs) -> Exit {
    let Some(probe_args) = parse_args(args) else {
        eprintln!(
            "{}",
            json!({"command": "monitor outbox", "outcome": "usage_refused", "counts": {}})
        );
        return Exit::Usage;
    };
    let (exit, line) = match Target::from_dsn(&probe_args.dsn) {
        Ok(target) => probe(&target, probe_args.thresholds, probe_args.timeout).await,
        Err(error) => error_report(error),
    };
    println!("{line}");
    let outcome = match exit {
        Exit::Ok => "ok",
        Exit::Breached => "threshold_breached",
        _ => "probe_error",
    };
    eprintln!(
        "{}",
        json!({"command": "monitor outbox", "outcome": outcome, "counts": {}})
    );
    exit
}
