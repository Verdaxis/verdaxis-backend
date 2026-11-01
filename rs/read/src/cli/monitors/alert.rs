//! Alert dispatch for the local monitors (`verdaxis-read monitor alert`).
//!
//! Replaces `deploy/monitor/alert_dispatch.py`. The monitors admit each
//! observation into their own ordered journal (see [`super::alert_journal`]);
//! this dispatcher delivers the journal's oldest entry and nothing else.
//!
//! # Wakeups and order
//!
//! `--event <check>` is a wakeup for one check, not a message: the old
//! `<check>-failure` and `<check>-healthy` forms are accepted and their state
//! part is ignored. Every wakeup, including a healthy one that runs first,
//! attempts only the oldest unacknowledged sequence, so a later transition can
//! never overtake an earlier one. Each invocation handles at most one head.
//!
//! # Locks
//!
//! The stable `.<check>.lock` in the alert state directory (the same path the
//! Python dispatcher used) is held across delivery and state publication. The
//! producer's journal lock is taken only briefly to select or remove an entry,
//! never during network I/O. Lock order is delivery lock, then journal lock.
//! Checks do not share a lock. A lock not acquired within 20 s exits 4.
//!
//! # Progress and recovery
//!
//! The dispatcher's private `<check>.delivery.json` records the head in
//! progress: its attempts (at most three, at least 5 s apart, kept across
//! crashes), and per destination whether a send is pending, in flight,
//! confirmed, skipped or uncertain. An attempt is recorded before network I/O.
//! A crash during a send, or a send whose result is unknown, leaves that
//! destination uncertain and blocks the check until an operator reconciles it.
//! Exhaustion also blocks the check until the operator re-arms the same
//! sequence. Confirmed destinations are never sent again.
//!
//! After every required destination is confirmed, the dispatcher publishes the
//! Python-compatible `<check>.json` (schema 3, receipts), records the head as
//! acknowledged, and only then removes the entry under the journal lock. A
//! crash between acknowledgement and removal resumes with the removal, without
//! a resend.
//!
//! # Python coexistence fence
//!
//! The native dispatcher keeps the stable delivery lock read-only (0400). The
//! unchanged Python dispatcher opens that lock for writing, so a direct Python
//! invocation fails before it reads or sends anything. `--release <check>`
//! makes the lock writable again only when the journal is empty and no head is
//! in progress, so Python can take over after a revert.
use crate::cli::monitors::alert_journal::{
    AlertState, Check, Head, HeldLock, JournalPaths, JournalReader, LOCK_TIMEOUT, LockFailure,
    Sequence, lock_with_timeout,
};
use crate::cli::monitors::status::{parse_bounded_timestamp, parse_strict_json, utc_text};
use crate::cli::state::{read_bounded, write_atomic};
use crate::cli::{Exit, MonitorArgs};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::future::Future;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Most delivery attempts of one head, across crashes and restarts.
pub const MAX_ATTEMPTS: u32 = 3;
/// Shortest time between two attempts of the same head.
pub const ATTEMPT_SPACING: Duration = Duration::from_secs(5);
/// Bound of one check's work in one invocation.
pub const WORKER_BUDGET: Duration = Duration::from_secs(60);
/// A failure is repeated once an hour while it lasts (Python's
/// `HOURLY_REPEAT_SECONDS`).
pub const REMINDER_INTERVAL_SECONDS: i64 = 3600;
/// Timeout of one provider request (Python's `_post` timeout).
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// The installed private alert state directory.
pub const INSTALLED_ALERT_DIRECTORY: &str = "/var/lib/verdaxis-monitor-alert";
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const LEGACY_SCHEMA_VERSION: u32 = 3;
const DELIVERY_SCHEMA_VERSION: u32 = 1;
/// Python's bound for `<check>.json`.
const MAX_LEGACY_BYTES: u64 = 16_384;
const MAX_DELIVERY_BYTES: u64 = 65_536;
/// Python refuses a state document observed more than this far in the future.
const LEGACY_FUTURE_TOLERANCE_SECONDS: i64 = 300;
const DEFAULT_TELEGRAM_BASE: &str = "https://api.telegram.org";
/// Mode of the stable delivery lock while the native dispatcher owns a check.
const FENCED_LOCK_MODE: u32 = 0o400;
/// Mode Python creates and expects for the stable delivery lock.
const PYTHON_LOCK_MODE: u32 = 0o600;

// ---- destinations and settings -----------------------------------------------------

/// Where an alert goes. The order is the delivery order Python used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Destination {
    Healthchecks,
    Telegram,
}

impl Destination {
    pub const ALL: [Destination; 2] = [Destination::Healthchecks, Destination::Telegram];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthchecks => "healthchecks",
            Self::Telegram => "telegram",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "healthchecks" => Some(Self::Healthchecks),
            "telegram" => Some(Self::Telegram),
            _ => None,
        }
    }
}

/// The wording of one alert, as Python chose it from the receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Kind {
    Failure,
    Healthy,
    Recovery,
    Reminder,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failure => "FAILURE",
            Self::Healthy => "HEALTHY",
            Self::Recovery => "RECOVERY",
            Self::Reminder => "REMINDER",
        }
    }
}

/// Telegram settings. The token and chat id never appear in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct TelegramSettings {
    token: String,
    chat_id: String,
    api_base: String,
}

/// The destinations configured in the alert environment (`alert.env`).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AlertSettings {
    telegram: Option<TelegramSettings>,
    healthchecks: BTreeMap<Check, String>,
}

impl AlertSettings {
    /// Read the settings the way Python did: values are trimmed, a blank value
    /// is absent, and Telegram needs both the token and the chat id.
    pub fn from_variables(variables: &BTreeMap<String, String>) -> Self {
        let value = |name: &str| {
            variables
                .get(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let telegram = match (value("TELEGRAM_BOT_TOKEN"), value("TELEGRAM_CHAT_ID")) {
            (Some(token), Some(chat_id)) => Some(TelegramSettings {
                token,
                chat_id,
                api_base: variables
                    .get("TELEGRAM_API_BASE")
                    .map_or(DEFAULT_TELEGRAM_BASE, String::as_str)
                    .trim_end_matches('/')
                    .to_owned(),
            }),
            _ => None,
        };
        let mut healthchecks = BTreeMap::new();
        for check in Check::ALL {
            let name = format!("HEALTHCHECKS_{}_URL", check.as_str().to_ascii_uppercase());
            if let Some(url) = value(&name) {
                healthchecks.insert(check, url);
            }
        }
        Self {
            telegram,
            healthchecks,
        }
    }

    /// The settings from this process's environment. Variables that are not
    /// UTF-8 are ignored.
    pub fn from_process_environment() -> Self {
        let variables = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        Self::from_variables(&variables)
    }

    /// The destinations configured for `check`, in delivery order.
    pub fn configured(&self, check: Check) -> Vec<Destination> {
        Destination::ALL
            .into_iter()
            .filter(|destination| match destination {
                Destination::Healthchecks => self.healthchecks.contains_key(&check),
                Destination::Telegram => self.telegram.is_some(),
            })
            .collect()
    }
}

impl fmt::Debug for AlertSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AlertSettings")
            .field("telegram", &self.telegram.as_ref().map(|_| "<redacted>"))
            .field(
                "healthchecks",
                &self.healthchecks.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

// ---- transport and clock --------------------------------------------------------

/// One send to one destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub check: Check,
    pub state: AlertState,
    pub kind: Kind,
    pub destination: Destination,
    /// The journal sequence, or `None` for an hourly reminder.
    pub sequence: Option<Sequence>,
}

impl Delivery {
    /// The Telegram text, exactly Python's wording.
    pub fn text(&self) -> String {
        format!(
            "Verdaxis local {}: {}",
            self.check.as_str(),
            self.kind.as_str()
        )
    }
}

/// What a destination did with one send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// The destination confirmed it.
    Delivered,
    /// The destination certainly did not take it: no connection, or a refusal.
    NotDelivered,
    /// The request may have taken effect, but no answer confirms it.
    Uncertain,
}

/// The network side, replaced by fakes in tests.
pub trait Transport: Sync {
    fn send(
        &self,
        settings: &AlertSettings,
        delivery: &Delivery,
    ) -> impl Future<Output = SendOutcome> + Send;
}

/// The provider client: one POST per send, 10 s timeout, no redirect, no proxy,
/// no retry. A request that may have reached the provider without a confirmed
/// answer is [`SendOutcome::Uncertain`]; a repeat could duplicate the alert.
pub struct HttpTransport {
    client: Option<reqwest::Client>,
}

impl HttpTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .ok();
        Self { client }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// How a request error is classified: a failure to connect never sent the
/// request; anything later (a timeout, a broken body) may have.
fn classify_error(error: &reqwest::Error) -> SendOutcome {
    if error.is_connect() || error.is_builder() {
        SendOutcome::NotDelivered
    } else {
        SendOutcome::Uncertain
    }
}

impl Transport for HttpTransport {
    async fn send(&self, settings: &AlertSettings, delivery: &Delivery) -> SendOutcome {
        let Some(client) = &self.client else {
            return SendOutcome::NotDelivered;
        };
        match delivery.destination {
            Destination::Healthchecks => {
                let Some(url) = settings.healthchecks.get(&delivery.check) else {
                    return SendOutcome::NotDelivered;
                };
                let mut target = url.trim_end_matches('/').to_owned();
                if delivery.state == AlertState::Failure {
                    target.push_str("/fail");
                }
                match client.post(target).send().await {
                    Ok(response) if response.status().is_success() => SendOutcome::Delivered,
                    Ok(_) => SendOutcome::NotDelivered,
                    Err(error) => classify_error(&error),
                }
            }
            Destination::Telegram => {
                let Some(telegram) = &settings.telegram else {
                    return SendOutcome::NotDelivered;
                };
                let url = format!("{}/bot{}/sendMessage", telegram.api_base, telegram.token);
                let body = serde_json::json!({
                    "chat_id": telegram.chat_id,
                    "text": delivery.text(),
                });
                let mut response = match client.post(url).json(&body).send().await {
                    Ok(response) => response,
                    Err(error) => return classify_error(&error),
                };
                if !response.status().is_success() {
                    return SendOutcome::NotDelivered;
                }
                let mut answer = Vec::new();
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if answer.len() + chunk.len() > MAX_RESPONSE_BYTES {
                                return SendOutcome::Uncertain;
                            }
                            answer.extend_from_slice(&chunk);
                        }
                        Ok(None) => break,
                        Err(_) => return SendOutcome::Uncertain,
                    }
                }
                match serde_json::from_slice::<serde_json::Value>(&answer)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                {
                    Some(true) => SendOutcome::Delivered,
                    Some(false) => SendOutcome::NotDelivered,
                    None => SendOutcome::Uncertain,
                }
            }
        }
    }
}

/// Time, replaced by a fake clock in tests.
pub trait Clock: Sync {
    fn now(&self) -> DateTime<Utc>;
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Points inside one delivery. Tests stop a real dispatcher process at each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStage {
    /// The attempt is recorded as in flight; the send has not started.
    InFlight(Destination),
    /// The Python-compatible state document is published; the head is not
    /// acknowledged yet.
    LegacyPublished,
    /// The head is acknowledged; its entry is not removed yet.
    Acknowledged,
}

/// Observes the stages of a delivery.
pub trait DispatchHook: Send + Sync {
    fn reached(&self, stage: DispatchStage);
}

// ---- state documents --------------------------------------------------------------

/// One legacy receipt, as Python wrote it. Fields are in key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub delivered_at: String,
    pub receipt_id: String,
}

/// `<check>.json`: the Python-compatible state document (schema 3). The native
/// dispatcher keeps it current so a revert can hand the same receipts back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyState {
    pub observed_at: String,
    pub receipts: BTreeMap<Destination, BTreeMap<AlertState, Receipt>>,
    pub schema_version: u32,
    pub state: AlertState,
}

/// Python's `valid_alert_state`, for a parsed document.
fn valid_legacy_state(value: &serde_json::Value, now: DateTime<Utc>) -> Option<LegacyState> {
    let document: LegacyState = serde_json::from_value(value.clone()).ok()?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(LEGACY_SCHEMA_VERSION))
    {
        return None;
    }
    let observed_at = parse_legacy_time(&document.observed_at)?;
    if (observed_at - now).num_seconds() > LEGACY_FUTURE_TOLERANCE_SECONDS {
        return None;
    }
    for receipts in document.receipts.values() {
        if receipts.is_empty() {
            return None;
        }
        for receipt in receipts.values() {
            let hex = receipt.receipt_id.len() == 32
                && receipt
                    .receipt_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            let delivered_at = parse_legacy_time(&receipt.delivered_at)?;
            if !hex || delivered_at > observed_at {
                return None;
            }
        }
    }
    Some(document)
}

fn parse_legacy_time(text: &str) -> Option<DateTime<Utc>> {
    if text.chars().count() > 40 {
        return None;
    }
    parse_bounded_timestamp(text)
}

/// Progress of one destination for the head in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationStatus {
    /// Due and not confirmed; the next attempt sends it.
    Pending,
    /// A send started and its result is not recorded.
    InFlight,
    /// The destination confirmed it.
    Delivered,
    /// Not due under the receipt rules; nothing is sent.
    Skipped,
    /// The result is unknown; an operator must reconcile it.
    Uncertain,
}

/// One destination of the head in progress. Fields are in key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationProgress {
    pub delivered_at: Option<String>,
    pub kind: Kind,
    pub status: DestinationStatus,
}

/// Why a head blocks its check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// Three attempts failed; `--rearm` the sequence after fixing the cause.
    Exhausted,
    /// A send may have taken effect; `--reconcile` each uncertain destination.
    Uncertain,
    /// The oldest entry is not a valid entry; repair the journal.
    InvalidEntry,
    /// An entry reuses an acknowledged sequence; repair the journal.
    SequenceReused,
}

impl BlockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exhausted => "exhausted",
            Self::Uncertain => "uncertain",
            Self::InvalidEntry => "invalid_entry",
            Self::SequenceReused => "sequence_reused",
        }
    }
}

/// The head in progress. Fields are in key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadProgress {
    pub attempts: u32,
    pub blocked: Option<BlockReason>,
    pub destinations: BTreeMap<Destination, DestinationProgress>,
    pub digest: String,
    pub last_attempt_at: Option<String>,
    pub sequence: Sequence,
    pub state: Option<AlertState>,
}

/// The last acknowledged head. Fields are in key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledged {
    pub digest: String,
    pub sequence: Sequence,
    pub state: AlertState,
}

/// `<check>.delivery.json`: the dispatcher's private progress. Fields are in
/// key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryState {
    pub acknowledged: Option<Acknowledged>,
    pub check: Check,
    pub head: Option<HeadProgress>,
    pub schema_version: u32,
}

impl DeliveryState {
    fn empty(check: Check) -> Self {
        Self {
            acknowledged: None,
            check,
            head: None,
            schema_version: DELIVERY_SCHEMA_VERSION,
        }
    }
}

/// Python's `_receipt_time`.
fn receipt_time(
    receipts: Option<&BTreeMap<AlertState, Receipt>>,
    state: AlertState,
) -> Option<DateTime<Utc>> {
    receipts
        .and_then(|receipts| receipts.get(&state))
        .and_then(|receipt| parse_legacy_time(&receipt.delivered_at))
}

/// Python's `_delivery_kind`.
fn delivery_kind(receipts: Option<&BTreeMap<AlertState, Receipt>>, state: AlertState) -> Kind {
    let failure_at = receipt_time(receipts, AlertState::Failure);
    let healthy_at = receipt_time(receipts, AlertState::Healthy);
    match state {
        AlertState::Healthy if failure_at.is_some() => Kind::Recovery,
        AlertState::Healthy => Kind::Healthy,
        AlertState::Failure => match (failure_at, healthy_at) {
            (Some(_), None) => Kind::Reminder,
            (Some(failure), Some(healthy)) if failure > healthy => Kind::Reminder,
            _ => Kind::Failure,
        },
    }
}

/// Python's `_receipt_due`: a healthy state is sent only as a recovery from a
/// confirmed failure; a failure is sent when it is new, or again once an hour.
fn receipt_due(
    receipts: Option<&BTreeMap<AlertState, Receipt>>,
    state: AlertState,
    now: DateTime<Utc>,
) -> bool {
    let failure_at = receipt_time(receipts, AlertState::Failure);
    let healthy_at = receipt_time(receipts, AlertState::Healthy);
    match state {
        AlertState::Healthy => {
            failure_at.is_some_and(|failure| healthy_at.is_none_or(|healthy| failure > healthy))
        }
        AlertState::Failure => match failure_at {
            None => true,
            Some(failure) if healthy_at.is_some_and(|healthy| healthy >= failure) => true,
            Some(failure) => (now - failure).num_seconds() >= REMINDER_INTERVAL_SECONDS,
        },
    }
}

/// An attempt time in milliseconds, rounded up, so that waiting from the
/// recorded time never makes two attempts closer than the spacing.
fn attempt_time(at: DateTime<Utc>) -> String {
    (at + chrono::Duration::microseconds(999)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---- configuration and outcomes ------------------------------------------------------

/// Paths and bounds of the dispatcher.
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    pub alert_directory: PathBuf,
    pub health: JournalPaths,
    pub backup: JournalPaths,
    pub lock_timeout: Duration,
    pub max_attempts: u32,
    pub attempt_spacing: Duration,
}

impl DispatcherConfig {
    /// A layout with the given directories and the installed bounds.
    pub fn new(
        alert_directory: impl Into<PathBuf>,
        health_state_directory: impl Into<PathBuf>,
        backup_state_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            alert_directory: alert_directory.into(),
            health: JournalPaths::new(Check::Health, health_state_directory),
            backup: JournalPaths::new(Check::Backup, backup_state_directory),
            lock_timeout: LOCK_TIMEOUT,
            max_attempts: MAX_ATTEMPTS,
            attempt_spacing: ATTEMPT_SPACING,
        }
    }

    /// The installed layout.
    pub fn installed() -> Self {
        Self::new(
            INSTALLED_ALERT_DIRECTORY,
            Check::Health.installed_state_directory(),
            Check::Backup.installed_state_directory(),
        )
    }

    pub fn journal(&self, check: Check) -> &JournalPaths {
        match check {
            Check::Health => &self.health,
            Check::Backup => &self.backup,
        }
    }

    /// The stable delivery lock, the same path Python used.
    pub fn delivery_lock(&self, check: Check) -> PathBuf {
        self.alert_directory
            .join(format!(".{}.lock", check.as_str()))
    }

    /// The Python-compatible state document.
    pub fn legacy_state(&self, check: Check) -> PathBuf {
        self.alert_directory
            .join(format!("{}.json", check.as_str()))
    }

    /// The dispatcher's private progress document.
    pub fn delivery_state(&self, check: Check) -> PathBuf {
        self.alert_directory
            .join(format!("{}.delivery.json", check.as_str()))
    }
}

/// What one check's invocation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was due: an empty journal, or no reminder due.
    Idle,
    /// The head was acknowledged and removed (sent where due).
    Delivered(Sequence),
    /// No destination is configured; the head was acknowledged without a send.
    Unconfigured(Option<Sequence>),
    /// The attempt failed; the head stays and a later wakeup retries it.
    RetryLater(Sequence),
    /// The check is blocked until an operator acts.
    Blocked(Sequence, BlockReason),
    /// An hourly failure reminder was sent.
    Reminded,
    /// A due reminder could not be sent.
    ReminderFailed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Delivered(_) => "delivered",
            Self::Unconfigured(_) => "delivery_unconfigured",
            Self::RetryLater(_) => "retry_later",
            Self::Blocked(_, reason) => reason.as_str(),
            Self::Reminded => "reminded",
            Self::ReminderFailed => "reminder_failed",
        }
    }
}

/// The exit code of one check's result: 0 done, 2 no destination configured
/// (as Python), 3 blocked until an operator repairs or re-arms, 4 retry later
/// (including a lock timeout), 5 an uncertain send to reconcile.
pub fn exit_for(result: &Result<Outcome, Exit>) -> Exit {
    match result {
        Ok(Outcome::Idle | Outcome::Delivered(_) | Outcome::Reminded) => Exit::Ok,
        Ok(Outcome::Unconfigured(_)) => Exit::Usage,
        Ok(Outcome::RetryLater(_) | Outcome::ReminderFailed) => Exit::Dependency,
        Ok(Outcome::Blocked(_, BlockReason::Uncertain)) => Exit::Ambiguous,
        Ok(Outcome::Blocked(_, _)) => Exit::Precondition,
        Err(exit) => *exit,
    }
}

/// An operator's finding for one uncertain destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The destination did receive it: record it as delivered.
    Delivered,
    /// It did not: send it again on the next attempt.
    NotDelivered,
}

// ---- the dispatcher -------------------------------------------------------------------

/// The delivery side of the journal.
pub struct Dispatcher<'a, T: Transport, C: Clock> {
    config: &'a DispatcherConfig,
    settings: &'a AlertSettings,
    transport: &'a T,
    clock: &'a C,
    hook: Option<&'a dyn DispatchHook>,
}

impl<'a, T: Transport, C: Clock> Dispatcher<'a, T, C> {
    pub fn new(
        config: &'a DispatcherConfig,
        settings: &'a AlertSettings,
        transport: &'a T,
        clock: &'a C,
    ) -> Self {
        Self {
            config,
            settings,
            transport,
            clock,
            hook: None,
        }
    }

    pub fn with_hook(mut self, hook: &'a dyn DispatchHook) -> Self {
        self.hook = Some(hook);
        self
    }

    fn reached(&self, stage: DispatchStage) {
        if let Some(hook) = self.hook {
            hook.reached(stage);
        }
    }

    fn reader(&self, check: Check) -> JournalReader {
        JournalReader::new(self.config.journal(check).clone(), self.config.lock_timeout)
    }

    /// `--event <check>`: deliver the oldest unacknowledged entry, if any.
    pub async fn wake(&self, check: Check) -> Result<Outcome, Exit> {
        let lock = self.delivery_lock(check)?;
        let outcome = self.drain_head(check).await?;
        drop(lock);
        Ok(outcome.unwrap_or(Outcome::Idle))
    }

    /// `--remind <check>`: drain a pending head first; with an empty journal,
    /// repeat a lasting failure once an hour. A healthy state is never
    /// reminded, and a reminder never publishes a new state.
    pub async fn remind(&self, check: Check) -> Result<Outcome, Exit> {
        let lock = self.delivery_lock(check)?;
        let outcome = match self.drain_head(check).await? {
            Some(outcome) => outcome,
            None => self.remind_failure(check).await?,
        };
        drop(lock);
        Ok(outcome)
    }

    /// `--reconcile`: record an operator's finding for one uncertain
    /// destination of the head in progress.
    pub fn reconcile(
        &self,
        check: Check,
        sequence: Sequence,
        destination: Destination,
        resolution: Resolution,
    ) -> Result<(), Exit> {
        let _lock = self.delivery_lock(check)?;
        let mut delivery = self.load_delivery(check)?;
        let now = utc_text(self.clock.now());
        let head = delivery
            .head
            .as_mut()
            .filter(|head| head.sequence == sequence)
            .ok_or(Exit::Precondition)?;
        let progress = head
            .destinations
            .get_mut(&destination)
            .filter(|progress| {
                matches!(
                    progress.status,
                    DestinationStatus::Uncertain | DestinationStatus::InFlight
                )
            })
            .ok_or(Exit::Precondition)?;
        match resolution {
            Resolution::Delivered => {
                progress.status = DestinationStatus::Delivered;
                progress.delivered_at = Some(now);
            }
            Resolution::NotDelivered => progress.status = DestinationStatus::Pending,
        }
        let unresolved = head.destinations.values().any(|progress| {
            matches!(
                progress.status,
                DestinationStatus::Uncertain | DestinationStatus::InFlight
            )
        });
        if !unresolved && head.blocked == Some(BlockReason::Uncertain) {
            head.blocked = None;
        }
        self.save_delivery(check, &delivery)
    }

    /// `--rearm`: give an exhausted head a fresh set of attempts.
    pub fn rearm(&self, check: Check, sequence: Sequence) -> Result<(), Exit> {
        let _lock = self.delivery_lock(check)?;
        let mut delivery = self.load_delivery(check)?;
        let head = delivery
            .head
            .as_mut()
            .filter(|head| {
                head.sequence == sequence && head.blocked == Some(BlockReason::Exhausted)
            })
            .ok_or(Exit::Precondition)?;
        head.attempts = 0;
        head.blocked = None;
        self.save_delivery(check, &delivery)
    }

    /// `--release`: hand the check back to the Python dispatcher. Refused
    /// unless the journal is empty, no head is in progress and the state
    /// document is valid for Python.
    pub fn release_to_python(&self, check: Check) -> Result<(), Exit> {
        let lock = self.delivery_lock(check)?;
        let delivery = self.load_delivery(check)?;
        if delivery.head.is_some() {
            return Err(Exit::Precondition);
        }
        if self.reader(check).head()? != Head::Empty {
            return Err(Exit::Precondition);
        }
        if self.load_legacy(check)? == LegacyLoad::Malformed {
            return Err(Exit::Precondition);
        }
        set_lock_mode(&lock.file, PYTHON_LOCK_MODE)
    }

    // ---- locks ----

    /// Open (creating fenced) and take the stable delivery lock, then fence it.
    fn delivery_lock(&self, check: Check) -> Result<DeliveryLock, Exit> {
        create_private_directory(&self.config.alert_directory)?;
        let path = self.config.delivery_lock(check);
        let file = open_delivery_lock(&path)?;
        let lock_file = file.try_clone().map_err(|_| Exit::Dependency)?;
        // A timeout and an unusable lock are both "try again later" (exit 4).
        let held = lock_with_timeout(lock_file, self.config.lock_timeout)
            .map_err(|_: LockFailure| Exit::Dependency)?;
        set_lock_mode(&file, FENCED_LOCK_MODE)?;
        Ok(DeliveryLock { file, _held: held })
    }

    // ---- head delivery ----

    /// Deliver or advance the oldest entry. `None` when the journal is empty.
    async fn drain_head(&self, check: Check) -> Result<Option<Outcome>, Exit> {
        let reader = self.reader(check);
        let mut delivery = self.load_delivery(check)?;
        if let Some(head) = delivery.head.as_mut() {
            let sequence = head.sequence;
            if let Some(reason) = head.blocked {
                return Ok(Some(Outcome::Blocked(sequence, reason)));
            }
            let mut crashed_send = false;
            for progress in head.destinations.values_mut() {
                if progress.status == DestinationStatus::InFlight {
                    progress.status = DestinationStatus::Uncertain;
                    crashed_send = true;
                }
            }
            let digest = head.digest.clone();
            if crashed_send {
                return self
                    .block(check, &mut delivery, sequence, BlockReason::Uncertain)
                    .map(Some);
            }
            let present = matches!(
                reader.head()?,
                Head::Entry { sequence: found, digest: ref found_digest, .. }
                    if found == sequence && *found_digest == digest
            );
            if !present {
                return self
                    .block(check, &mut delivery, sequence, BlockReason::InvalidEntry)
                    .map(Some);
            }
            return self.attempt(check, &mut delivery).await.map(Some);
        }
        // An acknowledged entry left behind by a crash is removed first, then
        // the next entry becomes the head.
        for _ in 0..2 {
            match reader.head()? {
                Head::Empty => return Ok(None),
                Head::Invalid(sequence) => {
                    return self
                        .block(check, &mut delivery, sequence, BlockReason::InvalidEntry)
                        .map(Some);
                }
                Head::Entry {
                    sequence,
                    entry,
                    digest,
                } => {
                    if let Some(acknowledged) = &delivery.acknowledged
                        && sequence <= acknowledged.sequence
                    {
                        if sequence == acknowledged.sequence && digest == acknowledged.digest {
                            reader.remove(sequence, &digest)?;
                            continue;
                        }
                        return self
                            .block(check, &mut delivery, sequence, BlockReason::SequenceReused)
                            .map(Some);
                    }
                    let legacy = self.current_legacy(check)?;
                    let now = self.clock.now();
                    let configured = self.settings.configured(check);
                    let destinations = configured
                        .iter()
                        .map(|destination| {
                            let receipts = legacy
                                .as_ref()
                                .and_then(|legacy| legacy.receipts.get(destination));
                            let status = if receipt_due(receipts, entry.state, now) {
                                DestinationStatus::Pending
                            } else {
                                DestinationStatus::Skipped
                            };
                            (
                                *destination,
                                DestinationProgress {
                                    delivered_at: None,
                                    kind: delivery_kind(receipts, entry.state),
                                    status,
                                },
                            )
                        })
                        .collect();
                    delivery.head = Some(HeadProgress {
                        attempts: 0,
                        blocked: None,
                        destinations,
                        digest,
                        last_attempt_at: None,
                        sequence,
                        state: Some(entry.state),
                    });
                    self.save_delivery(check, &delivery)?;
                    if configured.is_empty() {
                        self.finalize(check, &mut delivery)?;
                        return Ok(Some(Outcome::Unconfigured(Some(sequence))));
                    }
                    return self.attempt(check, &mut delivery).await.map(Some);
                }
            }
        }
        Ok(Some(Outcome::Idle))
    }

    fn block(
        &self,
        check: Check,
        delivery: &mut DeliveryState,
        sequence: Sequence,
        reason: BlockReason,
    ) -> Result<Outcome, Exit> {
        match delivery.head.as_mut() {
            Some(head) => head.blocked = Some(reason),
            None => {
                delivery.head = Some(HeadProgress {
                    attempts: 0,
                    blocked: Some(reason),
                    destinations: BTreeMap::new(),
                    digest: String::new(),
                    last_attempt_at: None,
                    sequence,
                    state: None,
                });
            }
        }
        self.save_delivery(check, delivery)?;
        Ok(Outcome::Blocked(sequence, reason))
    }

    /// One attempt of the head in progress: every pending destination once.
    async fn attempt(&self, check: Check, delivery: &mut DeliveryState) -> Result<Outcome, Exit> {
        let (sequence, state, pending, attempts, last_attempt_at) = {
            let head = delivery.head.as_ref().ok_or(Exit::Precondition)?;
            let pending: Vec<Destination> = head
                .destinations
                .iter()
                .filter(|(_, progress)| progress.status == DestinationStatus::Pending)
                .map(|(destination, _)| *destination)
                .collect();
            (
                head.sequence,
                head.state.ok_or(Exit::Precondition)?,
                pending,
                head.attempts,
                head.last_attempt_at.clone(),
            )
        };
        if pending.is_empty() {
            self.finalize(check, delivery)?;
            return Ok(Outcome::Delivered(sequence));
        }
        if attempts >= self.config.max_attempts {
            return self.block(check, delivery, sequence, BlockReason::Exhausted);
        }
        if let Some(last) = last_attempt_at.as_deref().and_then(parse_bounded_timestamp) {
            let spacing = chrono::Duration::from_std(self.config.attempt_spacing)
                .unwrap_or(chrono::Duration::MAX);
            if let Ok(wait) = (last + spacing - self.clock.now()).to_std()
                && !wait.is_zero()
            {
                self.clock
                    .sleep(wait.min(self.config.attempt_spacing))
                    .await;
            }
        }
        {
            let head = delivery.head.as_mut().ok_or(Exit::Precondition)?;
            head.attempts += 1;
            head.last_attempt_at = Some(attempt_time(self.clock.now()));
        }
        self.save_delivery(check, delivery)?;

        for destination in pending {
            let kind = self.set_status(delivery, destination, DestinationStatus::InFlight, None)?;
            self.save_delivery(check, delivery)?;
            self.reached(DispatchStage::InFlight(destination));
            let outcome = self
                .transport
                .send(
                    self.settings,
                    &Delivery {
                        check,
                        state,
                        kind,
                        destination,
                        sequence: Some(sequence),
                    },
                )
                .await;
            match outcome {
                SendOutcome::Delivered => {
                    let now = utc_text(self.clock.now());
                    self.set_status(
                        delivery,
                        destination,
                        DestinationStatus::Delivered,
                        Some(now),
                    )?
                }
                SendOutcome::NotDelivered => {
                    self.set_status(delivery, destination, DestinationStatus::Pending, None)?
                }
                SendOutcome::Uncertain => {
                    self.set_status(delivery, destination, DestinationStatus::Uncertain, None)?
                }
            };
            self.save_delivery(check, delivery)?;
        }

        let (uncertain, pending, attempts) = {
            let head = delivery.head.as_ref().ok_or(Exit::Precondition)?;
            let has = |status| {
                head.destinations
                    .values()
                    .any(|progress| progress.status == status)
            };
            (
                has(DestinationStatus::Uncertain),
                has(DestinationStatus::Pending),
                head.attempts,
            )
        };
        if uncertain {
            return self.block(check, delivery, sequence, BlockReason::Uncertain);
        }
        if pending {
            if attempts >= self.config.max_attempts {
                return self.block(check, delivery, sequence, BlockReason::Exhausted);
            }
            return Ok(Outcome::RetryLater(sequence));
        }
        self.finalize(check, delivery)?;
        Ok(Outcome::Delivered(sequence))
    }

    /// Set one destination's status (and confirmation time) and return its kind.
    fn set_status(
        &self,
        delivery: &mut DeliveryState,
        destination: Destination,
        status: DestinationStatus,
        delivered_at: Option<String>,
    ) -> Result<Kind, Exit> {
        let progress = delivery
            .head
            .as_mut()
            .and_then(|head| head.destinations.get_mut(&destination))
            .ok_or(Exit::Precondition)?;
        progress.status = status;
        if delivered_at.is_some() {
            progress.delivered_at = delivered_at;
        }
        Ok(progress.kind)
    }

    /// Publish the state with the new receipts, acknowledge the head, then
    /// remove its entry.
    fn finalize(&self, check: Check, delivery: &mut DeliveryState) -> Result<(), Exit> {
        let head = delivery.head.clone().ok_or(Exit::Precondition)?;
        let state = head.state.ok_or(Exit::Precondition)?;
        let mut receipts = self
            .current_legacy(check)?
            .map(|legacy| legacy.receipts)
            .unwrap_or_default();
        for (destination, progress) in &head.destinations {
            if progress.status == DestinationStatus::Delivered {
                let delivered_at = progress.delivered_at.clone().ok_or(Exit::Precondition)?;
                receipts.entry(*destination).or_default().insert(
                    state,
                    Receipt {
                        delivered_at,
                        receipt_id: uuid::Uuid::new_v4().simple().to_string(),
                    },
                );
            }
        }
        let legacy = LegacyState {
            observed_at: utc_text(self.clock.now()),
            receipts,
            schema_version: LEGACY_SCHEMA_VERSION,
            state,
        };
        write_atomic(&self.config.legacy_state(check), &legacy)?;
        self.reached(DispatchStage::LegacyPublished);
        delivery.acknowledged = Some(Acknowledged {
            digest: head.digest.clone(),
            sequence: head.sequence,
            state,
        });
        delivery.head = None;
        self.save_delivery(check, delivery)?;
        self.reached(DispatchStage::Acknowledged);
        self.reader(check).remove(head.sequence, &head.digest)
    }

    // ---- reminders ----

    async fn remind_failure(&self, check: Check) -> Result<Outcome, Exit> {
        let now = self.clock.now();
        let mut legacy = match self.load_legacy(check)? {
            LegacyLoad::Absent => return Ok(Outcome::Idle),
            LegacyLoad::Valid(legacy) if legacy.state != AlertState::Failure => {
                return Ok(Outcome::Idle);
            }
            LegacyLoad::Valid(legacy) => legacy,
            // Python treats an unreadable state as a failure to report.
            LegacyLoad::Malformed => {
                quarantine_file(&self.config.legacy_state(check))?;
                LegacyState {
                    observed_at: utc_text(now),
                    receipts: BTreeMap::new(),
                    schema_version: LEGACY_SCHEMA_VERSION,
                    state: AlertState::Failure,
                }
            }
        };
        let configured = self.settings.configured(check);
        if configured.is_empty() {
            legacy.observed_at = utc_text(now);
            write_atomic(&self.config.legacy_state(check), &legacy)?;
            return Ok(Outcome::Unconfigured(None));
        }
        let mut sent = false;
        let mut failed = false;
        for destination in configured {
            let receipts = legacy.receipts.get(&destination);
            if !receipt_due(receipts, AlertState::Failure, now) {
                continue;
            }
            let delivery = Delivery {
                check,
                state: AlertState::Failure,
                kind: delivery_kind(receipts, AlertState::Failure),
                destination,
                sequence: None,
            };
            // A reminder repeats a failure that is already delivered, so a
            // duplicate is harmless: an unknown result is not reconciled.
            match self.transport.send(self.settings, &delivery).await {
                SendOutcome::Delivered => {
                    legacy.receipts.entry(destination).or_default().insert(
                        AlertState::Failure,
                        Receipt {
                            delivered_at: utc_text(self.clock.now()),
                            receipt_id: uuid::Uuid::new_v4().simple().to_string(),
                        },
                    );
                    sent = true;
                }
                SendOutcome::NotDelivered | SendOutcome::Uncertain => failed = true,
            }
        }
        if !sent && !failed {
            return Ok(Outcome::Idle);
        }
        legacy.observed_at = utc_text(self.clock.now());
        write_atomic(&self.config.legacy_state(check), &legacy)?;
        Ok(if failed {
            Outcome::ReminderFailed
        } else {
            Outcome::Reminded
        })
    }

    // ---- documents ----

    fn load_delivery(&self, check: Check) -> Result<DeliveryState, Exit> {
        let path = self.config.delivery_state(check);
        if !path_exists(&path)? {
            return Ok(DeliveryState::empty(check));
        }
        match read_bounded::<DeliveryState>(&path, MAX_DELIVERY_BYTES) {
            Ok(state)
                if state.schema_version == DELIVERY_SCHEMA_VERSION && state.check == check =>
            {
                Ok(state)
            }
            Ok(_) | Err(Exit::Precondition) => {
                // A damaged progress file loses only resend protection: the
                // head is attempted again, never skipped.
                quarantine_file(&path)?;
                Ok(DeliveryState::empty(check))
            }
            Err(exit) => Err(exit),
        }
    }

    fn save_delivery(&self, check: Check, delivery: &DeliveryState) -> Result<(), Exit> {
        write_atomic(&self.config.delivery_state(check), delivery)
    }

    fn load_legacy(&self, check: Check) -> Result<LegacyLoad, Exit> {
        let path = self.config.legacy_state(check);
        if !path_exists(&path)? {
            return Ok(LegacyLoad::Absent);
        }
        let file = match crate::cli::state::open_regular(&path) {
            Ok(file) => file,
            Err(Exit::Precondition) => return Ok(LegacyLoad::Malformed),
            Err(exit) => return Err(exit),
        };
        let mut bytes = Vec::new();
        use std::io::Read;
        file.take(MAX_LEGACY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Exit::Dependency)?;
        if bytes.len() as u64 > MAX_LEGACY_BYTES {
            return Ok(LegacyLoad::Malformed);
        }
        Ok(parse_strict_json(&bytes)
            .and_then(|value| valid_legacy_state(&value, self.clock.now()))
            .map_or(LegacyLoad::Malformed, LegacyLoad::Valid))
    }

    /// The state document for receipt decisions. A malformed document is
    /// quarantined, as Python did, and counts as absent.
    fn current_legacy(&self, check: Check) -> Result<Option<LegacyState>, Exit> {
        match self.load_legacy(check)? {
            LegacyLoad::Valid(legacy) => Ok(Some(legacy)),
            LegacyLoad::Absent => Ok(None),
            LegacyLoad::Malformed => {
                quarantine_file(&self.config.legacy_state(check))?;
                Ok(None)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LegacyLoad {
    Absent,
    Valid(LegacyState),
    Malformed,
}

/// The held stable delivery lock and its descriptor (for the fence mode).
struct DeliveryLock {
    file: File,
    _held: HeldLock,
}

fn path_exists(path: &Path) -> Result<bool, Exit> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Exit::Dependency),
    }
}

fn create_private_directory(path: &Path) -> Result<(), Exit> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(Exit::Precondition),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|_| Exit::Dependency),
        Err(_) => Err(Exit::Dependency),
    }
}

/// Open the stable delivery lock read-only, creating it fenced (0400) on first
/// use. The path is never renamed or deleted. A link, a non-regular file, a
/// second link or a file owned by someone other than the directory's owner is
/// refused.
fn open_delivery_lock(path: &Path) -> Result<File, Exit> {
    use rustix::fs::{Mode, OFlags};
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(FENCED_LOCK_MODE as _),
    )
    .map_err(|_| Exit::Dependency)?;
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|_| Exit::Dependency)?;
    let owner = std::fs::metadata(path.parent().unwrap_or(Path::new(".")))
        .map_err(|_| Exit::Dependency)?
        .uid();
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != owner {
        return Err(Exit::Precondition);
    }
    Ok(file)
}

fn set_lock_mode(file: &File, mode: u32) -> Result<(), Exit> {
    use rustix::fs::Mode;
    let current = file.metadata().map_err(|_| Exit::Dependency)?.mode() & 0o777;
    if current == mode {
        return Ok(());
    }
    rustix::fs::fchmod(file, Mode::from_raw_mode(mode as _)).map_err(|_| Exit::Dependency)
}

/// Move a damaged private document aside as `<name>.invalid-<nanoseconds>`.
fn quarantine_file(path: &Path) -> Result<(), Exit> {
    crate::cli::monitors::status::quarantine(path)
        .map(|_| ())
        .map_err(|_| Exit::Dependency)
}

// ---- command ---------------------------------------------------------------------------

/// What `monitor alert` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Wake(Check),
    Remind(Vec<Check>),
    Reconcile(Check, Sequence, Destination, Resolution),
    Rearm(Check, Sequence),
    Release(Check),
}

/// Parse `--event <check>` (or the old `<check>-<state>` form).
pub fn parse_event(text: &str) -> Option<Check> {
    if let Some(check) = Check::parse(text) {
        return Some(check);
    }
    let (check, state) = text.rsplit_once('-')?;
    matches!(state, "failure" | "healthy")
        .then(|| Check::parse(check))
        .flatten()
}

fn parse_sequence(text: &str) -> Option<Sequence> {
    crate::cli::monitors::alert_journal::entry_sequence(&format!("{text}.json"))
}

/// The action the arguments name, or `None` for a usage refusal. Exactly one
/// action flag is allowed, and no flag of another monitor.
pub fn parse_action(args: &MonitorArgs) -> Option<Action> {
    if args.environment.is_some()
        || args.release_sha.is_some()
        || args.telegram_disabled
        || args.state_file.is_some()
        || args.dry_run
        || args.bootstrap
        || args.show_emails
        || args.dsn.is_some()
        || args.max_pending.is_some()
        || args.max_age_seconds.is_some()
        || args.query_timeout.is_some()
    {
        return None;
    }
    let chosen = [
        args.event.is_some(),
        args.remind.is_some(),
        args.reconcile.is_some(),
        args.rearm.is_some(),
        args.release.is_some(),
    ];
    if chosen.iter().filter(|chosen| **chosen).count() != 1 {
        return None;
    }
    if let Some(event) = &args.event {
        return parse_event(event).map(Action::Wake);
    }
    if let Some(remind) = &args.remind {
        return match remind.as_str() {
            "all" => Some(Action::Remind(Check::ALL.to_vec())),
            other => Check::parse(other).map(|check| Action::Remind(vec![check])),
        };
    }
    if let Some(text) = &args.reconcile {
        let parts: Vec<&str> = text.split(':').collect();
        let [check, sequence, destination, resolution] = parts.as_slice() else {
            return None;
        };
        let resolution = match *resolution {
            "delivered" => Resolution::Delivered,
            "not-delivered" => Resolution::NotDelivered,
            _ => return None,
        };
        return Some(Action::Reconcile(
            Check::parse(check)?,
            parse_sequence(sequence)?,
            Destination::parse(destination)?,
            resolution,
        ));
    }
    if let Some(text) = &args.rearm {
        let (check, sequence) = text.split_once(':')?;
        return Some(Action::Rearm(
            Check::parse(check)?,
            parse_sequence(sequence)?,
        ));
    }
    args.release
        .as_deref()
        .and_then(Check::parse)
        .map(Action::Release)
}

/// Run one action with a dispatcher. Each check's work is bounded by
/// [`WORKER_BUDGET`]; a reminder for both checks runs them one after the other
/// and returns the highest exit code.
pub async fn run_action<T: Transport, C: Clock>(
    dispatcher: &Dispatcher<'_, T, C>,
    action: &Action,
) -> (Exit, Vec<(Check, String)>) {
    let mut results = Vec::new();
    match action {
        Action::Wake(check) => {
            let result = bounded(dispatcher.wake(*check)).await;
            results.push((*check, result));
        }
        Action::Remind(checks) => {
            for check in checks {
                let result = bounded(dispatcher.remind(*check)).await;
                results.push((*check, result));
            }
        }
        Action::Reconcile(check, sequence, destination, resolution) => {
            let result = dispatcher
                .reconcile(*check, *sequence, *destination, *resolution)
                .map(|()| Outcome::Idle);
            results.push((*check, result));
        }
        Action::Rearm(check, sequence) => {
            let result = dispatcher.rearm(*check, *sequence).map(|()| Outcome::Idle);
            results.push((*check, result));
        }
        Action::Release(check) => {
            let result = dispatcher.release_to_python(*check).map(|()| Outcome::Idle);
            results.push((*check, result));
        }
    }
    let exit = results
        .iter()
        .map(|(_, result)| exit_for(result))
        .max_by_key(|exit| *exit as i32)
        .unwrap_or(Exit::Ok);
    let named = results
        .into_iter()
        .map(|(check, result)| {
            let outcome = match result {
                Ok(outcome) => outcome.as_str().to_owned(),
                Err(exit) => format!("refused_exit_{}", exit as i32),
            };
            (check, outcome)
        })
        .collect();
    (exit, named)
}

/// Bound one check's work by [`WORKER_BUDGET`]; running out is exit 4.
async fn bounded(work: impl Future<Output = Result<Outcome, Exit>>) -> Result<Outcome, Exit> {
    tokio::time::timeout(WORKER_BUDGET, work)
        .await
        .unwrap_or(Err(Exit::Dependency))
}

/// `verdaxis-read monitor alert ...` with the installed layout, the alert
/// environment and the real provider client. One JSON line on stderr names
/// each check's outcome; no token, chat id or URL is printed.
pub async fn run_command(args: &MonitorArgs) -> Exit {
    let Some(action) = parse_action(args) else {
        report(&[], "usage_refused");
        return Exit::Usage;
    };
    let config = DispatcherConfig::installed();
    let settings = AlertSettings::from_process_environment();
    let transport = HttpTransport::new();
    let dispatcher = Dispatcher::new(&config, &settings, &transport, &SystemClock);
    let (exit, outcomes) = run_action(&dispatcher, &action).await;
    report(&outcomes, if exit == Exit::Ok { "ok" } else { "failed" });
    exit
}

fn report(outcomes: &[(Check, String)], outcome: &str) {
    let counts: serde_json::Map<String, serde_json::Value> = outcomes
        .iter()
        .map(|(check, outcome)| (check.as_str().to_owned(), outcome.clone().into()))
        .collect();
    eprintln!(
        "{}",
        serde_json::json!({"command": "monitor alert", "outcome": outcome, "counts": counts})
    );
}
