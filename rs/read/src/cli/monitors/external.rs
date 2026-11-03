//! `verdaxis-read monitor external`: the shared host's route and uptime
//! monitor (`/usr/local/sbin/verdaxis-monitor`, the installed copy of
//! `deploy/external_monitor/verdaxis_monitor.py`).
//!
//! One run, in the installed order:
//!
//! 1. the Caddyfile: present, large enough or importing others, valid for
//!    `caddy adapt --validate` as the `caddy` user, and naming every
//!    required host;
//! 2. each public endpoint, with bounded retries; a Vercel Security
//!    Checkpoint on an endpoint that allows it counts as reachable;
//! 3. the local monitor units (plan R4-3): a failed producer or alert unit
//!    is an error, read from unit state only;
//! 4. the expensive checks, at most once per 30 minutes (cached in the
//!    status file): frontend bundles, the rendered login page, the backup
//!    and restore evidence, the signup canaries, the demo trade canaries and
//!    the analytics volume.
//!
//! It writes the status file, then alerts on a new failure, a repeated
//! failure after the cooldown, and a recovery (Telegram and the Healthchecks
//! ping), and exits 0 when every check passed or 2 otherwise.
//!
//! Every external program runs from a fixed argument vector, never through a
//! shell. Response bodies, tokens and headers never reach the status file or
//! an alert: messages name the check and the class of failure only.
//!
//! Differences from the Python monitor (recorded in the Task 29 notes): no
//! `curl` or `gzip` subprocess (HTTP and the gzip test run in process), the
//! Umami ingestion canary is retired with the Umami smoke tool, a run-time
//! setting that does not parse refuses the run (exit 3) instead of raising,
//! and log lines go to stderr only (systemd journals them).
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::cli::Exit;

pub mod webdriver;

pub const REQUIRED_HOSTS: [&str; 7] = [
    "verdaxis.exchange",
    "www.verdaxis.exchange",
    "app.verdaxis.exchange",
    "api.verdaxis.exchange",
    "staging.verdaxis.exchange",
    "api-staging.verdaxis.exchange",
    "deck.verdaxis.exchange",
];

/// The units whose failure the monitor reports (plan R4-3). A refused
/// admission leaves the producer failed; the alert units fail when delivery
/// is refused. A unit not installed on this host is not reported.
pub const WATCHED_UNITS: [&str; 5] = [
    "verdaxis-health.service",
    "verdaxis-backup-verify.service",
    "verdaxis-monitor-alert@health.service",
    "verdaxis-monitor-alert@backup.service",
    "verdaxis-monitor-alert-reminder.service",
];

pub const DEFAULT_STATE_FILE: &str = "/var/lib/verdaxis-monitor/state.json";
pub const DEFAULT_STATUS_FILE: &str = "/var/lib/verdaxis-monitor/status.json";
const DEFAULT_CADDYFILE: &str = "/etc/caddy/Caddyfile";
const DEFAULT_MIN_CADDY_BYTES: u64 = 20_000;
const DEFAULT_ALERT_COOLDOWN_SECONDS: i64 = 3_600;
const DEFAULT_HTTP_RETRIES: u32 = 3;
const DEFAULT_HTTP_RETRY_DELAY_SECONDS: f64 = 2.0;
const DEFAULT_HTTP_TIMEOUT_SECONDS: u64 = 15;
/// `text_request` and `json_request` use Python's fixed 20 s.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Caddy validation, the gzip test and `systemctl show` each get 20 s.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_RENDER_TIMEOUT_SECONDS: u64 = 75;
const DEFAULT_RENDER_RETRIES: u32 = 3;
const DEFAULT_RENDER_RETRY_DELAY_SECONDS: f64 = 2.0;
/// Demo activity runs every 30 minutes; 15 more minutes allow for delay.
const DEMO_TRADE_MAX_AGE_SECONDS: f64 = 45.0 * 60.0;
const DEMO_TRADE_FUTURE_TOLERANCE_SECONDS: f64 = 5.0 * 60.0;
const DEFAULT_ANALYTICS_DATA_DIR: &str = "/home/verdaxis-prod/verdaxis/analytics/data";
const DEFAULT_ANALYTICS_MAX_DATA_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const DEFAULT_MIN_DISK_FREE_PERCENT: f64 = 10.0;
const DEFAULT_BACKUP_STATUS_FILE: &str = "/home/verdaxis-prod/backups/status.json";
const DEFAULT_BACKUP_MAX_AGE_SECONDS: f64 = 30.0 * 60.0 * 60.0;
const BACKUP_FUTURE_TOLERANCE_SECONDS: f64 = 300.0;
const RESTORE_STATUS_MAX_AGE_SECONDS: f64 = 8.0 * 24.0 * 60.0 * 60.0;
const RESTORE_STATUS_FUTURE_TOLERANCE_SECONDS: f64 = 5.0 * 60.0;
const RESTORE_STATUS_MAX_BYTES: u64 = 5_000;
const DEFAULT_EXPENSIVE_CHECK_INTERVAL_SECONDS: i64 = 30 * 60;
const MIN_EXPENSIVE_CHECK_INTERVAL_SECONDS: i64 = 300;
/// The state and status files are small; a larger one is treated as absent.
const MAX_STATE_BYTES: u64 = 1024 * 1024;
/// Largest response body read from an endpoint or a bundle.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Largest Caddyfile read for the size and import test.
const MAX_CADDYFILE_BYTES: u64 = 16 * 1024 * 1024;
const BACKUP_DATABASES: [&str; 3] = ["verdaxis", "verdaxis_staging", "umami"];
const USER_AGENT: &str = "VerdaxisMonitor/1.0 (+https://verdaxis.exchange)";
const TELEGRAM_BASE: &str = "https://api.telegram.org";

// ---- configuration ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub name: String,
    pub url: String,
    pub status: u16,
    pub contains: Option<String>,
    pub allow_vercel_challenge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCheck {
    pub name: String,
    pub url: String,
    pub must_contain: Vec<String>,
    pub must_not_contain: Vec<String>,
}

fn endpoint(name: &str, url: &str, contains: Option<&str>, vercel: bool) -> Endpoint {
    Endpoint {
        name: name.into(),
        url: url.into(),
        status: 200,
        contains: contains.map(Into::into),
        allow_vercel_challenge: vercel,
    }
}

fn page(name: &str, url: &str, must: &[&str], must_not: &[&str]) -> PageCheck {
    PageCheck {
        name: name.into(),
        url: url.into(),
        must_contain: must.iter().map(|text| (*text).into()).collect(),
        must_not_contain: must_not.iter().map(|text| (*text).into()).collect(),
    }
}

/// The installed endpoint list, in order.
pub fn default_endpoints() -> Vec<Endpoint> {
    vec![
        endpoint("verdaxis landing", "https://verdaxis.exchange/", None, true),
        endpoint(
            "verdaxis app",
            "https://app.verdaxis.exchange/app",
            None,
            true,
        ),
        endpoint(
            "verdaxis staging",
            "https://staging.verdaxis.exchange/",
            None,
            false,
        ),
        endpoint(
            "verdaxis api",
            "https://api.verdaxis.exchange/health",
            Some("\"status\":\"ok\""),
            false,
        ),
        endpoint(
            "verdaxis staging api",
            "https://api-staging.verdaxis.exchange/health",
            Some("\"status\":\"ok\""),
            false,
        ),
        endpoint(
            "verdaxis staging analytics collector",
            "https://api-staging.verdaxis.exchange/script.js",
            Some("api/send"),
            false,
        ),
        endpoint(
            "verdaxis deck",
            "https://deck.verdaxis.exchange/",
            None,
            true,
        ),
        endpoint(
            "verdaxis buyer deck",
            "https://deck.verdaxis.exchange/buyer",
            None,
            true,
        ),
        endpoint(
            "verdaxis supplier deck",
            "https://deck.verdaxis.exchange/supplier",
            None,
            true,
        ),
    ]
}

pub fn default_bundle_checks() -> Vec<PageCheck> {
    vec![
        page(
            "verdaxis app production bundle",
            "https://app.verdaxis.exchange/login?lang=en",
            &["https://api.verdaxis.exchange/api"],
            &[
                "http://localhost:8000/api",
                "https://api-staging.verdaxis.exchange/api",
            ],
        ),
        page(
            "verdaxis staging bundle",
            "https://staging.verdaxis.exchange/login?lang=en",
            &["https://api-staging.verdaxis.exchange/api"],
            &[
                "http://localhost:8000/api",
                "https://api.verdaxis.exchange/api",
            ],
        ),
    ]
}

pub fn default_rendered_pages() -> Vec<PageCheck> {
    vec![page(
        "verdaxis app login rendered page",
        "https://app.verdaxis.exchange/login?lang=en",
        &["Sign In"],
        &[
            "Verdaxis is temporarily under maintenance",
            "temporarily under maintenance",
        ],
    )]
}

/// Everything one run reads from its environment, plus the targets and
/// program paths, which tests replace.
#[derive(Debug, Clone)]
pub struct ExternalConfig {
    pub state_file: PathBuf,
    pub status_file: PathBuf,
    pub caddyfile: PathBuf,
    pub min_caddyfile_bytes: u64,
    /// The validation command; the Caddyfile path is appended after
    /// `adapt --validate --config`.
    pub caddy_command: Vec<String>,
    pub systemctl: PathBuf,
    pub watched_units: Vec<String>,
    pub alert_cooldown_seconds: i64,
    pub http_retries: u32,
    pub http_retry_delay: Duration,
    pub http_timeout_seconds: u64,
    pub endpoints: Vec<Endpoint>,
    pub bundle_checks_enabled: bool,
    pub bundle_checks: Vec<PageCheck>,
    pub rendered_checks_enabled: bool,
    pub rendered_pages: Vec<PageCheck>,
    pub render: webdriver::RenderSettings,
    pub signup_canary_enabled: bool,
    pub demo_trade_canary_enabled: bool,
    /// `name -> API base` for the signup and demo trade canaries, in order.
    pub canary_targets: Vec<(String, String)>,
    pub monitor_token: Option<String>,
    pub backup_status_file: PathBuf,
    pub backup_max_age_seconds: f64,
    pub analytics_data_dir: PathBuf,
    pub analytics_max_data_bytes: u64,
    pub min_disk_free_percent: f64,
    /// `None` when the setting does not parse: the cache is then not used.
    pub expensive_interval_seconds: Option<i64>,
    pub telegram_disabled: bool,
    pub telegram_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_base: String,
    pub healthchecks_ping_url: Option<String>,
}

/// A setting that does not parse. It names the key, never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub &'static str);

fn disabled(value: Option<&String>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn nonempty(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    vars.get(key).filter(|value| !value.is_empty()).cloned()
}

/// Python's `int(text)` for a setting: optional surrounding whitespace, an
/// optional sign, ASCII digits and single underscores between digits.
fn python_int(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    trimmed.replace('_', "").parse().ok()
}

fn python_float(text: &str) -> Option<f64> {
    let value: f64 = text.trim().parse().ok()?;
    value.is_finite().then_some(value)
}

fn int_setting(
    vars: &HashMap<String, String>,
    key: &'static str,
    default: i64,
) -> Result<i64, ConfigError> {
    match vars.get(key) {
        None => Ok(default),
        Some(text) => python_int(text).ok_or(ConfigError(key)),
    }
}

fn float_setting(
    vars: &HashMap<String, String>,
    key: &'static str,
    default: f64,
) -> Result<f64, ConfigError> {
    match vars.get(key) {
        None => Ok(default),
        Some(text) => python_float(text).ok_or(ConfigError(key)),
    }
}

fn path_setting(vars: &HashMap<String, String>, key: &str, default: &str) -> PathBuf {
    PathBuf::from(vars.get(key).map_or(default, String::as_str))
}

/// `SIGNUP_CANARY_TARGETS`: `name=url` items separated by commas; blank or
/// malformed items are skipped; a blank value selects production and staging.
pub fn parse_canary_targets(raw: Option<&String>) -> Vec<(String, String)> {
    let raw = raw.map_or("", String::as_str);
    if raw.trim().is_empty() {
        return vec![
            ("prod".into(), "https://api.verdaxis.exchange/api".into()),
            (
                "staging".into(),
                "https://api-staging.verdaxis.exchange/api".into(),
            ),
        ];
    }
    let mut targets: Vec<(String, String)> = Vec::new();
    for item in raw.split(',') {
        let Some((name, url)) = item.split_once('=') else {
            continue;
        };
        if item.trim().is_empty() {
            continue;
        }
        let name = name.trim().to_owned();
        let url = url.trim().trim_end_matches('/').to_owned();
        // A repeated name keeps its first place and takes the last URL.
        match targets.iter_mut().find(|(existing, _)| *existing == name) {
            Some(target) => target.1 = url,
            None => targets.push((name, url)),
        }
    }
    targets
}

impl ExternalConfig {
    pub fn from_environment(
        vars: &HashMap<String, String>,
        home: &Path,
        telegram_disabled_flag: bool,
    ) -> Result<Self, ConfigError> {
        let retries = int_setting(vars, "HTTP_RETRIES", DEFAULT_HTTP_RETRIES.into())?;
        let render_retries = int_setting(vars, "RENDER_RETRIES", DEFAULT_RENDER_RETRIES.into())?;
        let http_timeout = int_setting(
            vars,
            "HTTP_TIMEOUT_SECONDS",
            DEFAULT_HTTP_TIMEOUT_SECONDS as i64,
        )?;
        let render_timeout = int_setting(
            vars,
            "RENDER_TIMEOUT_SECONDS",
            DEFAULT_RENDER_TIMEOUT_SECONDS as i64,
        )?;
        let min_caddy = int_setting(vars, "MIN_CADDYFILE_BYTES", DEFAULT_MIN_CADDY_BYTES as i64)?;
        let max_analytics = int_setting(
            vars,
            "ANALYTICS_MAX_DATA_BYTES",
            DEFAULT_ANALYTICS_MAX_DATA_BYTES as i64,
        )?;
        let seconds = |value: f64| Duration::from_secs_f64(value.max(0.0));
        Ok(Self {
            state_file: path_setting(vars, "STATE_FILE", DEFAULT_STATE_FILE),
            status_file: path_setting(vars, "STATUS_FILE", DEFAULT_STATUS_FILE),
            caddyfile: path_setting(vars, "CADDYFILE", DEFAULT_CADDYFILE),
            min_caddyfile_bytes: u64::try_from(min_caddy).unwrap_or(0),
            caddy_command: ["/usr/sbin/runuser", "-u", "caddy", "--", "/usr/bin/caddy"]
                .map(String::from)
                .to_vec(),
            systemctl: PathBuf::from("/usr/bin/systemctl"),
            watched_units: WATCHED_UNITS.map(String::from).to_vec(),
            alert_cooldown_seconds: int_setting(
                vars,
                "ALERT_COOLDOWN_SECONDS",
                DEFAULT_ALERT_COOLDOWN_SECONDS,
            )?,
            http_retries: u32::try_from(retries.max(1)).unwrap_or(u32::MAX),
            http_retry_delay: seconds(float_setting(
                vars,
                "HTTP_RETRY_DELAY_SECONDS",
                DEFAULT_HTTP_RETRY_DELAY_SECONDS,
            )?),
            http_timeout_seconds: u64::try_from(http_timeout)
                .map_err(|_| ConfigError("HTTP_TIMEOUT_SECONDS"))?,
            endpoints: default_endpoints(),
            bundle_checks_enabled: !disabled(vars.get("FRONTEND_BUNDLE_CHECKS_ENABLED")),
            bundle_checks: default_bundle_checks(),
            rendered_checks_enabled: !disabled(vars.get("RENDERED_PAGE_CHECKS_ENABLED")),
            rendered_pages: default_rendered_pages(),
            render: webdriver::RenderSettings {
                chromedriver: nonempty(vars, "CHROMEDRIVER_BIN").map(PathBuf::from),
                profile_parent: vars.get("CHROMIUM_PROFILE_PARENT").map(PathBuf::from),
                render_user: nonempty(vars, "RENDER_USER"),
                home: home.to_path_buf(),
                lang: vars.get("LANG").cloned(),
                lc_all: vars.get("LC_ALL").cloned(),
                timeout_seconds: u64::try_from(render_timeout)
                    .map_err(|_| ConfigError("RENDER_TIMEOUT_SECONDS"))?,
                retries: u32::try_from(render_retries.max(1)).unwrap_or(u32::MAX),
                retry_delay: seconds(float_setting(
                    vars,
                    "RENDER_RETRY_DELAY_SECONDS",
                    DEFAULT_RENDER_RETRY_DELAY_SECONDS,
                )?),
            },
            signup_canary_enabled: !disabled(vars.get("SIGNUP_CANARY_ENABLED")),
            demo_trade_canary_enabled: !disabled(vars.get("DEMO_TRADE_CANARY_ENABLED")),
            canary_targets: parse_canary_targets(vars.get("SIGNUP_CANARY_TARGETS")),
            monitor_token: nonempty(vars, "MONITOR_TOKEN"),
            backup_status_file: path_setting(
                vars,
                "BACKUP_STATUS_FILE",
                DEFAULT_BACKUP_STATUS_FILE,
            ),
            backup_max_age_seconds: int_setting(
                vars,
                "BACKUP_MAX_AGE_SECONDS",
                DEFAULT_BACKUP_MAX_AGE_SECONDS as i64,
            )? as f64,
            analytics_data_dir: path_setting(
                vars,
                "ANALYTICS_DATA_DIR",
                DEFAULT_ANALYTICS_DATA_DIR,
            ),
            analytics_max_data_bytes: u64::try_from(max_analytics).unwrap_or(0),
            min_disk_free_percent: float_setting(
                vars,
                "MIN_DISK_FREE_PERCENT",
                DEFAULT_MIN_DISK_FREE_PERCENT,
            )?,
            expensive_interval_seconds: match vars.get("EXPENSIVE_CHECK_INTERVAL_SECONDS") {
                None => Some(DEFAULT_EXPENSIVE_CHECK_INTERVAL_SECONDS),
                Some(text) => python_int(text),
            }
            .map(|interval| interval.max(MIN_EXPENSIVE_CHECK_INTERVAL_SECONDS)),
            telegram_disabled: telegram_disabled_flag || truthy(vars.get("TELEGRAM_DISABLED")),
            telegram_token: nonempty(vars, "TELEGRAM_BOT_TOKEN"),
            telegram_chat_id: nonempty(vars, "TELEGRAM_CHAT_ID"),
            telegram_base: vars
                .get("TELEGRAM_API_BASE")
                .map_or(TELEGRAM_BASE, String::as_str)
                .trim_end_matches('/')
                .to_owned(),
            healthchecks_ping_url: nonempty(vars, "HEALTHCHECKS_PING_URL"),
        })
    }
}

// ---- small helpers -------------------------------------------------------------

fn log(message: &str) {
    eprintln!("{message}");
}

/// Python's `repr` of a short ASCII needle, as the messages quote it.
fn quoted(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// A JSON object with its keys sorted, recursively, as Python's
/// `json.dumps(..., sort_keys=True)` writes it.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), sorted(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// `load_state`: any unreadable, oversized or non-object file is `{}`.
pub fn load_state(path: &Path) -> Map<String, Value> {
    std::fs::File::open(path)
        .ok()
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(MAX_STATE_BYTES + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            (bytes.len() as u64 <= MAX_STATE_BYTES).then_some(bytes)
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

/// `save_state`: create the directory, then replace the file atomically with
/// two-space indented JSON, sorted keys and a final newline.
pub fn save_state(path: &Path, state: &Value) -> Result<(), Exit> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| Exit::Dependency)?;
    }
    let mut document = serde_json::to_vec_pretty(&sorted(state)).map_err(|_| Exit::Dependency)?;
    document.push(b'\n');
    crate::cli::state::write_atomic_bytes(path, &document)
}

fn utc_text(seconds: i64) -> String {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map_or_else(String::new, |at| {
            at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        })
}

/// Python 3.10's `datetime.fromisoformat`: `YYYY-MM-DD`, then any one
/// separator and `HH[:MM[:SS[.fff|.ffffff]]]`, then an optional
/// `+HH:MM[:SS[.ffffff]]` offset. `Z` is not accepted. Returns the instant
/// and whether an offset was present.
pub fn python_fromisoformat(text: &str) -> Option<(DateTime<FixedOffset>, bool)> {
    static PATTERN: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})(?:.(\d{2})(?::(\d{2})(?::(\d{2})(?:\.(\d{3}|\d{6}))?)?)?(?:([+-])(\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{6}))?)?)?)?$",
        )
        .expect("a valid pattern")
    });
    let captures = PATTERN.captures(text)?;
    let number = |index: usize| -> Option<u32> {
        captures
            .get(index)
            .map_or(Some(0), |found| found.as_str().parse().ok())
    };
    let date = chrono::NaiveDate::from_ymd_opt(
        captures[1].parse().ok()?,
        captures[2].parse().ok()?,
        captures[3].parse().ok()?,
    )?;
    let fraction = captures.get(7).map_or(Some(0), |found| {
        let digits = found.as_str();
        let value: u32 = digits.parse().ok()?;
        Some(if digits.len() == 3 {
            value * 1_000_000
        } else {
            value * 1_000
        })
    })?;
    let time = chrono::NaiveTime::from_hms_nano_opt(number(4)?, number(5)?, number(6)?, fraction)?;
    let naive = date.and_time(time);
    let has_offset = captures.get(8).is_some();
    let offset_seconds = if has_offset {
        let magnitude = i32::try_from(number(9)? * 3600 + number(10)? * 60 + number(11)?).ok()?;
        if magnitude >= 86_400 {
            return None;
        }
        if &captures[8] == "-" {
            -magnitude
        } else {
            magnitude
        }
    } else {
        0
    };
    let offset = FixedOffset::east_opt(offset_seconds)?;
    Some((offset.from_local_datetime(&naive).single()?, has_offset))
}

fn age_seconds(now: DateTime<Utc>, at: DateTime<FixedOffset>) -> f64 {
    (now - at.with_timezone(&Utc))
        .num_microseconds()
        .map_or(f64::MAX, |micros| micros as f64 / 1e6)
}

fn is_backup_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
}

/// Run one program from a fixed argument vector (never a shell), bounded in
/// time. Returns the exit status and the captured output, or `None` when it
/// could not start or ran out of time.
async fn run_command(program: &[String], timeout: Duration) -> Option<(bool, String, String)> {
    let (first, rest) = program.split_first()?;
    let mut command = tokio::process::Command::new(first);
    command
        .args(rest)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()?;
    Some((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

// ---- Caddy ---------------------------------------------------------------------

pub async fn check_caddyfile(config: &ExternalConfig) -> Vec<String> {
    let caddyfile = &config.caddyfile;
    let display = caddyfile.display();
    let Ok(metadata) = std::fs::metadata(caddyfile) else {
        return vec![format!("{display} does not exist")];
    };
    let mut errors = Vec::new();
    let contents = std::fs::File::open(caddyfile)
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(MAX_CADDYFILE_BYTES).read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let size = metadata.len();
    if !contents.contains("import ") && size < config.min_caddyfile_bytes {
        errors.push(format!(
            "{display} is only {size} bytes; expected at least {}",
            config.min_caddyfile_bytes
        ));
    }

    let mut command = config.caddy_command.clone();
    command.extend(["adapt", "--validate", "--config"].map(String::from));
    command.push(caddyfile.to_string_lossy().into_owned());
    let Some((success, stdout, stderr)) = run_command(&command, COMMAND_TIMEOUT).await else {
        errors.push("caddy validate failed: could not run within 20 s".into());
        return errors;
    };
    if !success {
        let details = if stderr.trim().is_empty() {
            &stdout
        } else {
            &stderr
        };
        let last = details.trim().lines().last().unwrap_or("unknown error");
        errors.push(format!("caddy validate failed: {last}"));
        return errors;
    }
    let adapted: Value = match serde_json::from_str(&stdout) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("caddy adapt returned invalid JSON: {error}"));
            return errors;
        }
    };
    let mut configured = std::collections::BTreeSet::new();
    if let Some(servers) = adapted["apps"]["http"]["servers"].as_object() {
        for server in servers.values() {
            for route in server["routes"].as_array().into_iter().flatten() {
                for matcher in route["match"].as_array().into_iter().flatten() {
                    for host in matcher["host"].as_array().into_iter().flatten() {
                        if let Some(host) = host.as_str() {
                            configured.insert(host.to_owned());
                        }
                    }
                }
            }
        }
    }
    let missing: Vec<&str> = REQUIRED_HOSTS
        .iter()
        .copied()
        .filter(|host| !configured.contains(*host))
        .collect();
    if !missing.is_empty() {
        errors.push(format!("missing Caddy host blocks: {}", missing.join(", ")));
    }
    errors
}

// ---- HTTP ------------------------------------------------------------------------

/// A response: the final status, its lower-cased headers and a bounded body.
pub struct Fetched {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

/// Why a request produced no response. The text is a class, never a body,
/// a header or a URL with credentials.
fn request_failure(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_redirect() {
        "too many redirects"
    } else if error.is_body() || error.is_decode() {
        "response body could not be read"
    } else {
        "request failed"
    }
}

async fn read_body(mut response: reqwest::Response) -> Result<Fetched, &'static str> {
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).trim().to_owned(),
            )
        })
        .collect();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| request_failure(&error))?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err("response body is too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Fetched {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// The monitor's client: redirects followed (as `curl --location` and
/// urllib did), no proxy from the environment, the monitor's user agent.
pub fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .no_proxy()
        .user_agent(USER_AGENT)
        .build()
        .ok()
}

async fn get(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> Result<Fetched, &'static str> {
    let response = client
        .get(url)
        .header(
            "accept",
            "text/html,application/javascript,text/javascript,*/*",
        )
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| request_failure(&error))?;
    read_body(response).await
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    payload: &Value,
    token: &str,
) -> Result<Fetched, &'static str> {
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-monitor-token", token)
        .body(payload.to_string())
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| request_failure(&error))?;
    read_body(response).await
}

pub fn is_vercel_security_challenge(
    status: u16,
    headers: &HashMap<String, String>,
    body: &str,
) -> bool {
    let header = |name: &str| {
        headers
            .get(name)
            .map_or(String::new(), |value| value.to_ascii_lowercase())
    };
    status == 403
        && header("server") == "vercel"
        && header("x-vercel-mitigated") == "challenge"
        && (body.contains("Vercel Security Checkpoint")
            || headers.contains_key("x-vercel-challenge-token"))
}

pub fn looks_like_vercel_security_checkpoint(body: &str) -> bool {
    let normalized = body.to_lowercase();
    [
        "vercel security checkpoint",
        "x-vercel-challenge-token",
        "we're verifying your browser",
        "we are verifying your browser",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

// ---- endpoints -----------------------------------------------------------------

pub async fn check_endpoint(
    client: &reqwest::Client,
    config: &ExternalConfig,
    endpoint: &Endpoint,
) -> (Option<String>, Value) {
    let mut status = json!({
        "name": endpoint.name,
        "url": endpoint.url,
        "expected_status": endpoint.status,
        "ok": false,
        "attempts": [],
    });
    let timeout = Duration::from_secs(config.http_timeout_seconds);
    let mut final_error: Option<String> = None;
    for attempt in 1..=config.http_retries {
        let mut attempt_status = json!({"attempt": attempt});
        match get(client, &endpoint.url, timeout).await {
            Err(reason) => {
                attempt_status["error"] = json!(reason);
                final_error = Some(format!("{} request failed: {reason}", endpoint.name));
            }
            Ok(fetched) => {
                attempt_status["http_status"] = json!(fetched.status);
                status["http_status"] = json!(fetched.status);
                if fetched.status != endpoint.status {
                    if endpoint.allow_vercel_challenge
                        && is_vercel_security_challenge(
                            fetched.status,
                            &fetched.headers,
                            &fetched.body,
                        )
                    {
                        attempt_status["ok"] = json!(true);
                        attempt_status["note"] = json!("vercel_security_challenge");
                        push(&mut status["attempts"], attempt_status);
                        status["ok"] = json!(true);
                        status["note"] = json!(
                            "Vercel edge is reachable but returned Security Checkpoint to the monitor client"
                        );
                        return (None, status);
                    }
                    let error = format!("HTTP {}, expected {}", fetched.status, endpoint.status);
                    final_error = Some(format!("{} returned {error}", endpoint.name));
                    attempt_status["error"] = json!(error);
                } else if let Some(expected) = endpoint
                    .contains
                    .as_deref()
                    .filter(|expected| !fetched.body.contains(expected))
                {
                    let error = format!("body did not contain {}", quoted(expected));
                    final_error = Some(format!("{} {error}", endpoint.name));
                    attempt_status["error"] = json!(error);
                } else {
                    attempt_status["ok"] = json!(true);
                    push(&mut status["attempts"], attempt_status);
                    status["ok"] = json!(true);
                    return (None, status);
                }
            }
        }
        push(&mut status["attempts"], attempt_status);
        if attempt < config.http_retries {
            log(&format!(
                "{} check attempt {attempt}/{} failed; retrying: {}",
                endpoint.name,
                config.http_retries,
                final_error.as_deref().unwrap_or("")
            ));
            tokio::time::sleep(config.http_retry_delay).await;
        }
    }
    let error = final_error.unwrap_or_else(|| "unknown endpoint failure".into());
    status["error"] = json!(error);
    (Some(error), status)
}

fn push(array: &mut Value, item: Value) {
    if let Value::Array(items) = array {
        items.push(item);
    }
}

pub async fn check_endpoints(
    client: &reqwest::Client,
    config: &ExternalConfig,
) -> (Vec<String>, Vec<Value>) {
    let mut errors = Vec::new();
    let mut statuses = Vec::new();
    for endpoint in &config.endpoints {
        let (error, status) = check_endpoint(client, config, endpoint).await;
        statuses.push(status);
        errors.extend(error);
    }
    (errors, statuses)
}

// ---- local monitor units (R4-3) -------------------------------------------------

/// A failed watched unit is an error. Only `systemctl show` is read: the
/// check never reads a producer's files or credentials and never starts,
/// resets or retries a unit.
pub async fn check_units(config: &ExternalConfig) -> Vec<String> {
    let mut errors = Vec::new();
    for unit in &config.watched_units {
        let command = vec![
            config.systemctl.to_string_lossy().into_owned(),
            "show".into(),
            "--property=LoadState,ActiveState,Result".into(),
            "--".into(),
            unit.clone(),
        ];
        let Some((true, stdout, _)) = run_command(&command, COMMAND_TIMEOUT).await else {
            errors.push(format!("{unit} state could not be read"));
            continue;
        };
        let properties: HashMap<&str, &str> = stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        if properties.get("LoadState").copied() == Some("not-found") {
            continue;
        }
        if properties.get("ActiveState").copied() == Some("failed") {
            let result = properties.get("Result").copied().unwrap_or("unknown");
            errors.push(format!("{unit} is failed (result {result})"));
        }
    }
    errors
}

// ---- backups -------------------------------------------------------------------

/// `gzip -t` in process: every member decodes to the end within 20 s.
fn gzip_is_valid(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let mut decoder = flate2::read::MultiGzDecoder::new(std::io::BufReader::new(file));
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        if Instant::now() > deadline {
            return false;
        }
        match decoder.read(&mut buffer) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

pub fn check_backup_status(config: &ExternalConfig, now: DateTime<Utc>) -> Vec<String> {
    let status_file = &config.backup_status_file;
    let bytes = match std::fs::read(status_file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return vec![format!(
                "backup status is missing: {}",
                status_file.display()
            )];
        }
        Err(_) => return vec!["backup status is unreadable: the file could not be read".into()],
    };
    let Ok(Value::Object(payload)) = serde_json::from_slice::<Value>(&bytes) else {
        return vec!["backup status is unreadable: not a JSON object".into()];
    };
    if payload.get("ok") != Some(&Value::Bool(true)) {
        return vec!["last database backup did not complete successfully".into()];
    }
    let databases: std::collections::BTreeSet<&str> = payload
        .get("databases")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let all_strings = payload
        .get("databases")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().all(Value::is_string));
    if !all_strings || databases != BACKUP_DATABASES.into_iter().collect() {
        return vec!["last database backup did not include prod, staging, and analytics".into()];
    }
    let completed = match payload.get("completed_at").and_then(Value::as_str) {
        None => {
            return vec![
                "backup status has an invalid completion timestamp: missing or not text".into(),
            ];
        }
        Some(text) => {
            match python_fromisoformat(text) {
                None => {
                    return vec![
                    "backup status has an invalid completion timestamp: invalid isoformat string"
                        .into(),
                ];
                }
                Some((_, false)) => {
                    return vec!["backup status has an invalid completion timestamp: timestamp has no timezone".into()];
                }
                Some((at, true)) => at,
            }
        }
    };
    let age = age_seconds(now, completed);
    if age < -BACKUP_FUTURE_TOLERANCE_SECONDS {
        return vec!["backup completion timestamp is in the future".into()];
    }
    if age > config.backup_max_age_seconds {
        return vec![format!(
            "database backup is stale ({} hours old)",
            (age / 3600.0).floor() as i64
        )];
    }
    let Some(backup_id) = payload
        .get("backup_id")
        .and_then(Value::as_str)
        .filter(|id| is_backup_id(id))
    else {
        return vec!["backup status has an invalid backup ID".into()];
    };
    let directory = status_file.parent().unwrap_or(Path::new("."));
    let mut errors = Vec::new();
    for prefix in ["verdaxis", "verdaxis-staging", "umami"] {
        let name = format!("{prefix}-{backup_id}.sql.gz");
        match std::fs::metadata(directory.join(&name)) {
            Ok(metadata) if metadata.len() == 0 => {
                errors.push(format!("backup artifact is empty: {name}"));
            }
            Ok(_) => {
                if !gzip_is_valid(&directory.join(&name)) {
                    errors.push(format!("backup artifact failed gzip validation: {name}"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                errors.push(format!("backup artifact is missing: {name}"));
            }
            Err(_) => errors.push(format!("backup status check crashed: {name} is unreadable")),
        }
    }
    errors
}

pub fn check_restore_status(config: &ExternalConfig, now: DateTime<Utc>) -> Vec<String> {
    let status_file = config
        .backup_status_file
        .with_file_name("restore-status.json");
    let one = |message: &str| vec![message.to_owned()];
    let raw = match std::fs::File::open(&status_file) {
        Ok(file) => {
            let mut bytes = Vec::new();
            if file
                .take(RESTORE_STATUS_MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
            {
                return one("restore verification status is unreadable");
            }
            bytes
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return one("restore verification status is missing");
        }
        Err(_) => return one("restore verification status is unreadable"),
    };
    if raw.len() as u64 > RESTORE_STATUS_MAX_BYTES {
        return one("restore verification status is unreadable");
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&raw) else {
        return one("restore verification status is unreadable");
    };
    let Some(object) = payload.as_object() else {
        return one("restore verification status has an invalid schema");
    };
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    if keys != ["backup_id", "completed_at", "databases", "ok"] {
        return one("restore verification status has an invalid schema");
    }
    let (Some(ok), Some(completed), Some(backup_id), Some(databases)) = (
        object["ok"].as_bool(),
        object["completed_at"].as_str(),
        object["backup_id"].as_str(),
        object["databases"].as_array(),
    ) else {
        return one("restore verification status has an invalid schema");
    };
    if !ok {
        return one("last restore verification did not complete successfully");
    }
    let names: std::collections::BTreeSet<&str> =
        databases.iter().filter_map(Value::as_str).collect();
    if databases.len() != BACKUP_DATABASES.len()
        || !databases.iter().all(Value::is_string)
        || names != BACKUP_DATABASES.into_iter().collect()
    {
        return one("restore verification status has an invalid database inventory");
    }
    if !is_backup_id(backup_id) {
        return one("restore verification status has an invalid backup ID");
    }
    let normalized = completed
        .strip_suffix('Z')
        .map_or_else(|| completed.to_owned(), |head| format!("{head}+00:00"));
    let Some((at, true)) = python_fromisoformat(&normalized) else {
        return one("restore verification status has an invalid completion timestamp");
    };
    let age = age_seconds(now, at);
    if age < -RESTORE_STATUS_FUTURE_TOLERANCE_SECONDS {
        return one("restore verification completion timestamp is in the future");
    }
    if age > RESTORE_STATUS_MAX_AGE_SECONDS {
        return one("restore verification is stale");
    }
    Vec::new()
}

// ---- analytics volume ------------------------------------------------------------

/// Bytes of the regular files under `root`, without following links;
/// unreadable directories and vanished entries are skipped.
pub fn directory_file_bytes(root: &Path) -> u64 {
    if !root.is_dir() {
        return 0;
    }
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                total += entry.metadata().map_or(0, |metadata| metadata.len());
            } else if kind.is_symlink() {
                // Python's is_file() follows the link to a regular file.
                if let Ok(metadata) = std::fs::metadata(entry.path())
                    && metadata.is_file()
                {
                    total += metadata.len();
                }
            }
        }
    }
    total
}

pub fn check_analytics_storage(config: &ExternalConfig) -> Vec<String> {
    let data_dir = &config.analytics_data_dir;
    if !data_dir.exists() {
        return vec![format!(
            "analytics data directory is missing: {}",
            data_dir.display()
        )];
    }
    let mut errors = Vec::new();
    let used = directory_file_bytes(data_dir);
    if used > config.analytics_max_data_bytes {
        errors.push(format!(
            "analytics data is {used} bytes; configured limit is {} bytes",
            config.analytics_max_data_bytes
        ));
    }
    match crate::cli::monitors::health::system_disk_usage(data_dir) {
        Ok(usage) => {
            let free_percent = if usage.total > 0 {
                usage.free as f64 / usage.total as f64 * 100.0
            } else {
                0.0
            };
            if free_percent < config.min_disk_free_percent {
                errors.push(format!(
                    "analytics volume has {free_percent:.1}% free; expected at least {:.1}%",
                    config.min_disk_free_percent
                ));
            }
        }
        Err(_) => errors.push("analytics storage check crashed: disk usage unavailable".into()),
    }
    errors
}

// ---- frontend bundles --------------------------------------------------------------

pub async fn check_frontend_bundles(
    client: &reqwest::Client,
    config: &ExternalConfig,
) -> Vec<String> {
    static SCRIPT: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r#"(?:src|href)=["']([^"']*?/assets/index-[^"']+\.js)["']"#)
            .expect("a valid pattern")
    });
    if !config.bundle_checks_enabled {
        return Vec::new();
    }
    let timeout = Duration::from_secs(config.http_timeout_seconds);
    let mut errors = Vec::new();
    for check in &config.bundle_checks {
        let (code, html) = match get(client, &check.url, timeout).await {
            Ok(fetched) => (fetched.status, fetched.body),
            Err(reason) => (0, reason.to_owned()),
        };
        if code != 200 {
            if code == 403 && looks_like_vercel_security_checkpoint(&html) {
                log(&format!(
                    "{} skipped: Vercel returned Security Checkpoint to monitor client",
                    check.name
                ));
                continue;
            }
            errors.push(format!(
                "{} page returned HTTP {code}, expected 200",
                check.name
            ));
            continue;
        }
        let Some(script) = SCRIPT
            .captures_iter(&html)
            .filter_map(|captures| captures.get(1))
            .last()
        else {
            errors.push(format!(
                "{} could not find the main frontend bundle",
                check.name
            ));
            continue;
        };
        let Some(bundle_url) = url::Url::parse(&check.url)
            .ok()
            .and_then(|base| base.join(script.as_str()).ok())
        else {
            errors.push(format!(
                "{} could not find the main frontend bundle",
                check.name
            ));
            continue;
        };
        let (bundle_code, bundle) = match get(client, bundle_url.as_str(), timeout).await {
            Ok(fetched) => (fetched.status, fetched.body),
            Err(_) => (0, String::new()),
        };
        if bundle_code != 200 {
            errors.push(format!(
                "{} bundle returned HTTP {bundle_code}, expected 200",
                check.name
            ));
            continue;
        }
        for needle in &check.must_contain {
            if !bundle.contains(needle.as_str()) {
                errors.push(format!(
                    "{} bundle missing required text {}",
                    check.name,
                    quoted(needle)
                ));
            }
        }
        for needle in &check.must_not_contain {
            if bundle.contains(needle.as_str()) {
                errors.push(format!(
                    "{} bundle still contains forbidden text {}",
                    check.name,
                    quoted(needle)
                ));
            }
        }
    }
    errors
}

// ---- signup canary -----------------------------------------------------------------

/// Email-first registration: `register` answers 202 `check_email` and creates
/// no row for a monitor canary, so a healthy cleanup deletes nothing. The
/// cleanup always runs, even after a failed registration.
pub async fn signup_canary(
    client: &reqwest::Client,
    config: &ExternalConfig,
    name: &str,
    api_base: &str,
    now: DateTime<Utc>,
) -> Option<String> {
    let Some(token) = config.monitor_token.as_deref() else {
        return Some(format!("{name} signup canary missing MONITOR_TOKEN"));
    };
    let stamp = now.timestamp();
    let domain = format!("{name}-{stamp}.canary.verdaxis.exchange");
    let email = format!("canary+{name}-{stamp}@{domain}");
    let registration = json!({
        "email": email,
        "password": format!("VerdaxisCanary-{stamp}-check"),
        "first_name": "Signup",
        "last_name": "Canary",
        "role": "BUYER",
    });
    let mut signup_succeeded = false;
    let signup_error = match post_json(
        client,
        &format!("{api_base}/auth/register"),
        &registration,
        token,
    )
    .await
    {
        Err(_) => Some(format!("{name} signup canary failed (request)")),
        Ok(fetched) if fetched.status != 202 => Some(format!(
            "{name} signup canary register returned HTTP {}",
            fetched.status
        )),
        Ok(fetched) => {
            let answer: Option<Value> = serde_json::from_str(&fetched.body).ok();
            if answer.as_ref().and_then(|value| value.get("status")) == Some(&json!("check_email"))
            {
                signup_succeeded = true;
                None
            } else {
                Some(format!(
                    "{name} signup canary register returned invalid response"
                ))
            }
        }
    };
    let cleanup_error = match post_json(
        client,
        &format!("{api_base}/monitor/signup-canary-cleanup"),
        &json!({"email": email}),
        token,
    )
    .await
    {
        Err(_) => Some(format!("{name} signup canary cleanup failed (request)")),
        Ok(fetched) if fetched.status != 200 => Some(format!(
            "{name} signup canary cleanup returned HTTP {}",
            fetched.status
        )),
        Ok(fetched) => {
            let answer: Option<Value> = serde_json::from_str(&fetched.body).ok();
            // Python's `type(x) is int`: an integer, not a boolean or float.
            let count = |key: &str| {
                answer
                    .as_ref()
                    .and_then(|value| value.get(key))
                    .filter(|value| value.is_i64() || value.is_u64())
                    .and_then(Value::as_i64)
                    .filter(|count| matches!(count, 0 | 1))
            };
            match (count("deleted_users"), count("deleted_orgs")) {
                (Some(users), Some(orgs)) => {
                    (signup_succeeded && (users, orgs) != (0, 0)).then(|| {
                        format!("{name} signup canary cleanup reported an undeleted identity")
                    })
                }
                _ => Some(format!(
                    "{name} signup canary cleanup returned invalid deletion counts"
                )),
            }
        }
    };
    if let Some(error) = &cleanup_error {
        log(error);
    }
    let errors: Vec<String> = [signup_error, cleanup_error]
        .into_iter()
        .flatten()
        .collect();
    (!errors.is_empty()).then(|| errors.join("; "))
}

// ---- demo trade canary ---------------------------------------------------------------

pub async fn demo_trade_canary(
    client: &reqwest::Client,
    name: &str,
    api_base: &str,
    now: DateTime<Utc>,
) -> Option<String> {
    let fetched = match get(
        client,
        &format!("{api_base}/trade-tape?demo_only=true&limit=1"),
        REQUEST_TIMEOUT,
    )
    .await
    {
        Ok(fetched) => fetched,
        Err(_) => return Some(format!("{name} demo trade canary returned HTTP 0")),
    };
    if fetched.status != 200 {
        return Some(format!(
            "{name} demo trade canary returned HTTP {}",
            fetched.status
        ));
    }
    let Ok(payload) = serde_json::from_str::<Value>(&fetched.body) else {
        return Some(format!("{name} demo trade canary returned invalid JSON"));
    };
    let item = match payload.get("items").and_then(Value::as_array) {
        Some(items) if items.len() == 1 && items[0].is_object() => &items[0],
        _ => {
            return Some(format!(
                "{name} demo trade canary returned an invalid item list"
            ));
        }
    };
    if item.get("is_demo_trade") != Some(&Value::Bool(true))
        || item.get("demo_status") != Some(&json!("DEMO_ONLY"))
        || item.get("source_kind") != Some(&json!("DEMO_SEED"))
        || item.get("provenance_kind") != Some(&json!("DEMO_SEED"))
    {
        return Some(format!(
            "{name} demo trade canary returned invalid DEMO evidence"
        ));
    }
    let Some(confirmed) = item.get("confirmed_at").and_then(Value::as_str) else {
        return Some(format!(
            "{name} demo trade canary returned an invalid timestamp"
        ));
    };
    let Some((confirmed, has_offset)) = python_fromisoformat(&confirmed.replace('Z', "+00:00"))
    else {
        return Some(format!(
            "{name} demo trade canary returned an invalid timestamp"
        ));
    };
    if !has_offset || confirmed.offset().local_minus_utc() != 0 {
        return Some(format!("{name} demo trade canary timestamp is not UTC"));
    }
    let age = age_seconds(now, confirmed);
    if age < -DEMO_TRADE_FUTURE_TOLERANCE_SECONDS {
        return Some(format!(
            "{name} demo trade canary timestamp is too far in the future"
        ));
    }
    if age > DEMO_TRADE_MAX_AGE_SECONDS {
        return Some(format!("{name} demo trade canary is stale"));
    }
    None
}

// ---- expensive checks ------------------------------------------------------------------

/// The last expensive result while its slot is active: an integer
/// `checked_at` no older than the interval and not in the future, and a list
/// of text errors.
pub fn load_cached_expensive_checks(
    config: &ExternalConfig,
    now: i64,
) -> Option<(Vec<String>, Value)> {
    let status = load_state(&config.status_file);
    let cached = status.get("expensive_checks")?.as_object()?;
    let checked_at = cached
        .get("checked_at")
        .filter(|value| value.is_i64() || value.is_u64())?
        .as_i64()?;
    let errors: Vec<String> = cached
        .get("errors")?
        .as_array()?
        .iter()
        .map(|error| error.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    let interval = config.expensive_interval_seconds?;
    let age = now - checked_at;
    (0..interval)
        .contains(&age)
        .then(|| (errors, Value::Object(cached.clone())))
}

pub async fn run_expensive_checks(
    client: &reqwest::Client,
    config: &ExternalConfig,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut errors = check_frontend_bundles(client, config).await;
    errors.extend(webdriver::check_rendered_pages(config).await);
    errors.extend(check_backup_status(config, now));
    errors.extend(check_restore_status(config, now));
    if config.signup_canary_enabled {
        for (name, base) in &config.canary_targets {
            errors.extend(signup_canary(client, config, name, base, now).await);
        }
    }
    if config.demo_trade_canary_enabled {
        for (name, base) in &config.canary_targets {
            errors.extend(demo_trade_canary(client, name, base, now).await);
        }
    }
    errors.extend(check_analytics_storage(config));
    errors
}

async fn expensive_if_due(
    client: &reqwest::Client,
    config: &ExternalConfig,
    clock: &dyn Fn() -> DateTime<Utc>,
) -> (Vec<String>, Value) {
    if let Some(cached) = load_cached_expensive_checks(config, clock().timestamp()) {
        return cached;
    }
    let errors = run_expensive_checks(client, config, clock()).await;
    let checked_at = clock().timestamp();
    let status = json!({
        "ok": errors.is_empty(),
        "checked_at": checked_at,
        "checked_at_utc": utc_text(checked_at),
        "errors": errors,
    });
    (errors, status)
}

// ---- alerts --------------------------------------------------------------------------

async fn telegram_alert(client: &reqwest::Client, config: &ExternalConfig, message: &str) {
    if config.telegram_disabled {
        return;
    }
    let (Some(token), Some(chat)) = (&config.telegram_token, &config.telegram_chat_id) else {
        return;
    };
    let url = format!("{}/bot{token}/sendMessage", config.telegram_base);
    let sent = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("chat_id", chat)
                .append_pair("text", message)
                .finish(),
        )
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    if let Err(error) = sent {
        log(&format!(
            "telegram alert failed: {}",
            request_failure(&error)
        ));
    }
}

async fn healthchecks_ping(
    client: &reqwest::Client,
    config: &ExternalConfig,
    ok: bool,
    message: &str,
) {
    let Some(base) = &config.healthchecks_ping_url else {
        return;
    };
    let url = if ok {
        base.clone()
    } else {
        format!("{}/fail", base.trim_end_matches('/'))
    };
    let sent = client
        .post(url)
        .body(message.to_owned())
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    if let Err(error) = sent {
        log(&format!(
            "healthchecks ping failed: {}",
            request_failure(&error)
        ));
    }
}

/// Python's `int(state.get("last_alert_at", 0))` for the values it accepts.
fn last_alert_at(state: &Map<String, Value>) -> i64 {
    match state.get("last_alert_at") {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value.trunc() as i64))
            .unwrap_or(0),
        Some(Value::Bool(value)) => i64::from(*value),
        Some(Value::String(text)) => python_int(text).unwrap_or(0),
        _ => 0,
    }
}

pub async fn maybe_alert(
    client: &reqwest::Client,
    config: &ExternalConfig,
    ok: bool,
    errors: &[String],
    now: i64,
) -> Result<(), Exit> {
    let state = load_state(&config.state_file);
    let previous_failed = state.get("ok") == Some(&Value::Bool(false));
    let mut last = last_alert_at(&state);
    if ok {
        let message = "Verdaxis monitor recovered: all checks passing";
        if previous_failed {
            log(message);
            telegram_alert(client, config, message).await;
        }
        healthchecks_ping(client, config, true, "ok").await;
        return save_state(
            &config.state_file,
            &json!({"ok": true, "last_checked_at": now, "last_alert_at": last}),
        );
    }
    let lines: Vec<String> = errors.iter().map(|error| format!("- {error}")).collect();
    let message = format!("Verdaxis monitor failure:\n{}", lines.join("\n"));
    let should_alert = !previous_failed || now - last >= config.alert_cooldown_seconds;
    log(&message);
    if should_alert {
        telegram_alert(client, config, &message).await;
        healthchecks_ping(client, config, false, &message).await;
        last = now;
    }
    save_state(
        &config.state_file,
        &json!({"ok": false, "last_checked_at": now, "last_alert_at": last, "errors": errors}),
    )
}

// ---- one run -------------------------------------------------------------------------

/// One monitor run with an injected clock. Exit 0 when every check passed,
/// 2 when any failed, 4 when the state or status file cannot be written.
pub async fn run_with(config: &ExternalConfig, clock: &dyn Fn() -> DateTime<Utc>) -> Exit {
    let Some(client) = http_client() else {
        return Exit::Dependency;
    };
    let mut errors = check_caddyfile(config).await;
    let (endpoint_errors, endpoint_statuses) = check_endpoints(&client, config).await;
    errors.extend(endpoint_errors);
    errors.extend(check_units(config).await);
    let (expensive_errors, expensive_status) = expensive_if_due(&client, config, clock).await;
    errors.extend(expensive_errors);
    let ok = errors.is_empty();
    let now = clock().timestamp();
    let status = json!({
        "ok": ok,
        "checked_at": now,
        "checked_at_utc": utc_text(now),
        "errors": errors,
        "endpoints": endpoint_statuses,
        "expensive_checks": expensive_status,
        "alert_cooldown_seconds": config.alert_cooldown_seconds,
    });
    if let Err(exit) = save_state(&config.status_file, &status) {
        return exit;
    }
    if let Err(exit) = maybe_alert(&client, config, ok, &errors, now).await {
        return exit;
    }
    if ok {
        println!("Verdaxis monitor OK");
        Exit::Ok
    } else {
        Exit::Usage
    }
}

/// Entry point for `monitor external [--telegram-disabled]`.
pub async fn run(telegram_disabled: bool) -> Exit {
    let vars: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let home = vars
        .get("HOME")
        .map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    match ExternalConfig::from_environment(&vars, &home, telegram_disabled) {
        Ok(config) => run_with(&config, &Utc::now).await,
        Err(ConfigError(key)) => {
            log(&format!("monitor external: {key} is not a valid setting"));
            Exit::Precondition
        }
    }
}
