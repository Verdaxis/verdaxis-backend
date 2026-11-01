//! Onboarding attention monitor (`verdaxis-read monitor onboarding`).
//!
//! Interface skeleton: the behaviour lands in the next commit.

use super::super::Exit;
use crate::business::attention::Candidate;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;

/// Where the deployed monitor keeps its state file.
pub const DEFAULT_STATE_FILE: &str = "/var/lib/verdaxis-onboarding-attention/state.json";
/// The largest state file the monitor reads.
pub const MAX_STATE_BYTES: u64 = 1024 * 1024;
/// The most rows one candidate query returns before the run refuses.
pub const MAX_CANDIDATES: usize = 10_000;

/// Stage records by candidate key, as the state file stores them.
pub type State = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Monitor,
    DryRun,
    Bootstrap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub state_file: PathBuf,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    NotImplemented,
    StateInvalid,
    StateChanged,
    StateWrite,
    Candidates,
    DeliveryNotConfigured,
    DeliveryFailed,
}

impl Fault {
    pub fn exit(self) -> Exit {
        match self {
            Self::StateWrite | Self::Candidates | Self::DeliveryFailed => Exit::Dependency,
            _ => Exit::Precondition,
        }
    }

    pub fn outcome(self) -> &'static str {
        "not_implemented"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    NotConfigured,
    Failed,
}

pub trait AlertSender: Sync {
    fn send(&self, message: &str) -> impl Future<Output = Result<(), SendError>> + Send;
}

pub trait CandidateSource: Sync {
    fn load(&self) -> impl Future<Output = Result<Vec<Candidate>, Exit>> + Send;
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TelegramConfig {
    token: String,
    chat_id: String,
    api_base: String,
}

impl TelegramConfig {
    pub fn from_environment(_variables: &BTreeMap<String, String>) -> Option<Self> {
        None
    }
}

pub struct TelegramSender {
    _config: Option<TelegramConfig>,
}

impl TelegramSender {
    pub fn new(config: Option<TelegramConfig>) -> Self {
        Self { _config: config }
    }
}

impl AlertSender for TelegramSender {
    async fn send(&self, _message: &str) -> Result<(), SendError> {
        Err(SendError::NotConfigured)
    }
}

pub struct PgCandidates {
    pool: PgPool,
    limit: usize,
}

impl PgCandidates {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            limit: MAX_CANDIDATES,
        }
    }

    pub fn with_limit(pool: PgPool, limit: usize) -> Self {
        Self { pool, limit }
    }
}

impl CandidateSource for PgCandidates {
    async fn load(&self) -> Result<Vec<Candidate>, Exit> {
        let _ = (&self.pool, self.limit);
        Err(Exit::Precondition)
    }
}

pub fn python_timestamp(_instant: DateTime<Utc>) -> String {
    String::new()
}

pub fn recovery_message(_candidate: &Candidate) -> String {
    String::new()
}

pub fn bootstrap_state(_candidates: &[Candidate], _now: DateTime<Utc>) -> State {
    State::new()
}

pub async fn reconcile<S: AlertSender>(
    _candidates: &[Candidate],
    previous: &State,
    _now: DateTime<Utc>,
    _sender: &S,
) -> Result<(State, usize, usize), SendError> {
    Ok((previous.clone(), 0, 0))
}

pub async fn run<C: CandidateSource, S: AlertSender>(
    _source: &C,
    _sender: &S,
    _options: &Options,
    _now: DateTime<Utc>,
) -> Result<Outcome, Fault> {
    Err(Fault::NotImplemented)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub environment: Option<crate::cli::Environment>,
    pub release_sha: Option<String>,
    pub state_file: PathBuf,
    pub dry_run: bool,
    pub bootstrap: bool,
    pub telegram_disabled: bool,
}

pub async fn run_command(_invocation: Invocation) -> Exit {
    Exit::Precondition
}
