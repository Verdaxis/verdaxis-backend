//! Onboarding attention monitor (`verdaxis-read monitor onboarding`).
//!
//! Source: `app/cli/onboarding_attention.py` (alert state, delivery and the
//! command) and `app/services/onboarding_attention.py` (`load_candidates` and
//! the message copy). The stage rules are the shared classifier in
//! [`crate::business::attention`], so this alert channel and the
//! administrator read cannot disagree.
//!
//! # Order and delivery
//!
//! A run takes the lock, reads the candidates, reads the state, sends every due
//! alert, and only then publishes the new state. The alert goes out before the
//! state is recorded because a missed alert is worse than a repeated one. A
//! crash between the two repeats the alerts of that run on the next run; that
//! behaviour is accepted and is not hidden. A failed send ends the run without
//! writing state, so alerts already delivered in that run are sent again later.
//!
//! # Exclusion and the Python handover
//!
//! One process at a time runs: a nonblocking `flock` on `onboarding.lock`, a
//! separate stable file beside the state file. It is held from before the state
//! read until after the atomic state rename. The lock is not on the state file
//! itself, because the rename replaces that file. A second process exits 0
//! without a query or an alert.
//!
//! Python never takes this lock. Before ownership moves, the cutover stops the
//! Python timer, drains an active run and blocks direct Python launches; the
//! same holds for a revert. As a second line of defence, the run compares the
//! state file's identity before it publishes and refuses (exit 3) when another
//! writer replaced it meanwhile, so it never overwrites that writer.
//!
//! # State file
//!
//! `{"candidates":{key:{"stage":..,"updated_at":..[,"silent":"true"]}},"schema_version":1}`
//! with sorted keys and compact separators, exactly what Python reads and
//! writes, so either owner can follow the other.

use crate::business::attention::{Candidate, classify, format_message};
use crate::cli::context::{self, Purpose};
use crate::cli::state::{StateLock, read_bounded, write_atomic};
use crate::cli::{Environment, Exit, MonitorKind, retry_bounded};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::fmt;
use std::fs::DirBuilder;
use std::future::Future;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

/// Where the deployed monitor keeps its state file.
pub const DEFAULT_STATE_FILE: &str = "/var/lib/verdaxis-onboarding-attention/state.json";
/// The stable lock file, in the same private directory as the state file.
const LOCK_FILE_NAME: &str = "onboarding.lock";
/// The largest state file the monitor reads (plan section 6.5).
pub const MAX_STATE_BYTES: u64 = 1024 * 1024;
/// The most rows one candidate query returns before the run refuses. The
/// account set is far smaller; a larger result is refused instead of being
/// truncated, because a truncated list would hide candidates.
pub const MAX_CANDIDATES: usize = 10_000;
/// The state document version this monitor reads and writes.
const SCHEMA_VERSION: u32 = 1;
/// Python's timeout for one Telegram call.
const TELEGRAM_TIMEOUT: Duration = Duration::from_secs(10);
/// The Telegram answer is a short JSON object; anything larger is refused.
const MAX_TELEGRAM_RESPONSE_BYTES: usize = 64 * 1024;
const DEFAULT_TELEGRAM_BASE: &str = "https://api.telegram.org";
/// Python's registry name for the organization approval audit row.
const ORGANIZATION_APPROVED_ACTION: &str = "admin.organization_approved";

/// Stage records by candidate key, as the state file stores them. Entry values
/// stay a string map so a key Python wrote is kept when Rust writes the file.
pub type State = BTreeMap<String, BTreeMap<String, String>>;

/// The state document. Field order is the sorted key order Python writes.
#[derive(Serialize, Deserialize)]
struct StateDocument {
    candidates: State,
    schema_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Send due alerts and record the new state.
    Monitor,
    /// Print the alerts that would be sent; no lock, state or delivery.
    DryRun,
    /// Record the present stages as silent; no delivery.
    Bootstrap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub state_file: PathBuf,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Another process holds the lock; nothing was read or sent.
    LockBusy,
    DryRun {
        messages: Vec<String>,
    },
    Bootstrapped {
        recorded: usize,
    },
    Checked {
        candidates: usize,
        alerts: usize,
        recoveries: usize,
    },
}

/// Why a run ended without completing. Each fault maps to one exit code and
/// one outcome label; none carries an address, token or state value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The state directory, state file or lock is not a private regular file
    /// of the expected shape, or the document is not version 1.
    StateInvalid,
    /// A state read or the lock could not be opened.
    StateIo,
    /// Another writer replaced the state file during the run.
    StateChanged,
    /// The new state could not be published.
    StateWrite,
    /// The candidate query failed.
    Candidates,
    /// The candidate query returned more rows than the bound.
    CandidatesBound,
    /// An alert is due but Telegram is not configured.
    DeliveryNotConfigured,
    /// Telegram did not accept an alert.
    DeliveryFailed,
}

impl Fault {
    pub fn exit(self) -> Exit {
        match self {
            Self::StateInvalid
            | Self::StateChanged
            | Self::CandidatesBound
            | Self::DeliveryNotConfigured => Exit::Precondition,
            Self::StateIo | Self::StateWrite | Self::Candidates | Self::DeliveryFailed => {
                Exit::Dependency
            }
        }
    }

    pub fn outcome(self) -> &'static str {
        match self {
            Self::StateInvalid => "state_invalid",
            Self::StateIo => "state_unavailable",
            Self::StateChanged => "state_changed_by_another_writer",
            Self::StateWrite => "state_write_failed",
            Self::Candidates => "candidates_unavailable",
            Self::CandidatesBound => "candidates_over_bound",
            Self::DeliveryNotConfigured => "telegram_not_configured",
            Self::DeliveryFailed => "delivery_failed",
        }
    }
}

/// Why one alert was not delivered. A class only: the provider's answer and
/// the request URL carry the bot token and are never kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    NotConfigured,
    Failed,
}

/// The alert channel.
pub trait AlertSender: Sync {
    fn send(&self, message: &str) -> impl Future<Output = Result<(), SendError>> + Send;
}

/// Where the candidates come from.
pub trait CandidateSource: Sync {
    fn load(&self) -> impl Future<Output = Result<Vec<Candidate>, Exit>> + Send;
}

/// Telegram settings, read from the monitor's environment file.
#[derive(Clone, PartialEq, Eq)]
pub struct TelegramConfig {
    token: String,
    chat_id: String,
    api_base: String,
}

impl TelegramConfig {
    /// `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID` must both be nonblank after
    /// trimming. `TELEGRAM_API_BASE` defaults to the public API and loses any
    /// trailing slashes. `None` means the channel is not configured.
    pub fn from_environment(variables: &BTreeMap<String, String>) -> Option<Self> {
        let value = |name: &str| {
            variables
                .get(name)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
        };
        Some(Self {
            token: value("TELEGRAM_BOT_TOKEN")?.to_owned(),
            chat_id: value("TELEGRAM_CHAT_ID")?.to_owned(),
            api_base: variables
                .get("TELEGRAM_API_BASE")
                .map_or(DEFAULT_TELEGRAM_BASE, |base| base.as_str())
                .trim_end_matches('/')
                .to_owned(),
        })
    }
}

impl fmt::Debug for TelegramConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelegramConfig")
            .field("token", &"<redacted>")
            .field("chat_id", &"<redacted>")
            .field("api_base", &self.api_base)
            .finish()
    }
}

/// The Telegram `sendMessage` client.
///
/// A missing configuration fails at the first alert that is due, as in Python,
/// so a run with nothing to send does not need it. The client never follows a
/// redirect (the bot token is in the path), ignores proxy variables, and a send
/// is never retried: a timeout after the request left may already have
/// delivered, and a repeat would be a second alert.
pub struct TelegramSender {
    config: Option<TelegramConfig>,
    client: Option<reqwest::Client>,
}

impl TelegramSender {
    pub fn new(config: Option<TelegramConfig>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(TELEGRAM_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .ok();
        Self { config, client }
    }
}

impl AlertSender for TelegramSender {
    async fn send(&self, message: &str) -> Result<(), SendError> {
        let config = self.config.as_ref().ok_or(SendError::NotConfigured)?;
        let client = self.client.as_ref().ok_or(SendError::Failed)?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("chat_id", &config.chat_id)
            .append_pair("text", message)
            .finish();
        let mut response = client
            .post(format!(
                "{}/bot{}/sendMessage",
                config.api_base, config.token
            ))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|_| SendError::Failed)?;
        if !response.status().is_success() {
            return Err(SendError::Failed);
        }
        let mut answer = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| SendError::Failed)? {
            if answer.len() + chunk.len() > MAX_TELEGRAM_RESPONSE_BYTES {
                return Err(SendError::Failed);
            }
            answer.extend_from_slice(&chunk);
        }
        let answer: serde_json::Value =
            serde_json::from_slice(&answer).map_err(|_| SendError::Failed)?;
        if answer.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(SendError::Failed)
        }
    }
}

/// `load_candidates`: every non-administrator user with its organization,
/// current-membership join row, latest `APPROVED` status transition and latest
/// organization-approval audit row, then every unused pending registration.
///
/// Reads only. Each query returns at most `limit` rows; one more row than that
/// refuses the run, so the list is never silently truncated.
pub struct PgCandidates {
    pool: PgPool,
    limit: usize,
}

impl PgCandidates {
    pub fn new(pool: PgPool) -> Self {
        Self::with_limit(pool, MAX_CANDIDATES)
    }

    pub fn with_limit(pool: PgPool, limit: usize) -> Self {
        Self { pool, limit }
    }
}

/// `u.role::text <> 'ADMIN'` keeps Python's `User.role != ADMIN`: the
/// comparison is NULL for a NULL role, so that row is not selected either.
const USER_CANDIDATES_SQL: &str = r#"
SELECT u.id,
       u.email,
       u.first_name,
       u.last_name,
       u.role::text AS role,
       u.created_at,
       u.email_verified,
       u.email_verification_token_expires_at,
       u.status::text AS user_status,
       o.name AS organization_name,
       o.verification_status::text AS organization_status,
       o.provenance::text AS organization_provenance,
       m.status::text AS membership_status,
       m.reviewed_at AS membership_reviewed_at,
       (SELECT max(t.effective_at)
          FROM user_status_transitions t
         WHERE t.user_id = u.id AND t.to_status = 'APPROVED') AS user_approved_at,
       (SELECT max(l.timestamp)
          FROM audit_logs l
         WHERE l.action = $1 AND l.resource_id = o.id::text) AS organization_approved_at,
       u.last_login
  FROM users u
  LEFT JOIN organizations o ON o.id = u.organization_id
  LEFT JOIN organization_join_requests m
         ON m.user_id = u.id AND m.organization_id = u.organization_id
 WHERE u.role::text <> 'ADMIN'
 ORDER BY u.created_at, u.id
 LIMIT $2
"#;

const PENDING_CANDIDATES_SQL: &str = r#"
SELECT p.id,
       p.email,
       p.first_name,
       p.last_name,
       p.role::text AS role,
       p.created_at,
       p.expires_at
  FROM pending_registrations p
 WHERE p.used_at IS NULL
 ORDER BY p.created_at, p.id
 LIMIT $1
"#;

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
    role: Option<String>,
    created_at: DateTime<Utc>,
    email_verified: bool,
    email_verification_token_expires_at: Option<DateTime<Utc>>,
    user_status: Option<String>,
    organization_name: Option<String>,
    organization_status: Option<String>,
    organization_provenance: Option<String>,
    membership_status: Option<String>,
    membership_reviewed_at: Option<DateTime<Utc>>,
    user_approved_at: Option<DateTime<Utc>>,
    organization_approved_at: Option<DateTime<Utc>>,
    last_login: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct PendingRow {
    id: Uuid,
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
    role: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// Python's `_value`: the classifier compares upper-cased text.
fn upper(value: Option<String>) -> Option<String> {
    value.map(|text| text.to_uppercase())
}

impl From<UserRow> for Candidate {
    fn from(row: UserRow) -> Self {
        Self {
            key: format!("user:{}", row.id),
            email: row.email,
            first_name: row.first_name,
            last_name: row.last_name,
            role: upper(row.role),
            created_at: row.created_at,
            email_verified: row.email_verified,
            verification_expires_at: row.email_verification_token_expires_at,
            user_status: upper(row.user_status),
            organization_name: row.organization_name,
            organization_status: upper(row.organization_status),
            organization_provenance: upper(row.organization_provenance),
            membership_status: upper(row.membership_status),
            membership_reviewed_at: row.membership_reviewed_at,
            user_approved_at: row.user_approved_at,
            organization_approved_at: row.organization_approved_at,
            last_login: row.last_login,
            pending_registration_expires_at: None,
        }
    }
}

impl From<PendingRow> for Candidate {
    fn from(row: PendingRow) -> Self {
        Self {
            key: format!("pending:{}", row.id),
            email: row.email,
            first_name: row.first_name,
            last_name: row.last_name,
            role: upper(row.role),
            created_at: row.created_at,
            email_verified: false,
            verification_expires_at: None,
            user_status: None,
            organization_name: None,
            organization_status: None,
            organization_provenance: None,
            membership_status: None,
            membership_reviewed_at: None,
            user_approved_at: None,
            organization_approved_at: None,
            last_login: None,
            pending_registration_expires_at: Some(row.expires_at),
        }
    }
}

impl CandidateSource for PgCandidates {
    async fn load(&self) -> Result<Vec<Candidate>, Exit> {
        // The bound is checked against one more row than it allows.
        let fetch = i64::try_from(self.limit.saturating_add(1)).map_err(|_| Exit::Usage)?;
        let users: Vec<UserRow> = retry_bounded(|| {
            sqlx::query_as(USER_CANDIDATES_SQL)
                .bind(ORGANIZATION_APPROVED_ACTION)
                .bind(fetch)
                .fetch_all(&self.pool)
        })
        .await
        .map_err(|_| Exit::Dependency)?;
        let pending: Vec<PendingRow> = retry_bounded(|| {
            sqlx::query_as(PENDING_CANDIDATES_SQL)
                .bind(fetch)
                .fetch_all(&self.pool)
        })
        .await
        .map_err(|_| Exit::Dependency)?;
        if users.len() > self.limit || pending.len() > self.limit {
            return Err(Exit::Precondition);
        }
        Ok(users
            .into_iter()
            .map(Candidate::from)
            .chain(pending.into_iter().map(Candidate::from))
            .collect())
    }
}

/// Python's `datetime.isoformat()` for a UTC instant: fractional seconds only
/// when they are not zero, always `+00:00`. The state file stores this text.
pub fn python_timestamp(instant: DateTime<Utc>) -> String {
    if instant.timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// The name the alert shows: each part trimmed, blank parts dropped, the email
/// when nothing is left. Matches the display name of the attention message.
fn display_name(candidate: &Candidate) -> String {
    let joined = [
        candidate.first_name.as_deref(),
        candidate.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    if joined.is_empty() {
        candidate.email.clone()
    } else {
        joined
    }
}

/// `format_recovery_message`.
pub fn recovery_message(candidate: &Candidate) -> String {
    format!(
        "Verdaxis onboarding recovered\n\
         {} · {}\n\
         Organization: {}\n\
         The previously reported onboarding issue is now resolved.",
        display_name(candidate),
        candidate.email,
        candidate
            .organization_name
            .as_deref()
            .unwrap_or("Not created"),
    )
}

/// `bootstrap_state`: every candidate with an issue, recorded silent.
pub fn bootstrap_state(candidates: &[Candidate], now: DateTime<Utc>) -> State {
    candidates
        .iter()
        .filter_map(|candidate| {
            let attention = classify(candidate, now)?;
            Some((
                candidate.key.clone(),
                BTreeMap::from([
                    ("stage".to_owned(), attention.stage.as_str().to_owned()),
                    ("updated_at".to_owned(), python_timestamp(now)),
                    ("silent".to_owned(), "true".to_owned()),
                ]),
            ))
        })
        .collect()
}

/// `reconcile`: alert on a new or changed stage, send one recovery when a
/// reported issue ends (none for a silent entry), and keep every entry for a
/// candidate that is not in the list. Returns the new state with the alert and
/// recovery counts. The first failed send ends the reconcile and nothing is
/// returned, so the caller never records a stage that was not delivered.
pub async fn reconcile<S: AlertSender>(
    candidates: &[Candidate],
    previous: &State,
    now: DateTime<Utc>,
    sender: &S,
) -> Result<(State, usize, usize), SendError> {
    let mut current = previous.clone();
    let (mut alerts, mut recoveries) = (0, 0);
    for candidate in candidates {
        let prior = previous.get(&candidate.key);
        let Some(attention) = classify(candidate, now) else {
            if let Some(prior) = prior {
                if prior.get("silent").map(String::as_str) != Some("true") {
                    sender.send(&recovery_message(candidate)).await?;
                    recoveries += 1;
                }
                current.remove(&candidate.key);
            }
            continue;
        };
        let stage = attention.stage.as_str();
        if prior
            .and_then(|entry| entry.get("stage"))
            .map(String::as_str)
            == Some(stage)
        {
            continue;
        }
        sender
            .send(&format_message(candidate, &attention, now))
            .await?;
        alerts += 1;
        current.insert(
            candidate.key.clone(),
            BTreeMap::from([
                ("stage".to_owned(), stage.to_owned()),
                ("updated_at".to_owned(), python_timestamp(now)),
            ]),
        );
    }
    Ok((current, alerts, recoveries))
}

/// The state directory: created private (0700) when missing, and never a
/// symlink or writable by group or others.
fn prepare_state_directory(state_file: &Path) -> Result<PathBuf, Fault> {
    if state_file.file_name().is_none() {
        return Err(Fault::StateInvalid);
    }
    let directory = state_file
        .parent()
        .filter(|parent| parent.is_absolute())
        .ok_or(Fault::StateInvalid)?;
    match DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Fault::StateIo),
    }
    let metadata = std::fs::symlink_metadata(directory).map_err(|_| Fault::StateIo)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o022 != 0 {
        return Err(Fault::StateInvalid);
    }
    Ok(directory.to_path_buf())
}

/// Who a state file is: replaced by another writer means a different identity.
#[derive(Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
}

fn identity_of(path: &Path) -> Result<Option<Identity>, Fault> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Fault::StateIo),
    }
}

/// `load_state`: no file is an empty state; a file that is not one bounded
/// version-1 document is refused.
fn load_state(path: &Path) -> Result<State, Fault> {
    if identity_of(path)?.is_none() {
        return Ok(State::new());
    }
    let document: StateDocument = read_bounded(path, MAX_STATE_BYTES).map_err(|exit| {
        if exit == Exit::Dependency {
            Fault::StateIo
        } else {
            Fault::StateInvalid
        }
    })?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(Fault::StateInvalid);
    }
    Ok(document.candidates)
}

/// `write_state`: atomic, durable, mode 0600.
fn write_state(path: &Path, state: State) -> Result<(), Fault> {
    write_atomic(
        path,
        &StateDocument {
            candidates: state,
            schema_version: SCHEMA_VERSION,
        },
    )
    .map_err(|_| Fault::StateWrite)
}

async fn load_candidates<C: CandidateSource>(source: &C) -> Result<Vec<Candidate>, Fault> {
    source.load().await.map_err(|exit| {
        if exit == Exit::Precondition {
            Fault::CandidatesBound
        } else {
            Fault::Candidates
        }
    })
}

/// One run. `now` is explicit so a run is deterministic.
pub async fn run<C: CandidateSource, S: AlertSender>(
    source: &C,
    sender: &S,
    options: &Options,
    now: DateTime<Utc>,
) -> Result<Outcome, Fault> {
    if options.mode == Mode::DryRun {
        let candidates = load_candidates(source).await?;
        let messages = candidates
            .iter()
            .filter_map(|candidate| {
                let attention = classify(candidate, now)?;
                Some(format_message(candidate, &attention, now))
            })
            .collect();
        return Ok(Outcome::DryRun { messages });
    }

    let directory = prepare_state_directory(&options.state_file)?;
    // Held until this function returns: across the read, every send and the
    // state rename.
    let lock = StateLock::try_acquire(&directory.join(LOCK_FILE_NAME)).map_err(|exit| {
        if exit == Exit::Dependency {
            Fault::StateIo
        } else {
            Fault::StateInvalid
        }
    })?;
    let Some(_lock) = lock else {
        return Ok(Outcome::LockBusy);
    };
    let candidates = load_candidates(source).await?;

    if options.mode == Mode::Bootstrap {
        let state = bootstrap_state(&candidates, now);
        let recorded = state.len();
        write_state(&options.state_file, state)?;
        return Ok(Outcome::Bootstrapped { recorded });
    }

    let observed = identity_of(&options.state_file)?;
    let previous = load_state(&options.state_file)?;
    let (current, alerts, recoveries) = reconcile(&candidates, &previous, now, sender)
        .await
        .map_err(|error| match error {
            SendError::NotConfigured => Fault::DeliveryNotConfigured,
            SendError::Failed => Fault::DeliveryFailed,
        })?;
    // A writer that ignores the lock (unfenced Python) may have replaced the
    // file while alerts were going out. Do not overwrite its state.
    if identity_of(&options.state_file)? != observed {
        return Err(Fault::StateChanged);
    }
    write_state(&options.state_file, current)?;
    Ok(Outcome::Checked {
        candidates: candidates.len(),
        alerts,
        recoveries,
    })
}

/// The parsed `monitor onboarding` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub environment: Option<Environment>,
    pub release_sha: Option<String>,
    pub state_file: PathBuf,
    pub dry_run: bool,
    pub bootstrap: bool,
    pub telegram_disabled: bool,
}

fn report(outcome: &str, counts: serde_json::Value) {
    eprintln!(
        "{}",
        serde_json::json!({"command": "monitor onboarding", "outcome": outcome, "counts": counts})
    );
}

/// Run the command: refuse a misuse before any file or pool, open the
/// purpose-bound context, run once and report one JSON line on stderr. The dry
/// run also prints its alert text on stdout, as Python does.
pub async fn run_command(invocation: Invocation) -> Exit {
    let (Some(environment), Some(release_sha)) = (invocation.environment, invocation.release_sha)
    else {
        report("usage_refused", serde_json::json!({}));
        return Exit::Usage;
    };
    // The monitor is production-only. A disabled channel is refused too: it
    // would record stages as alerted when nothing was delivered.
    if environment != Environment::Production
        || (invocation.dry_run && invocation.bootstrap)
        || invocation.telegram_disabled
    {
        report("usage_refused", serde_json::json!({}));
        return Exit::Usage;
    }
    let mode = if invocation.dry_run {
        Mode::DryRun
    } else if invocation.bootstrap {
        Mode::Bootstrap
    } else {
        Mode::Monitor
    };
    let context = match context::open(
        Purpose::Monitor(MonitorKind::Onboarding),
        environment,
        &release_sha,
    )
    .await
    {
        Ok(context) => context,
        Err(exit) => {
            report("context_unavailable", serde_json::json!({}));
            return exit;
        }
    };
    let variables: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let sender = TelegramSender::new(TelegramConfig::from_environment(&variables));
    let options = Options {
        state_file: invocation.state_file,
        mode,
    };
    match run(
        &PgCandidates::new(context.pool),
        &sender,
        &options,
        context.now,
    )
    .await
    {
        Ok(Outcome::LockBusy) => {
            report("lock_busy", serde_json::json!({}));
            Exit::Ok
        }
        Ok(Outcome::DryRun { messages }) => {
            if messages.is_empty() {
                println!("No onboarding attention alerts.");
            } else {
                println!("{}", messages.join("\n\n"));
            }
            report("dry_run", serde_json::json!({"alerts": messages.len()}));
            Exit::Ok
        }
        Ok(Outcome::Bootstrapped { recorded }) => {
            report("bootstrapped", serde_json::json!({"recorded": recorded}));
            Exit::Ok
        }
        Ok(Outcome::Checked {
            candidates,
            alerts,
            recoveries,
        }) => {
            report(
                "checked",
                serde_json::json!({
                    "candidates": candidates,
                    "alerts": alerts,
                    "recoveries": recoveries,
                }),
            );
            Exit::Ok
        }
        Err(fault) => {
            report(fault.outcome(), serde_json::json!({}));
            fault.exit()
        }
    }
}
