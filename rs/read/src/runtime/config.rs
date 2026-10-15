use crate::{verify_release, Config};
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
    total: u32,
    configured_max_connections: u32,
}

/// Python's aggregate public-read capacity: every worker reserves one fewer
/// than its own pool plus overflow. Rust's single gate must match it (AD-02).
pub fn public_read_slots(workers: u32, pool: u32, overflow: u32) -> Option<u32> {
    let per_worker = pool.checked_add(overflow)?.checked_sub(1)?;
    workers.checked_mul(per_worker)
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
    );
    if budget.version != 1
        || budget.release_sha != config.release_sha
        || budget.environment_digest.len() != 64
        || !budget
            .environment_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || public_read_slots(budget.workers, budget.pool, budget.overflow)
            != Some(crate::ADMISSION_SLOTS)
        || budget.rust_pool != config.pool_max
        || budget.python_budget != config.python_budget
        || budget.configured_max_connections != config.configured_max_connections
        || total != Some(budget.total)
        || budget.python_budget.checked_add(budget.rust_pool) != total
    {
        return Err("prepared budget does not match runtime configuration".into());
    }
    validate_config(config)
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
        "development" | "test" | "staging"
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
    if config.pool_max != 9 {
        return Err("Phase 1 requires exactly nine pool connections".into());
    }
    let allowed: Vec<String> = serde_json::from_str(include_str!(
        "../../../../deploy/rust/allowed-revisions.json"
    ))
    .map_err(|_| "invalid compiled revision allowlist")?;
    check_revision(&config.migration_revision, &allowed)?;
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
