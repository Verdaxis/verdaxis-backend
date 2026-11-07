//! `ops prepare-runtime`: project the deployed `.env` into one private file per
//! purpose (Phase 10 Task 9).
//!
//! `serve` gets `runtime.env` and `auth-domains.json`, byte-identical to
//! `scripts/prepare_rust_runtime.py`, which this replaces. Each job gets
//! `jobs/<name>.env`. The keys of every file are exactly the rows of
//! `deploy/rust/runtime-policy.env`; the full `.env` is never copied and the
//! migrator URL is never written. Every value is checked before the first
//! file is written, so a refusal leaves the previous files in place.
//!
//! Spellings that Python accepted but this port refuses (each would stop the
//! deploy before any change, never pass a different value): seller fees and
//! market integers in exponent, underscore or non-ASCII digit form, a negative
//! zero fee, a `DATABASE_URL` with `?` or `#` or a control character, and a
//! `.env` line that python-dotenv would have skipped.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::Digest;

use super::dotenv::{self, Bindings, is_python_space};
use crate::cli::{Environment, Exit, PrepareRuntimeArgs, state};
use crate::runtime::config::PRODUCTION_REVISIONS;

const RUNTIME_POLICY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/runtime-policy.env"
));
const DATASETS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/auth-domain-datasets.json"
));
const DATASETS_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/auth-domain-datasets.manifest.json"
));
const STAGING_POLICY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/staging-policy.json"
));
const PRODUCTION_POLICY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/production-policy.json"
));

/// A deployed `.env` is a few kilobytes; anything near this bound is not one.
pub const MAX_ENV_BYTES: u64 = 256 * 1024;
const ANALYTICS_ENVIRONMENT_FILE: &str = ".env.analytics";
const UMAMI_KEYS: [&str; 4] = [
    "UMAMI_BASE_URL",
    "UMAMI_WEBSITE_ID",
    "UMAMI_API_USERNAME",
    "UMAMI_API_PASSWORD",
];
const PHASE2_REQUIRED_KEYS: [&str; 6] = [
    "JWT_SECRET",
    "JWT_ISSUER",
    "JWT_AUDIENCE",
    "FRONTEND_URL",
    "EMAIL_FROM",
    "RESEND_API_KEY",
];
/// HS256 keys shorter than this are refused (Phase 3 token policy).
const MIN_JWT_SECRET_CHARS: usize = 32;
const SYSTEM_STATUSES: [&str; 3] = ["operational", "degraded", "maintenance"];
const MAX_SYSTEM_STATUS_MESSAGE_CHARS: usize = 280;
/// Python's defaults and bounds (app/config.py) for the market write keys.
const MARKET_BOOL_KEYS: [(&str, bool); 2] = [
    ("AUTO_MATCHING_ENABLED", true),
    ("MARKET_SUPPORT_ENABLED", false),
];
const MARKET_INTEGER_KEYS: [(&str, i64, i64); 2] = [
    ("MARKET_SUPPORT_MAX_TTL_HOURS", 1, 720),
    ("MARKET_SUPPORT_CONTEXT_TTL_MINUTES", 1, 1440),
];
const DEFAULT_CONSENT_VERSION: &str = "v1";
const DEFAULT_CONSENT_REFERENCE: &str = "market-support/v1";
const MARKET_TEXT_KEYS: [&str; 3] = [
    "MARKET_SUPPORT_CONSENT_VERSION",
    "MARKET_SUPPORT_CONSENT_REFERENCE",
    "MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS",
];
/// Seller fees: (key, default as Python writes it). The upper bound is USD/MT.
const SELLER_FEES: [(&str, &str); 2] = [
    ("SELLER_FEE_PILOT_PER_MT_USD", "2"),
    ("SELLER_FEE_PROFESSIONAL_PER_MT_USD", "1.50"),
];
const MAX_SELLER_FEE: u64 = 100_000;
const DEFAULT_READ_ADMISSION_SLOTS: &str = "16";

/// Why a projection was refused. The reason names a key or a rule, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal(pub &'static str);

/// The fixed identity of one deployed environment.
pub struct Identity {
    pub database_name: &'static str,
    pub app_role: &'static str,
    pub allowed_hosts: &'static str,
    service_name: &'static str,
    bind: &'static str,
}

pub fn identity(environment: Environment) -> Identity {
    match environment {
        Environment::Production => Identity {
            database_name: "verdaxis_prod",
            app_role: "verdaxis_app",
            allowed_hosts: "api.verdaxis.exchange",
            service_name: "verdaxis-rust.service",
            bind: "127.0.0.1:8010",
        },
        Environment::Staging => Identity {
            database_name: "verdaxis_staging",
            app_role: "verdaxis_app_staging",
            allowed_hosts: "api-staging.verdaxis.exchange",
            service_name: "verdaxis-rust-staging.service",
            bind: "127.0.0.1:8011",
        },
    }
}

// ---- policy ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Required,
    Optional,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRow {
    pub purpose: String,
    pub key: String,
    pub status: Status,
    pub delivery: String,
}

/// The reviewed rows of `runtime-policy.env`.
pub fn policy() -> Vec<PolicyRow> {
    RUNTIME_POLICY
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let [purpose, key, status, delivery] = fields[..] else {
                panic!("runtime-policy.env row must have four tab-separated fields: {line}");
            };
            let status = match status {
                "required" => Status::Required,
                "optional" => Status::Optional,
                "derived" => Status::Derived,
                other => panic!("unknown runtime-policy.env status {other}"),
            };
            PolicyRow {
                purpose: purpose.to_owned(),
                key: key.to_owned(),
                status,
                delivery: delivery.to_owned(),
            }
        })
        .collect()
}

/// The session time zone of the environment's reviewed Rust policy file,
/// checked as `load_policy` checked it.
fn db_session_time_zone(environment: Environment) -> Result<String, Refusal> {
    let bytes = match environment {
        Environment::Staging => STAGING_POLICY,
        Environment::Production => PRODUCTION_POLICY,
    };
    let refused = Refusal("environment_policy");
    let policy: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| refused)?;
    let identity = identity(environment);
    let text = |key: &str| policy.get(key).and_then(serde_json::Value::as_str);
    let staged = text("staged_releases_dir").ok_or(refused)?;
    let zone = text("db_session_time_zone").ok_or(refused)?;
    if policy.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || text("environment") != Some(environment.as_str())
        || text("service_name") != Some(identity.service_name)
        || text("bind") != Some(identity.bind)
        || text("health_url") != Some(format!("http://{}/health/ready", identity.bind).as_str())
        || staged.starts_with('/')
        || staged.split('/').any(|part| part == "..")
        || !["UTC", "Asia/Singapore"].contains(&zone)
    {
        return Err(refused);
    }
    Ok(zone.to_owned())
}

/// The vendored datasets, checked against their manifest digest.
fn auth_domain_datasets() -> Result<&'static [u8], Refusal> {
    let manifest: serde_json::Value =
        serde_json::from_slice(DATASETS_MANIFEST).map_err(|_| Refusal("domain_datasets"))?;
    let digest: String = sha2::Sha256::digest(DATASETS)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if manifest.get("sha256").and_then(serde_json::Value::as_str) != Some(digest.as_str()) {
        return Err(Refusal("domain_datasets"));
    }
    Ok(DATASETS)
}

// ---- reading -----------------------------------------------------------------------

/// Read and parse one deployed environment file: regular, never a symlink,
/// bounded, UTF-8.
pub fn read_environment_file(path: &Path) -> Result<Bindings, Exit> {
    let bytes = state::read_regular_bytes(path, MAX_ENV_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Exit::Precondition)?;
    dotenv::parse(text).map_err(|_| Exit::Precondition)
}

/// One present value; `None` when the key is absent or written without `=`.
fn present<'a>(values: &'a Bindings, key: &str) -> Option<&'a str> {
    values.get(key).and_then(Option::as_deref)
}

fn is_blank(value: &str) -> bool {
    value.trim_matches(is_python_space).is_empty()
}

fn has_control_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| (character as u32) < 32 || character as u32 == 127)
}

// ---- identity ----------------------------------------------------------------------

/// Refuse a `.env` that belongs to another environment or release.
pub fn require_identity(
    values: &Bindings,
    environment: Environment,
    release_sha: &str,
) -> Result<(), Refusal> {
    match (
        present(values, "ENVIRONMENT"),
        present(values, "RELEASE_SHA"),
    ) {
        (Some(deployed), Some(release))
            if deployed == environment.as_str() && release == release_sha =>
        {
            Ok(())
        }
        _ => Err(Refusal("identity")),
    }
}

/// The deployed app `DATABASE_URL`, identity-checked and in libpq form.
///
/// The `.env` carries SQLAlchemy's `postgresql+asyncpg://` form; the Rust
/// process reads `postgresql://`. The user and the database must be the
/// environment's app role and database; the user, password and path are kept
/// exactly as written (still percent-encoded), as Python's `urlsplit` and
/// `urlunsplit` keep them.
pub fn app_database_url(values: &Bindings, environment: Environment) -> Result<String, Refusal> {
    let refused = Refusal("database_url");
    let raw = present(values, "DATABASE_URL").ok_or(refused)?;
    let url = raw.trim_matches(is_python_space);
    if url.is_empty() || has_control_character(url) || url.contains(['?', '#']) {
        return Err(refused);
    }
    let (scheme, rest) = url.split_once(':').ok_or(refused)?;
    let scheme = scheme.to_ascii_lowercase();
    let scheme = match scheme.as_str() {
        "postgresql+asyncpg" => "postgresql",
        "postgres" | "postgresql" | "postgresql+psycopg2" => scheme.as_str(),
        _ => return Err(refused),
    };
    let after_slashes = rest.strip_prefix("//").ok_or(refused)?;
    let netloc_end = after_slashes.find('/').unwrap_or(after_slashes.len());
    let (netloc, path) = after_slashes.split_at(netloc_end);
    let userinfo = netloc
        .rsplit_once('@')
        .map(|(userinfo, _)| userinfo)
        .ok_or(refused)?;
    let (user, password) = userinfo.split_once(':').ok_or(refused)?;
    let identity = identity(environment);
    if user != identity.app_role
        || path.trim_start_matches('/') != identity.database_name
        || is_blank(password)
    {
        return Err(refused);
    }
    Ok(format!("{scheme}://{netloc}{path}"))
}

// ---- serve -------------------------------------------------------------------------

/// Python's boolean spellings; an absent key is the caller's default.
fn deployed_bool(values: &Bindings, key: &str, default: bool) -> Result<bool, Refusal> {
    let Some(raw) = present(values, key) else {
        return Ok(default);
    };
    match raw.trim_matches(is_python_space).to_lowercase().as_str() {
        "true" | "t" | "y" | "yes" | "1" | "on" => Ok(true),
        "false" | "f" | "n" | "no" | "0" | "off" => Ok(false),
        _ => Err(Refusal("boolean")),
    }
}

fn bool_text(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

/// The required HTTP auth and mail keys, as written.
fn phase2_settings(
    values: &Bindings,
    projected: &mut BTreeMap<String, String>,
) -> Result<(), Refusal> {
    for key in PHASE2_REQUIRED_KEYS {
        let raw = present(values, key)
            .filter(|raw| !is_blank(raw))
            .ok_or(Refusal("auth_keys"))?;
        projected.insert(key.to_owned(), raw.to_owned());
    }
    if projected["JWT_SECRET"].chars().count() < MIN_JWT_SECRET_CHARS {
        return Err(Refusal("jwt_secret_length"));
    }
    if let Some(previous) = present(values, "JWT_SECRET_PREVIOUS").filter(|raw| !is_blank(raw)) {
        if previous.chars().count() < MIN_JWT_SECRET_CHARS {
            return Err(Refusal("jwt_secret_length"));
        }
        projected.insert("JWT_SECRET_PREVIOUS".to_owned(), previous.to_owned());
    }
    if !is_absolute_http_url(&projected["FRONTEND_URL"]) {
        return Err(Refusal("frontend_url"));
    }
    Ok(())
}

/// Python's `urlsplit` test: an `http` or `https` scheme (any case), a
/// non-empty network location and no whitespace anywhere.
fn is_absolute_http_url(url: &str) -> bool {
    if url.chars().any(is_python_space) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    let netloc = rest
        .strip_prefix("//")
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(""));
    ["http", "https"].contains(&scheme.to_ascii_lowercase().as_str())
        && netloc.is_some_and(|netloc| !netloc.is_empty())
}

/// The stream heartbeat status and message: trimmed, status lowercase.
fn system_status_settings(
    values: &Bindings,
    projected: &mut BTreeMap<String, String>,
) -> Result<(), Refusal> {
    if let Some(status) = present(values, "SYSTEM_STATUS") {
        let normalized = status.trim_matches(is_python_space).to_lowercase();
        if !normalized.is_empty() {
            if !SYSTEM_STATUSES.contains(&normalized.as_str()) {
                return Err(Refusal("system_status"));
            }
            projected.insert("SYSTEM_STATUS".to_owned(), normalized);
        }
    }
    if let Some(message) = present(values, "SYSTEM_STATUS_MESSAGE") {
        let message = message.trim_matches(is_python_space);
        if message.chars().count() > MAX_SYSTEM_STATUS_MESSAGE_CHARS {
            return Err(Refusal("system_status_message"));
        }
        if !message.is_empty() {
            projected.insert("SYSTEM_STATUS_MESSAGE".to_owned(), message.to_owned());
        }
    }
    Ok(())
}

/// An ASCII integer with an optional sign, as written after trimming.
fn plain_integer(raw: &str) -> Option<i64> {
    let trimmed = raw.trim_matches(is_python_space);
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

/// The market write keys present in the `.env`, validated as Python validated
/// them; booleans and integers in normal form, text as written.
fn market_write_settings(
    values: &Bindings,
    projected: &mut BTreeMap<String, String>,
) -> Result<(), Refusal> {
    for (key, default) in MARKET_BOOL_KEYS {
        if present(values, key).is_some() {
            projected.insert(
                key.to_owned(),
                bool_text(deployed_bool(values, key, default)?),
            );
        }
    }
    for (key, lowest, highest) in MARKET_INTEGER_KEYS {
        if let Some(raw) = present(values, key) {
            let number = plain_integer(raw)
                .filter(|number| (lowest..=highest).contains(number))
                .ok_or(Refusal("market_integer"))?;
            projected.insert(key.to_owned(), number.to_string());
        }
    }
    for key in MARKET_TEXT_KEYS {
        if let Some(raw) = present(values, key) {
            projected.insert(key.to_owned(), raw.to_owned());
        }
    }
    if deployed_bool(values, "MARKET_SUPPORT_ENABLED", false)? {
        let consent = |key: &str, default: &str| {
            present(values, key)
                .unwrap_or(default)
                .trim_matches(is_python_space)
                .to_owned()
        };
        let version = consent("MARKET_SUPPORT_CONSENT_VERSION", DEFAULT_CONSENT_VERSION);
        let reference = consent(
            "MARKET_SUPPORT_CONSENT_REFERENCE",
            DEFAULT_CONSENT_REFERENCE,
        );
        // Every environment this command serves is deployed, so the
        // placeholders are refused as Python refused them in staging and production.
        if version.is_empty()
            || reference.is_empty()
            || version == DEFAULT_CONSENT_VERSION
            || reference == DEFAULT_CONSENT_REFERENCE
        {
            return Err(Refusal("market_support_consent"));
        }
    }
    Ok(())
}

/// A seller fee in USD/MT: a plain decimal with at most two places, at most
/// 100000, written as Python's `str(Decimal(...))` writes it.
fn seller_fee(raw: Option<&str>, default: &str) -> Result<String, Refusal> {
    let refused = Refusal("seller_fee");
    let text = raw.filter(|raw| !raw.is_empty()).unwrap_or(default);
    let trimmed = text.trim_matches(is_python_space);
    let unsigned = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || fraction.is_some_and(|fraction| !digits(fraction) || fraction.len() > 2) {
        return Err(refused);
    }
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let whole_value: u64 = whole.parse().map_err(|_| refused)?;
    let fraction_is_zero =
        fraction.is_none_or(|fraction| fraction.bytes().all(|byte| byte == b'0'));
    if whole_value > MAX_SELLER_FEE || (whole_value == MAX_SELLER_FEE && !fraction_is_zero) {
        return Err(refused);
    }
    Ok(match fraction {
        Some(fraction) => format!("{whole}.{fraction}"),
        None => whole.to_owned(),
    })
}

/// The `serve` projection of `project_runtime_environment`, without the
/// datasets path, which the caller adds after publishing the datasets.
pub fn project_serve(
    values: &Bindings,
    environment: Environment,
    release_sha: &str,
    migration_revision: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    if environment == Environment::Production && !PRODUCTION_REVISIONS.contains(&migration_revision)
    {
        return Err(Refusal("production_revision"));
    }
    require_identity(values, environment, release_sha)?;
    let read_admission_slots = match values.get("READ_ADMISSION_SLOTS") {
        None => DEFAULT_READ_ADMISSION_SLOTS.to_owned(),
        Some(Some(raw)) => raw.clone(),
        Some(None) => return Err(Refusal("read_admission_slots")),
    };
    let mut projected = BTreeMap::from([
        ("ENVIRONMENT".to_owned(), environment.as_str().to_owned()),
        ("RELEASE_SHA".to_owned(), release_sha.to_owned()),
        (
            "MIGRATION_REVISION".to_owned(),
            migration_revision.to_owned(),
        ),
        (
            "DATABASE_URL".to_owned(),
            app_database_url(values, environment)?,
        ),
        (
            "DB_SESSION_TIME_ZONE".to_owned(),
            db_session_time_zone(environment)?,
        ),
        ("READ_ADMISSION_SLOTS".to_owned(), read_admission_slots),
        (
            "KYC_EXECUTION_GATE_ENABLED".to_owned(),
            bool_text(deployed_bool(values, "KYC_EXECUTION_GATE_ENABLED", true)?),
        ),
        (
            "ALLOWED_HOSTS".to_owned(),
            identity(environment).allowed_hosts.to_owned(),
        ),
    ]);
    phase2_settings(values, &mut projected)?;
    if let Some(token) = present(values, "MONITOR_TOKEN").filter(|token| !token.is_empty()) {
        projected.insert("MONITOR_TOKEN".to_owned(), token.to_owned());
    }
    for (key, default) in SELLER_FEES {
        projected.insert(key.to_owned(), seller_fee(present(values, key), default)?);
    }
    for key in UMAMI_KEYS {
        if let Some(value) = present(values, key).filter(|value| !is_blank(value)) {
            projected.insert(key.to_owned(), value.to_owned());
        }
    }
    let umami_enabled = UMAMI_KEYS.iter().all(|key| projected.contains_key(*key));
    projected.insert("UMAMI_ENABLED".to_owned(), bool_text(umami_enabled));
    system_status_settings(values, &mut projected)?;
    market_write_settings(values, &mut projected)?;
    refuse_control_characters(&projected)?;
    Ok(projected)
}

fn refuse_control_characters(projected: &BTreeMap<String, String>) -> Result<(), Refusal> {
    if projected.values().any(|value| has_control_character(value)) {
        return Err(Refusal("control_character"));
    }
    Ok(())
}

// ---- jobs --------------------------------------------------------------------------

/// Every `jobs/<name>.env` file, by delivery path, with exactly its policy rows.
pub fn project_jobs(
    values: &Bindings,
    environment: Environment,
    release_sha: &str,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, Refusal> {
    require_identity(values, environment, release_sha)?;
    let mut files: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for row in policy()
        .into_iter()
        .filter(|row| row.delivery.starts_with("jobs/"))
    {
        let value = match (row.status, row.key.as_str()) {
            (Status::Derived, "ENVIRONMENT") => Some(environment.as_str().to_owned()),
            (Status::Derived, "RELEASE_SHA") => Some(release_sha.to_owned()),
            (Status::Derived, "DATABASE_URL") => Some(app_database_url(values, environment)?),
            (Status::Derived, _) => return Err(Refusal("policy")),
            (Status::Required, key) => Some(
                present(values, key)
                    .filter(|raw| !is_blank(raw))
                    .ok_or(Refusal("job_required_key"))?
                    .to_owned(),
            ),
            (Status::Optional, key) => present(values, key)
                .filter(|raw| !is_blank(raw))
                .map(str::to_owned),
        };
        let file = files.entry(row.delivery).or_default();
        if let Some(value) = value {
            file.insert(row.key, value);
        }
    }
    for file in files.values() {
        refuse_control_characters(file)?;
    }
    Ok(files)
}

// ---- writing -----------------------------------------------------------------------

/// The `EnvironmentFile=` form Python wrote: a value of only URL-unreserved
/// characters is plain; anything else is double-quoted with `\` and `"`
/// escaped. Keys are sorted.
pub fn render(projected: &BTreeMap<String, String>) -> String {
    let plain = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte))
    };
    projected
        .iter()
        .map(|(key, value)| {
            if plain(value) {
                format!("{key}={value}\n")
            } else {
                let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
                format!("{key}=\"{escaped}\"\n")
            }
        })
        .collect()
}

/// Make `directory` a real directory (created 0700 when missing); a symlink
/// or another file type is refused.
fn private_directory(directory: &Path) -> Result<(), Exit> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(Exit::Precondition),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)
                .map_err(|_| Exit::Dependency)?;
            match std::fs::symlink_metadata(directory) {
                Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
                _ => Err(Exit::Precondition),
            }
        }
        Err(_) => Err(Exit::Dependency),
    }
}

/// A published path may be absent or a regular file, never a symlink.
fn replaceable(path: &Path) -> Result<(), Exit> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(Exit::Precondition),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Exit::Dependency),
    }
}

/// Read the deployed `.env` and, when present, the Umami keys of its
/// `.env.analytics` sibling, which win as in the Python unit's file order.
fn deployed_values(environment_file: &Path) -> Result<Bindings, Exit> {
    let mut values = read_environment_file(environment_file)?;
    let analytics_file = environment_file.with_file_name(ANALYTICS_ENVIRONMENT_FILE);
    if std::fs::symlink_metadata(&analytics_file).is_ok() {
        let analytics = read_environment_file(&analytics_file)?;
        for key in UMAMI_KEYS {
            if let Some(Some(value)) = analytics.get(key) {
                values.insert(key.to_owned(), Some(value.clone()));
            }
        }
    }
    Ok(values)
}

/// Run `ops prepare-runtime`. Nothing is written unless every file projects.
pub fn run(args: &PrepareRuntimeArgs) -> Result<(), (Exit, &'static str)> {
    let environment = args.identity.environment;
    let release_sha = args.identity.release_sha.as_str();
    let refused = |refusal: Refusal| (Exit::Precondition, refusal.0);
    let values =
        deployed_values(&args.environment_file).map_err(|exit| (exit, "environment_file"))?;
    let mut serve = project_serve(&values, environment, release_sha, &args.migration_revision)
        .map_err(refused)?;
    let jobs = project_jobs(&values, environment, release_sha).map_err(refused)?;
    let datasets = auth_domain_datasets().map_err(refused)?;

    let output = args.output_dir.as_path();
    if !output.is_absolute() {
        return Err((Exit::Precondition, "output_dir"));
    }
    let jobs_directory = output.join("jobs");
    for directory in [output, jobs_directory.as_path()] {
        private_directory(directory).map_err(|exit| (exit, "output_dir"))?;
    }
    let domains_path = output.join("auth-domains.json");
    let runtime_path = output.join("runtime.env");
    let job_paths: Vec<(PathBuf, String)> = jobs
        .iter()
        .map(|(delivery, file)| (output.join(delivery), render(file)))
        .collect();
    for path in job_paths
        .iter()
        .map(|(path, _)| path)
        .chain([&domains_path, &runtime_path])
    {
        replaceable(path).map_err(|exit| (exit, "output_file"))?;
    }

    // The datasets exceed per-variable limits, so serve reads them by path.
    let canonical_domains = output
        .canonicalize()
        .map_err(|_| (Exit::Dependency, "output_dir"))?
        .join("auth-domains.json");
    serve.insert(
        "AUTH_DOMAIN_DATASETS_FILE".to_owned(),
        canonical_domains
            .to_str()
            .ok_or((Exit::Precondition, "output_dir"))?
            .to_owned(),
    );
    refuse_control_characters(&serve).map_err(refused)?;

    let write = |path: &Path, bytes: &[u8]| {
        state::write_atomic_bytes(path, bytes).map_err(|exit| (exit, "write"))
    };
    write(&domains_path, datasets)?;
    for (path, body) in &job_paths {
        write(path, body.as_bytes())?;
    }
    write(&runtime_path, render(&serve).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_rows_are_well_formed() {
        let rows = policy();
        assert!(!rows.is_empty());
        for row in &rows {
            let derived_job_key =
                ["ENVIRONMENT", "RELEASE_SHA", "DATABASE_URL"].contains(&row.key.as_str());
            if row.delivery.starts_with("jobs/") && row.status == Status::Derived {
                assert!(derived_job_key, "{row:?}");
            }
            assert!(
                row.delivery == "runtime.env"
                    || row.delivery == "invocation"
                    || row.delivery.starts_with("credential:")
                    || (row.delivery.starts_with("jobs/") && row.delivery.ends_with(".env")),
                "{row:?}"
            );
        }
        // One key appears at most once per delivery file.
        let mut seen = std::collections::BTreeSet::new();
        assert!(
            rows.iter()
                .all(|row| seen.insert((&row.delivery, &row.key)))
        );
    }

    #[test]
    fn both_environment_policies_load() {
        assert_eq!(
            db_session_time_zone(Environment::Staging).as_deref(),
            Ok("UTC")
        );
        assert_eq!(
            db_session_time_zone(Environment::Production).as_deref(),
            Ok("UTC")
        );
        assert!(auth_domain_datasets().is_ok());
    }

    #[test]
    fn seller_fees_are_written_as_python_writes_them() {
        for (raw, written) in [
            (None, "2"),
            (Some(""), "2"),
            (Some("2.5"), "2.5"),
            (Some(" +02.50 "), "2.50"),
            (Some("0.05"), "0.05"),
            (Some("100000"), "100000"),
            (Some("100000.00"), "100000.00"),
        ] {
            assert_eq!(seller_fee(raw, "2").as_deref(), Ok(written), "{raw:?}");
        }
        for raw in ["2.505", "-1", "1e2", "1_0", "100000.01", "2.", ".5", "NaN"] {
            assert!(seller_fee(Some(raw), "2").is_err(), "{raw}");
        }
    }

    #[test]
    fn database_url_keeps_the_written_components() {
        let values =
            |url: &str| Bindings::from([("DATABASE_URL".to_owned(), Some(url.to_owned()))]);
        assert_eq!(
            app_database_url(
                &values(
                    " POSTGRESQL+ASYNCPG://verdaxis_app_staging:a%40b@db:5432//verdaxis_staging "
                ),
                Environment::Staging
            )
            .as_deref(),
            Ok("postgresql://verdaxis_app_staging:a%40b@db:5432//verdaxis_staging")
        );
        for url in [
            "postgresql://verdaxis_app_staging@db/verdaxis_staging",
            "postgresql://verdaxis_app_staging: @db/verdaxis_staging",
            "postgresql://verdaxis_app:pw@db/verdaxis_staging",
            "postgresql://verdaxis_app_staging:pw@db/verdaxis_prod",
            "postgresql://verdaxis_app_staging:pw@db/verdaxis_staging#x",
            "mysql://verdaxis_app_staging:pw@db/verdaxis_staging",
            "postgresql:verdaxis_app_staging:pw@db/verdaxis_staging",
        ] {
            assert!(
                app_database_url(&values(url), Environment::Staging).is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn render_quotes_like_python() {
        let projected = BTreeMap::from([
            ("A".to_owned(), "plain-1.2_3~".to_owned()),
            ("B".to_owned(), String::new()),
            ("C".to_owned(), "a b\"c\\d".to_owned()),
        ]);
        assert_eq!(
            render(&projected),
            "A=plain-1.2_3~\nB=\"\"\nC=\"a b\\\"c\\\\d\"\n"
        );
    }
}
