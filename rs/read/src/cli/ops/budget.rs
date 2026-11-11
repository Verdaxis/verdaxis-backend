//! `ops prepare-budget`: the private connection budget artifact (Phase 10
//! Task 9), the one budget artifact the deploy installs.
//!
//! The budget is configured authority, never measured load: it is computed
//! from the deployed `.env` alone and opens no connection. The six settings
//! keep the defaults, bounds and cross-field rules of `app/config.py` that the
//! Python script applied by starting `Settings`. Other `Settings` rules (JWT,
//! URLs) are not the budget's concern: `ops prepare-runtime` and `ops
//! preflight` check those keys.
use std::collections::BTreeMap;
use std::path::Path;

use sha2::Digest;

use super::dotenv::{Bindings, is_python_space};
use super::runtime_env::{MAX_ENV_BYTES, Refusal, require_identity};
use crate::cli::{BudgetArgs, Environment, Exit, state};

/// (artifact key, setting, default, lowest, highest) from `app/config.py`.
const FIELDS: [(&str, &str, u64, u64, u64); 6] = [
    ("services", "DB_SERVICE_COUNT", 2, 1, 100),
    ("workers", "UVICORN_WORKERS", 4, 1, 100),
    ("pool", "DB_POOL_SIZE", 2, 1, 100),
    ("overflow", "DB_MAX_OVERFLOW", 1, 0, 100),
    ("reserve", "DB_RESERVED_CONNECTIONS", 20, 0, 999),
    (
        "configured_max_connections",
        "DB_MAX_CONNECTIONS",
        100,
        1,
        1000,
    ),
];
/// The deployed topology: production plus staging on one PostgreSQL server.
const DEPLOYED_SERVICE_COUNT: u64 = 2;
/// The minimum maintenance reserve in a deployed environment.
const DEPLOYED_MIN_RESERVE: u64 = 20;
/// Every count is a PostgreSQL connection count stored as `u32`.
const MAX_COUNT: u64 = u32::MAX as u64;
/// The stream election listener needs one of at least four workers free.
const MIN_WORKERS_FOR_STREAM_LISTENER: u64 = 4;
const DEFAULT_READ_ADMISSION_SLOTS: u64 = 16;
const MAX_ARTIFACT_BYTES: u64 = 4096;

/// The six configured settings, by artifact key.
pub struct Settings(BTreeMap<&'static str, u64>);

impl Settings {
    fn get(&self, key: &str) -> u64 {
        self.0[key]
    }
}

/// A pydantic integer from an environment string: surrounding whitespace, an
/// optional sign, ASCII digits and an optional all-zero fraction (`"4.0"`).
fn settings_integer(raw: &str) -> Option<i64> {
    let trimmed = raw.trim_matches(is_python_space);
    let (sign, unsigned) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let is_digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !is_digits(whole)
        || fraction.is_some_and(|fraction| {
            !is_digits(fraction) || fraction.bytes().any(|byte| byte != b'0')
        })
    {
        return None;
    }
    whole.parse::<i64>().ok().map(|value| sign * value)
}

/// The six settings as `Settings` resolved them, with its validators.
pub fn configured_settings(values: &Bindings) -> Result<Settings, Refusal> {
    let mut settings = BTreeMap::new();
    for (key, setting, default, lowest, highest) in FIELDS {
        let value = match values.get(setting).and_then(Option::as_deref) {
            None => default,
            Some(raw) => settings_integer(raw)
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| (lowest..=highest).contains(value))
                .ok_or(Refusal("budget_setting"))?,
        };
        settings.insert(key, value);
    }
    let settings = Settings(settings);
    let (services, workers) = (settings.get("services"), settings.get("workers"));
    let (reserve, maximum) = (
        settings.get("reserve"),
        settings.get("configured_max_connections"),
    );
    if reserve >= maximum {
        return Err(Refusal("budget_reserve"));
    }
    if services * workers * (settings.get("pool") + settings.get("overflow")) > maximum - reserve {
        return Err(Refusal("budget_capacity"));
    }
    // Both environments this command serves are deployed ones.
    if services != DEPLOYED_SERVICE_COUNT || reserve < DEPLOYED_MIN_RESERVE {
        return Err(Refusal("budget_deployed_topology"));
    }
    Ok(settings)
}

/// The Rust read pool: `READ_ADMISSION_SLOTS` plus one readiness connection.
pub fn rust_pool(values: &Bindings) -> Result<u64, Refusal> {
    let refused = Refusal("read_admission_slots");
    let slots = match values.get("READ_ADMISSION_SLOTS") {
        None => DEFAULT_READ_ADMISSION_SLOTS,
        Some(None) => return Err(refused),
        Some(Some(raw)) => {
            let trimmed = raw.trim_matches(is_python_space);
            if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(refused);
            }
            trimmed.parse().map_err(|_| refused)?
        }
    };
    slots.checked_add(1).ok_or(refused)
}

/// `derive_budget`: Python's configured upper bound, the Rust pool and the
/// optional dedicated listener connections, as artifact fields.
pub fn derive(
    settings: &Settings,
    rust_pool: u64,
    cache_listener: u64,
    stream_listener: u64,
) -> Result<BTreeMap<&'static str, u64>, Refusal> {
    let refused = Refusal("budget_inputs");
    if !(2..=MAX_COUNT).contains(&rust_pool)
        || cache_listener > 1
        || stream_listener > 1
        || (stream_listener == 1 && settings.get("workers") < MIN_WORKERS_FOR_STREAM_LISTENER)
    {
        return Err(refused);
    }
    let python_budget = settings.get("services")
        * settings.get("workers")
        * (settings.get("pool") + settings.get("overflow") + 1)
        + settings.get("reserve");
    let total = python_budget + rust_pool + cache_listener + stream_listener;
    if total > settings.get("configured_max_connections").min(MAX_COUNT) {
        return Err(Refusal("budget_exceeds_maximum"));
    }
    let mut budget = settings.0.clone();
    budget.insert("rust_pool", rust_pool);
    budget.insert("python_budget", python_budget);
    budget.insert("total", total);
    // Absent when zero, so artifacts without a listener stay byte-identical.
    if cache_listener == 1 {
        budget.insert("cache_listener", 1);
    }
    if stream_listener == 1 {
        budget.insert("stream_listener", 1);
    }
    Ok(budget)
}

/// The artifact as Python's `json.dump(sort_keys=True)` writes it, with the
/// trailing newline.
fn artifact(
    environment_bytes: &[u8],
    values: &Bindings,
    environment: Environment,
    release_sha: &str,
    cache_listener: u64,
    stream_listener: u64,
) -> Result<serde_json::Map<String, serde_json::Value>, Refusal> {
    if release_sha.bytes().all(|byte| byte == b'0') {
        return Err(Refusal("release_sha"));
    }
    require_identity(values, environment, release_sha)?;
    let settings = configured_settings(values)?;
    let budget = derive(
        &settings,
        rust_pool(values)?,
        cache_listener,
        stream_listener,
    )?;
    let digest: String = sha2::Sha256::digest(environment_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut fields: BTreeMap<String, serde_json::Value> = budget
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.into()))
        .collect();
    fields.insert("version".to_owned(), 1.into());
    fields.insert("release_sha".to_owned(), release_sha.into());
    fields.insert("environment_digest".to_owned(), digest.into());
    Ok(fields.into_iter().collect())
}

/// `json.dumps(..., sort_keys=True)`: `", "` and `": "` separators. Every
/// value here is an integer or a lowercase hexadecimal string.
fn python_json(fields: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut sorted: Vec<(&String, &serde_json::Value)> = fields.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    let body: Vec<String> = sorted
        .into_iter()
        .map(|(key, value)| format!("{}: {value}", serde_json::Value::from(key.as_str())))
        .collect();
    format!("{{{}}}\n", body.join(", "))
}

/// Run `ops prepare-budget` (prepare, or verify with `--verify`).
pub fn run(args: &BudgetArgs) -> Result<(), (Exit, &'static str)> {
    let environment = args.identity.environment;
    let release_sha = args.identity.release_sha.as_str();
    let refused = |refusal: Refusal| (Exit::Precondition, refusal.0);
    let environment_bytes = state::read_regular_bytes(&args.environment_file, MAX_ENV_BYTES)
        .map_err(|exit| (exit, "environment_file"))?;
    let text = std::str::from_utf8(&environment_bytes)
        .map_err(|_| (Exit::Precondition, "environment_file"))?;
    let values =
        super::dotenv::parse(text).map_err(|_| (Exit::Precondition, "environment_file"))?;
    let output = args.output.as_path();
    if args.verify {
        let recorded_bytes = state::read_regular_bytes(output, MAX_ARTIFACT_BYTES)
            .map_err(|exit| (exit, "artifact"))?;
        let recorded: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&recorded_bytes)
                .map_err(|_| (Exit::Precondition, "artifact"))?;
        // The listener choice was made under the deploy lock when the artifact
        // was prepared; everything else is recomputed from the inputs.
        let listener = |key: &str| match recorded.get(key) {
            None => Ok(0),
            Some(value) => value
                .as_u64()
                .filter(|value| *value <= 1)
                .ok_or((Exit::Precondition, "artifact")),
        };
        let expected = artifact(
            &environment_bytes,
            &values,
            environment,
            release_sha,
            listener("cache_listener")?,
            listener("stream_listener")?,
        )
        .map_err(refused)?;
        return if recorded == expected {
            Ok(())
        } else {
            Err((Exit::Precondition, "budget_changed"))
        };
    }
    let fields = artifact(
        &environment_bytes,
        &values,
        environment,
        release_sha,
        u64::from(args.cache_listener),
        u64::from(args.stream_listener),
    )
    .map_err(refused)?;
    publish(output, python_json(&fields).as_bytes())
}

/// Publish the artifact by atomic rename in a real (not symlinked) directory,
/// over nothing or a regular file.
fn publish(output: &Path, document: &[u8]) -> Result<(), (Exit, &'static str)> {
    let refused = (Exit::Precondition, "artifact_path");
    let parent = output
        .parent()
        .filter(|_| output.is_absolute())
        .ok_or(refused)?;
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        _ => return Err(refused),
    }
    match std::fs::symlink_metadata(output) {
        Ok(metadata) if !metadata.file_type().is_file() => return Err(refused),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err((Exit::Dependency, "artifact_path")),
    }
    state::write_atomic_bytes(output, document).map_err(|exit| (exit, "write"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_follow_the_pydantic_spellings() {
        for (raw, value) in [
            ("4", 4),
            (" 4 ", 4),
            ("+4", 4),
            ("04", 4),
            ("4.0", 4),
            ("4.00", 4),
            ("-0", 0),
            ("\t7\n", 7),
        ] {
            assert_eq!(settings_integer(raw), Some(value), "{raw:?}");
        }
        for raw in [
            "",
            " ",
            "4_0",
            "0x4",
            "4.5",
            "4.",
            "1e1",
            "4 4",
            "٤",
            "4.0000000001",
        ] {
            assert_eq!(settings_integer(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn artifact_bytes_are_python_json() {
        let fields: serde_json::Map<String, serde_json::Value> = [
            ("workers".to_owned(), 4.into()),
            ("release_sha".to_owned(), "ab".into()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            python_json(&fields),
            "{\"release_sha\": \"ab\", \"workers\": 4}\n"
        );
    }
}
