use crate::{Config, DEFAULT_READ_ADMISSION_SLOTS, verify_release};
use percent_encoding::percent_decode_str;
use std::net::IpAddr;

pub fn check_revision(actual: &str, allowed: &[String]) -> Result<(), String> {
    if allowed.iter().any(|value| value == actual) {
        Ok(())
    } else {
        Err("migration revision is outside the compiled allowlist".into())
    }
}

pub fn connection_budget(
    services: u32,
    workers: u32,
    pool: u32,
    overflow: u32,
    reserve: u32,
    rust_pool: u32,
) -> Option<u32> {
    services
        .checked_mul(workers)?
        .checked_mul(pool.checked_add(overflow)?.checked_add(1)?)?
        .checked_add(reserve)?
        .checked_add(rust_pool)
}

pub fn validate_budget(total: u32, configured_max: u32, observed_max: u32) -> Result<(), String> {
    if total <= configured_max && total <= observed_max {
        Ok(())
    } else {
        Err("connection budget exceeds configured or observed max_connections".into())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedBudget {
    version: u32,
    release_sha: String,
    environment_digest: String,
    services: u32,
    workers: u32,
    pool: u32,
    overflow: u32,
    reserve: u32,
    rust_pool: u32,
    python_budget: u32,
    /// One when the dedicated cache listener connection is budgeted, else zero.
    /// Absent from artifacts that predate the listener, which read as zero.
    #[serde(default)]
    cache_listener: u32,
    total: u32,
    configured_max_connections: u32,
}

impl PreparedBudget {
    /// Python's budget, the Rust pool and the optional listener connection.
    fn expected_total(&self) -> Option<u32> {
        self.python_budget
            .checked_add(self.rust_pool)?
            .checked_add(self.cache_listener)
    }
}

/// Validate public budget metadata only; the prestart helper checks the secret file digest.
pub fn validate_prepared_budget(config: &Config, json: &str) -> Result<(), String> {
    let budget: PreparedBudget =
        serde_json::from_str(json).map_err(|_| "invalid prepared budget")?;
    let total = connection_budget(
        budget.services,
        budget.workers,
        budget.pool,
        budget.overflow,
        budget.reserve,
        budget.rust_pool,
    )
    .and_then(|total| total.checked_add(budget.cache_listener));
    if budget.version != 1
        || budget.cache_listener > 1
        || budget.release_sha != config.release_sha
        || budget.environment_digest.len() != 64
        || !budget
            .environment_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || budget.rust_pool != config.pool_max
        || budget.python_budget != config.python_budget
        || budget.configured_max_connections != config.configured_max_connections
        || total != Some(budget.total)
        || budget.expected_total() != total
    {
        return Err("prepared budget does not match runtime configuration".into());
    }
    validate_config(config)?;
    // The listener connection is outside `pool_max`, so check the whole budget.
    validate_budget(
        budget.total,
        config.configured_max_connections,
        config.configured_max_connections,
    )
}

/// Build the runtime configuration from the flat deployed environment.
///
/// The reviewed staging unit loads `.runtime-release.env` and
/// `.runtime-rust/runtime.env` and passes the prepared budget artifact with
/// `--budget-file`; the contract harness passes one `RUST_CONFIG` document
/// instead. Both routes end at the same validated `Config`, so a deployed
/// start cannot skip a check the tests hold.
pub fn config_from_deployed_environment(
    environment: &std::collections::HashMap<String, String>,
    bind: std::net::SocketAddr,
    budget_json: &str,
) -> Result<Config, String> {
    let required = |key: &str| {
        environment
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} must be set in the deployed environment"))
    };
    let raw_required = |key: &str| {
        environment
            .get(key)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .ok_or_else(|| format!("{key} must be set in the deployed environment"))
    };
    let release_sha = required("RELEASE_SHA")?;
    let budget: PreparedBudget = serde_json::from_str(budget_json)
        .map_err(|_| "the prepared budget artifact is not valid JSON")?;
    let read_admission_slots = environment
        .get("READ_ADMISSION_SLOTS")
        .map(|value| value.trim().parse::<u32>())
        .transpose()
        .map_err(|_| "READ_ADMISSION_SLOTS must be a positive integer")?
        .unwrap_or(DEFAULT_READ_ADMISSION_SLOTS);
    let rust_pool = read_admission_slots
        .checked_add(1)
        .filter(|_| read_admission_slots >= 1)
        .ok_or("READ_ADMISSION_SLOTS must be positive and leave room for readiness")?;
    if budget.version != 1 || budget.release_sha != release_sha {
        return Err("the prepared budget artifact does not belong to this release".into());
    }
    if budget.rust_pool != rust_pool
        || budget.cache_listener > 1
        || budget.expected_total() != Some(budget.total)
    {
        return Err("the prepared budget artifact is internally inconsistent".into());
    }
    Ok(Config {
        kyc_execution_gate_enabled: deployed_bool(environment, "KYC_EXECUTION_GATE_ENABLED", true)?,
        environment: required("ENVIRONMENT")?,
        release_sha,
        migration_revision: required("MIGRATION_REVISION")?,
        database_url: required("DATABASE_URL")?,
        db_session_time_zone: required("DB_SESSION_TIME_ZONE")?,
        prepared_budget: Some(budget_json.to_owned()),
        bind,
        read_admission_slots,
        pool_max: rust_pool,
        python_budget: budget.python_budget,
        configured_max_connections: budget.configured_max_connections,
        phase2: crate::Phase2Options {
            // Task 23: the deploy transaction writes these to the Rust runtime
            // file only. A deployed start without them is refused rather than
            // serving Phase 2 routes with an unverifiable token authority.
            jwt_secret: Some(raw_required("JWT_SECRET")?),
            jwt_secret_previous: environment
                .get("JWT_SECRET_PREVIOUS")
                .filter(|value| !value.trim().is_empty())
                .cloned(),
            jwt_issuer: Some(raw_required("JWT_ISSUER")?),
            jwt_audience: Some(raw_required("JWT_AUDIENCE")?),
            frontend_url: Some(raw_required("FRONTEND_URL")?),
            email_from: Some(raw_required("EMAIL_FROM")?),
            resend_api_key: Some(raw_required("RESEND_API_KEY")?),
            // PD5: only `.runtime-release.env` carries this key; the runtime
            // preparer never projects it from the deployed `.env`.
            watchlist_producer_activated_at: environment
                .get("WATCHLIST_PRODUCER_ACTIVATED_AT")
                .cloned(),
        },
    })
}

/// Resolve a deployed boolean switch. The environment file decides, so Rust
/// cannot answer differently from Python; an absent key takes the caller's
/// default, which mirrors the Python setting it stands for.
fn deployed_bool(
    environment: &std::collections::HashMap<String, String>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match environment.get(key) {
        None => Ok(default),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "y" | "yes" | "1" | "on" => Ok(true),
            "false" | "f" | "n" | "no" | "0" | "off" => Ok(false),
            _ => Err(format!("{key} must be true or false")),
        },
    }
}

/// The two arguments the reviewed staging unit passes: the loopback bind and
/// the path of the prepared budget artifact. Absolute paths only, so a start
/// from an unexpected working directory cannot redirect the read.
pub fn deployed_arguments(
    arguments: &[String],
) -> Result<(std::net::SocketAddr, std::path::PathBuf), String> {
    let (mut bind, mut budget) = (None, None);
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?;
        let slot = match flag {
            "--bind" => &mut bind,
            "--budget-file" => &mut budget,
            other => return Err(format!("unsupported argument: {other}")),
        };
        if slot.is_some() {
            return Err(format!("{flag} was given more than once"));
        }
        *slot = Some(value.clone());
        index += 2;
    }
    let bind = bind
        .ok_or("--bind is required")?
        .parse()
        .map_err(|_| "--bind is not a socket address".to_owned())?;
    let budget = std::path::PathBuf::from(budget.ok_or("--budget-file is required")?);
    if !budget.is_absolute() {
        return Err("--budget-file must be an absolute path".into());
    }
    Ok((bind, budget))
}

pub(crate) struct DatabaseTarget {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
}

pub(crate) fn database_target(config: &Config) -> Result<DatabaseTarget, String> {
    let invalid = || "invalid or unsafe database URL".to_owned();
    let raw = &config.database_url;
    if raw.chars().any(char::is_whitespace) || raw.contains(['?', '#']) {
        return Err(invalid());
    }
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(invalid());
    }
    let decode = |value: &str| {
        percent_decode_str(value)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|_| invalid())
    };
    let host = url
        .host_str()
        .ok_or_else(invalid)?
        .trim_matches(['[', ']'])
        .to_owned();
    let ip: IpAddr = host.parse().map_err(|_| invalid())?;
    let target = DatabaseTarget {
        host,
        port: url.port().unwrap_or(5432),
        database: decode(url.path().strip_prefix('/').ok_or_else(invalid)?)?,
        user: decode(url.username())?,
        password: decode(url.password().ok_or_else(invalid)?)?,
    };
    if !ip.is_loopback()
        || target.user.is_empty()
        || target.database.is_empty()
        || target.database.contains('/')
        || target.password.trim().is_empty()
        || matches!(
            target.password.trim().to_lowercase().as_str(),
            "postgres" | "change_me"
        )
    {
        return Err(invalid());
    }
    if config.environment == "staging"
        && (target.host != "127.0.0.1"
            || target.port != 5432
            || target.user != "verdaxis_app_staging"
            || target.database != "verdaxis_staging")
    {
        return Err(invalid());
    }
    if config.environment == "production"
        && (target.host != "127.0.0.1"
            || target.port != 5432
            || target.user != "verdaxis_app"
            || target.database != "verdaxis_prod")
    {
        return Err(invalid());
    }
    if config.environment == "test"
        && (!target.database.ends_with("_test")
            || !target
                .database
                .starts_with(|c: char| c.is_ascii_lowercase())
            || !target
                .database
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'))
    {
        return Err(invalid());
    }
    Ok(target)
}

pub fn validate_config(config: &Config) -> Result<(), String> {
    if !matches!(
        config.environment.as_str(),
        "development" | "test" | "staging" | "production"
    ) {
        return Err("unsupported runtime environment".into());
    }
    // Require an explicit named zone, not an ambiguous offset or empty policy.
    // The connection hook also verifies PostgreSQL's effective zone.
    if config
        .db_session_time_zone
        .parse::<chrono_tz::Tz>()
        .is_err()
    {
        return Err("DB_SESSION_TIME_ZONE must be an IANA time zone".into());
    }
    verify_release(&config.release_sha)?;
    if !config.bind.ip().is_loopback() {
        return Err("HTTP bind must be loopback".into());
    }
    if config.read_admission_slots == 0
        || config.read_admission_slots.checked_add(1) != Some(config.pool_max)
    {
        return Err(
            "read admission slots must be positive and pool_max must equal slots plus one".into(),
        );
    }
    let allowed: Vec<String> = serde_json::from_str(include_str!(
        "../../../../deploy/rust/allowed-revisions.json"
    ))
    .map_err(|_| "invalid compiled revision allowlist")?;
    check_revision(&config.migration_revision, &allowed)?;
    if config.environment == "production" {
        if !matches!(
            config.migration_revision.as_str(),
            "p0_20261016_union" | "prc_20261026_changes" | "p3_20261020_auth_tokens"
        ) {
            return Err("production requires its reviewed union checkpoint".into());
        }
        if config.prepared_budget.is_none() {
            return Err("production requires its prepared connection budget".into());
        }
    }
    database_target(config)?;
    let total = config
        .python_budget
        .checked_add(config.pool_max)
        .ok_or("connection budget overflow")?;
    validate_budget(
        total,
        config.configured_max_connections,
        config.configured_max_connections,
    )
}
