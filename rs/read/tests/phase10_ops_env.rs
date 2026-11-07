//! Phase 10 Task 9: purpose-bound runtime environments.
//!
//! `ops prepare-runtime` projects the deployed `.env` into one private file per
//! purpose (`runtime.env` for `serve`, `jobs/<name>.env` for each job), with
//! exactly the keys of `deploy/rust/runtime-policy.env`. `ops prepare-budget`
//! writes the connection budget artifact and `ops preflight` checks the
//! deployed identity. The Rust projection replaces
//! `scripts/prepare_rust_runtime.py` and `scripts/prepare_rust_budget.py`, so
//! the parity cases run both on the same fixtures and compare the bytes.
//!
//! requirements installed, as in the other Rust-launched Python checks.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::Utc;
use sha2::Digest;
use uuid::Uuid;
use verdaxis_read::auth::{
    TokenIssuer, TokenType, VerificationSettings, verify_access, verify_token,
};
use verdaxis_read::runtime::config::config_from_deployed_environment;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const STAGING_REVISION: &str = "sqlx_1";
const PRODUCTION_REVISION: &str = "sqlx_1";
const PRECONDITION: i32 = 3;
const DEPENDENCY: i32 = 4;

// Values that must never reach a purpose that does not read them. Each is
// unique, so a search of the written bytes finds any leak.
const MIGRATOR_PASSWORD: &str = "MIGRATOR-SENTINEL-pw";
const RESEND_KEY: &str = "re_RESEND-SENTINEL";
const JWT_SECRET: &str = "jwt$HOME-current-secret-0123456789abcdef";
const GEMINI_KEY: &str = "GEMINI-SENTINEL";
const WEBZ_TOKEN: &str = "WEBZ-SENTINEL";
const ADMIN_PASSWORD: &str = "ADMIN-SENTINEL";
const TELEGRAM_TOKEN: &str = "TELEGRAM-SENTINEL";
const MONITOR_TOKEN: &str = "MONITOR-SENTINEL token";

// ---- harness ------------------------------------------------------------------------

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn python() -> String {
    std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".to_owned())
}

/// A private scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-ops-env-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        // The canonical form, as the projection records it (macOS /var -> /private/var).
        Self(path.canonicalize().unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One deployed `.env` fixture: lines in order, written exactly as given.
#[derive(Clone)]
struct EnvFile(Vec<String>);

impl EnvFile {
    fn staging() -> Self {
        Self(
            [
                "ENVIRONMENT=staging".to_owned(),
                format!("RELEASE_SHA={SHA}"),
                "DATABASE_URL=postgresql+asyncpg://verdaxis_app_staging:app%24pw@127.0.0.1:5432/verdaxis_staging".to_owned(),
                "DATABASE_NAME=verdaxis_staging".to_owned(),
                "DATABASE_USER=verdaxis_app_staging".to_owned(),
                format!("MIGRATOR_DATABASE_URL=postgresql+asyncpg://verdaxis_migrator_staging:{MIGRATOR_PASSWORD}@127.0.0.1:5432/verdaxis_staging"),
                format!("JWT_SECRET={JWT_SECRET}"),
                "JWT_ISSUER=verdaxis-staging".to_owned(),
                "JWT_AUDIENCE=verdaxis-staging-api".to_owned(),
                "FRONTEND_URL=https://staging.verdaxis.exchange".to_owned(),
                "EMAIL_FROM=\"Verdaxis <noreply@verdaxis.exchange>\"".to_owned(),
                format!("RESEND_API_KEY={RESEND_KEY}"),
                "KYC_EXECUTION_GATE_ENABLED=false".to_owned(),
            ]
            .into(),
        )
    }

    fn production() -> Self {
        Self::staging()
            .set("ENVIRONMENT", "production")
            .set(
                "DATABASE_URL",
                "postgresql://verdaxis_app:prod%2Fpw@127.0.0.1:5432/verdaxis_prod",
            )
            .set("DATABASE_NAME", "verdaxis_prod")
            .set("DATABASE_USER", "verdaxis_app")
            .set(
                "MIGRATOR_DATABASE_URL",
                &format!(
                    "postgresql+asyncpg://verdaxis_migrator:{MIGRATOR_PASSWORD}@127.0.0.1:5432/verdaxis_prod"
                ),
            )
    }

    /// Every optional serve key, the keys other purposes read, and the
    /// python-dotenv spellings the deployed files may use.
    fn staging_full() -> Self {
        Self::staging().with(&[
            "export JWT_SECRET_PREVIOUS='previous-secret-for-fixture-0123456789\\'s'",
            &format!("MONITOR_TOKEN={MONITOR_TOKEN}   # an inline comment"),
            "SELLER_FEE_PILOT_PER_MT_USD=2.5",
            "SELLER_FEE_PROFESSIONAL_PER_MT_USD=",
            "READ_ADMISSION_SLOTS=24",
            "SYSTEM_STATUS= Degraded ",
            "SYSTEM_STATUS_MESSAGE=\"Planned \\\"maintenance\\\" at 02:00 # not a comment\"",
            "AUTO_MATCHING_ENABLED=No",
            "MARKET_SUPPORT_ENABLED=yes",
            "MARKET_SUPPORT_MAX_TTL_HOURS= 048",
            "MARKET_SUPPORT_CONTEXT_TTL_MINUTES=30",
            "MARKET_SUPPORT_CONSENT_VERSION=v2026-11",
            "MARKET_SUPPORT_CONSENT_REFERENCE='market-support/2026-11'",
            "MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS=",
            "UMAMI_BASE_URL=https://umami.example",
            "UMAMI_WEBSITE_ID=site-from-env",
            &format!("GEMINI_API_KEY={GEMINI_KEY}"),
            &format!("WEBZ_API_TOKEN={WEBZ_TOKEN}"),
            "WEBZ_API_URL=https://webz.example/api",
            "WEBZ_API_TIMEOUT_SECONDS=20",
            "KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED=I_UNDERSTAND",
            &format!("ADMIN_PASSWORD={ADMIN_PASSWORD}"),
            &format!("TELEGRAM_BOT_TOKEN={TELEGRAM_TOKEN}"),
            "# a comment line",
            "",
            "KEY_WITHOUT_VALUE",
        ])
    }

    fn with(mut self, lines: &[&str]) -> Self {
        self.0.extend(lines.iter().map(|line| (*line).to_owned()));
        self
    }

    /// Replace (or append) `key`, keeping the line order.
    fn set(mut self, key: &str, value: &str) -> Self {
        let line = format!("{key}={value}");
        match self
            .0
            .iter()
            .position(|old| old.starts_with(&format!("{key}=")))
        {
            Some(index) => self.0[index] = line,
            None => self.0.push(line),
        }
        self
    }

    fn without(mut self, key: &str) -> Self {
        self.0.retain(|line| !line.starts_with(&format!("{key}=")));
        self
    }

    /// Every database URL on loopback port 1, which is closed: a command that
    /// tries to connect fails there, so a command that must not connect is
    /// seen not to.
    fn on_closed_port(self) -> Self {
        Self(
            self.0
                .into_iter()
                .map(|line| line.replace("127.0.0.1:5432", "127.0.0.1:1"))
                .collect(),
        )
    }

    fn write(&self, directory: &Path) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        let path = directory.join(".env");
        std::fs::write(&path, self.0.join("\n") + "\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}

fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"));
    command.env_clear().env("PATH", "/usr/bin:/bin");
    command
}

fn rust_prepare_runtime(
    environment: &str,
    environment_file: &Path,
    output: &Path,
    revision: &str,
) -> Output {
    binary()
        .args(["ops", "prepare-runtime", "--environment", environment])
        .args(["--release-sha", SHA, "--migration-revision", revision])
        .arg("--environment-file")
        .arg(environment_file)
        .arg("--output-dir")
        .arg(output)
        .output()
        .unwrap()
}

fn python_prepare_runtime(
    environment: &str,
    environment_file: &Path,
    output: &Path,
    revision: &str,
) -> Output {
    let root = repository_root();
    Command::new(python())
        .arg("-I")
        .arg(root.join("scripts/prepare_rust_runtime.py"))
        .arg("--environment-file")
        .arg(environment_file)
        .args(["--release-sha", SHA, "--migration-revision", revision])
        .args(["--environment", environment])
        .arg("--output-dir")
        .arg(output)
        .arg("--source-root")
        .arg(&root)
        .output()
        .unwrap()
}

fn rust_prepare_budget(environment: &str, environment_file: &Path, extra: &[&str]) -> Output {
    binary()
        .args(["ops", "prepare-budget", "--environment", environment])
        .args(["--release-sha", SHA])
        .arg("--environment-file")
        .arg(environment_file)
        .args(extra)
        .output()
        .unwrap()
}

/// Parse a written file as systemd's `EnvironmentFile=` reads the renderer's
/// two forms: `KEY=plain` and `KEY="escaped"` with `\\` and `\"`.
fn read_environment_file(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("KEY=value");
            let value = match value
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
            {
                Some(quoted) => quoted.replace("\\\"", "\"").replace("\\\\", "\\"),
                None => value.to_owned(),
            };
            (key.to_owned(), value)
        })
        .collect()
}

/// The keys `runtime-policy.env` grants one delivery file.
fn policy_keys(delivery: &str) -> BTreeSet<String> {
    let policy =
        std::fs::read_to_string(repository_root().join("deploy/rust/runtime-policy.env")).unwrap();
    policy
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|fields| fields.len() == 4 && fields[3] == delivery)
        .map(|fields| fields[1].to_owned())
        .collect()
}

/// Every regular file under `directory`, by relative path.
fn written_files(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(directory).unwrap().display().to_string();
                files.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn strs(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

/// Decode a refresh token as the refresh route does: HS256 under the current
/// key, then the previous key. Its own claims are checked by the caller.
fn refresh_claims(token: &str, current: &str, previous: Option<&str>) -> serde_json::Value {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_aud = false;
    std::iter::once(current)
        .chain(previous)
        .find_map(|key| {
            jsonwebtoken::decode::<serde_json::Value>(
                token,
                &jsonwebtoken::DecodingKey::from_secret(key.as_bytes()),
                &validation,
            )
            .ok()
        })
        .expect("the refresh token verifies under the projected keys")
        .claims
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn assert_ok(output: &Output, what: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{what}: stdout {} stderr {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Prepare a full staging projection and return the scratch area and the
/// output directory.
fn prepared_staging_full() -> (Scratch, PathBuf) {
    let scratch = Scratch::new();
    let environment_file = EnvFile::staging_full().write(&scratch.path("be"));
    // The deployed unit's second file supplies the Umami reader credentials.
    std::fs::write(
        scratch.path("be/.env.analytics"),
        "UMAMI_API_USERNAME=reader\nUMAMI_API_PASSWORD=analytics-password\n",
    )
    .unwrap();
    let output = scratch.path("be/.runtime-rust");
    let result = rust_prepare_runtime("staging", &environment_file, &output, STAGING_REVISION);
    assert_ok(&result, "rust prepare-runtime");
    (scratch, output)
}

// ---- serve ---------------------------------------------------------------------------

#[test]
fn serve_env_requires_auth_keys_and_issues_and_verifies_sessions() {
    let (scratch, output) = prepared_staging_full();
    let budget = scratch.path("be/budget.json");
    let environment_file = scratch.path("be/.env");
    assert_ok(
        &rust_prepare_budget(
            "staging",
            &environment_file,
            &["--output", budget.to_str().unwrap()],
        ),
        "rust prepare-budget",
    );

    // The serve loader accepts exactly the projected file and the budget.
    let projected: HashMap<String, String> = read_environment_file(&output.join("runtime.env"))
        .into_iter()
        .collect();
    let config = config_from_deployed_environment(
        &projected,
        "127.0.0.1:8011".parse().unwrap(),
        &std::fs::read_to_string(&budget).unwrap(),
    )
    .expect("the projected serve environment loads");
    let phase2 = &config.phase2;
    let (secret, issuer_name, audience) = (
        phase2.jwt_secret.clone().unwrap(),
        phase2.jwt_issuer.clone().unwrap(),
        phase2.jwt_audience.clone().unwrap(),
    );
    assert_eq!(
        secret, JWT_SECRET,
        "no shell expansion of the deployed value"
    );
    assert_eq!(
        phase2.monitor_token.as_deref(),
        Some(MONITOR_TOKEN),
        "the inline comment is not part of the value"
    );

    // A session issued with the projected authority verifies with it: an
    // access, refresh and stream token, under the current and previous keys.
    let verification = VerificationSettings::new(
        secret.clone(),
        phase2.jwt_secret_previous.clone(),
        issuer_name.clone(),
        audience.clone(),
    );
    let now = Utc::now();
    let subject = Uuid::new_v4();
    for key in [
        secret.as_str(),
        phase2.jwt_secret_previous.as_deref().unwrap(),
    ] {
        let issuer = TokenIssuer::new(key, &issuer_name, &audience);
        let access = issuer.access(subject, now, None);
        assert_eq!(
            verify_access(&access, &verification, now).unwrap().subject,
            subject
        );
        let refresh = issuer.refresh(subject, Uuid::new_v4(), &"j".repeat(43), now, None);
        let claims = refresh_claims(&refresh, &secret, phase2.jwt_secret_previous.as_deref());
        assert_eq!(claims["type"], "refresh");
        assert_eq!(claims["sub"], subject.to_string());
        assert_eq!(
            (claims["iss"].as_str(), claims["aud"].as_str()),
            (Some(issuer_name.as_str()), Some(audience.as_str()))
        );
        let stream = issuer.stream(subject, Uuid::new_v4(), "staging", now);
        let verified = verify_token(&stream, &verification, TokenType::Stream, now).unwrap();
        assert_eq!(verified.subject(), subject);
    }

    // Without any one auth key the projection refuses and writes nothing.
    for key in [
        "JWT_SECRET",
        "JWT_ISSUER",
        "JWT_AUDIENCE",
        "FRONTEND_URL",
        "EMAIL_FROM",
        "RESEND_API_KEY",
    ] {
        let scratch = Scratch::new();
        let environment_file = EnvFile::staging().without(key).write(&scratch.path("be"));
        let output = scratch.path("be/.runtime-rust");
        let result = rust_prepare_runtime("staging", &environment_file, &output, STAGING_REVISION);
        assert_eq!(result.status.code(), Some(PRECONDITION), "missing {key}");
        assert!(
            !output.join("runtime.env").exists(),
            "missing {key}: nothing written"
        );
    }
}

#[test]
fn serve_env_contains_only_reviewed_http_credentials() {
    let (_scratch, output) = prepared_staging_full();
    let projected = read_environment_file(&output.join("runtime.env"));
    let keys: BTreeSet<String> = projected.keys().cloned().collect();
    assert_eq!(
        keys,
        policy_keys("runtime.env"),
        "serve keys are the reviewed policy rows"
    );
    // The KYC key ring is a systemd credential, never an environment key.
    assert!(
        !keys
            .iter()
            .any(|key| key.to_ascii_lowercase().contains("kyc-document"))
    );
    assert_eq!(
        policy_keys("credential:kyc-document-keys"),
        BTreeSet::from(["kyc-document-keys".to_owned()])
    );
}

/// Phase 10 Task 19: the signup-canary route reads `MONITOR_TOKEN` from the
/// serve file. It is projected only when the deployed `.env` sets it to a
/// nonblank value; no other purpose receives it.
#[test]
fn serve_env_carries_the_monitor_token_only_when_configured() {
    let (_scratch, output) = prepared_staging_full();
    let projected = read_environment_file(&output.join("runtime.env"));
    assert_eq!(
        projected.get("MONITOR_TOKEN").map(String::as_str),
        Some(MONITOR_TOKEN)
    );
    for job in std::fs::read_dir(output.join("jobs")).unwrap() {
        let job = read_environment_file(&job.unwrap().path());
        assert!(!job.contains_key("MONITOR_TOKEN"));
    }
    for (label, file) in [
        ("absent", EnvFile::staging()),
        ("blank", EnvFile::staging().with(&["MONITOR_TOKEN="])),
    ] {
        let scratch = Scratch::new();
        let environment_file = file.write(&scratch.path("be"));
        let output = scratch.path("be/.runtime-rust");
        let result = rust_prepare_runtime("staging", &environment_file, &output, STAGING_REVISION);
        assert_ok(&result, label);
        let projected = read_environment_file(&output.join("runtime.env"));
        assert!(!projected.contains_key("MONITOR_TOKEN"), "{label}");
    }
}

#[test]
fn serve_env_has_no_migrator_or_job_only_key() {
    let (_scratch, output) = prepared_staging_full();
    for name in ["runtime.env", "auth-domains.json"] {
        let bytes = std::fs::read(output.join(name)).unwrap();
        for secret in [
            MIGRATOR_PASSWORD,
            GEMINI_KEY,
            WEBZ_TOKEN,
            ADMIN_PASSWORD,
            TELEGRAM_TOKEN,
        ] {
            assert!(!contains(&bytes, secret), "{name} leaks {secret}");
        }
        for key in [
            "MIGRATOR_DATABASE_URL",
            "GEMINI_API_KEY",
            "WEBZ_API",
            "KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED",
            "TELEGRAM",
            "ADMIN_PASSWORD",
        ] {
            assert!(!contains(&bytes, key), "{name} names {key}");
        }
    }
}

#[test]
fn serve_projection_matches_python_byte_for_byte() {
    let cases = [
        (
            "staging minimal",
            "staging",
            EnvFile::staging(),
            STAGING_REVISION,
        ),
        (
            "staging full",
            "staging",
            EnvFile::staging_full(),
            STAGING_REVISION,
        ),
        (
            "production",
            "production",
            EnvFile::production(),
            PRODUCTION_REVISION,
        ),
    ];
    for (name, environment, fixture, revision) in cases {
        let scratch = Scratch::new();
        let environment_file = fixture.write(&scratch.path("be"));
        // The deployed Python unit's second file: its Umami keys win.
        std::fs::write(
            scratch.path("be/.env.analytics"),
            "UMAMI_WEBSITE_ID=site-from-analytics\nUMAMI_API_USERNAME=reader\nUMAMI_API_PASSWORD=p$HOME\\pw\n",
        )
        .unwrap();
        let (rust_dir, python_dir) = (scratch.path("rust"), scratch.path("python"));
        assert_ok(
            &python_prepare_runtime(environment, &environment_file, &python_dir, revision),
            name,
        );
        assert_ok(
            &rust_prepare_runtime(environment, &environment_file, &rust_dir, revision),
            name,
        );
        // The one expected difference: each names its own datasets file.
        let rust_env = String::from_utf8(std::fs::read(rust_dir.join("runtime.env")).unwrap())
            .unwrap()
            .replace(rust_dir.to_str().unwrap(), python_dir.to_str().unwrap());
        assert_eq!(
            rust_env,
            std::fs::read_to_string(python_dir.join("runtime.env")).unwrap(),
            "{name}: runtime.env"
        );
        assert_eq!(
            std::fs::read(rust_dir.join("auth-domains.json")).unwrap(),
            std::fs::read(python_dir.join("auth-domains.json")).unwrap(),
            "{name}: auth-domains.json"
        );
    }
}

#[test]
fn serve_projection_refuses_what_python_refuses() {
    let base = EnvFile::staging;
    let cases: Vec<(&str, EnvFile, &str)> = vec![
        (
            "short JWT secret",
            base().set("JWT_SECRET", "short"),
            STAGING_REVISION,
        ),
        (
            "short previous secret",
            base().set("JWT_SECRET_PREVIOUS", "short"),
            STAGING_REVISION,
        ),
        (
            "blank issuer",
            base().set("JWT_ISSUER", "  "),
            STAGING_REVISION,
        ),
        (
            "relative frontend",
            base().set("FRONTEND_URL", "staging.verdaxis.exchange"),
            STAGING_REVISION,
        ),
        (
            "wrong environment",
            base().set("ENVIRONMENT", "production"),
            STAGING_REVISION,
        ),
        (
            "wrong release",
            base().set("RELEASE_SHA", &"f".repeat(40)),
            STAGING_REVISION,
        ),
        (
            "production role in staging",
            base().set(
                "DATABASE_URL",
                "postgresql://verdaxis_app:pw@127.0.0.1/verdaxis_staging",
            ),
            STAGING_REVISION,
        ),
        (
            "wrong database",
            base().set(
                "DATABASE_URL",
                "postgresql://verdaxis_app_staging:pw@127.0.0.1/verdaxis_prod",
            ),
            STAGING_REVISION,
        ),
        (
            "query in database URL",
            base().set(
                "DATABASE_URL",
                "postgresql://verdaxis_app_staging:pw@127.0.0.1/verdaxis_staging?sslmode=disable",
            ),
            STAGING_REVISION,
        ),
        (
            "unknown status",
            base().set("SYSTEM_STATUS", "down"),
            STAGING_REVISION,
        ),
        (
            "long message",
            base().set("SYSTEM_STATUS_MESSAGE", &"m".repeat(281)),
            STAGING_REVISION,
        ),
        (
            "fee precision",
            base().set("SELLER_FEE_PILOT_PER_MT_USD", "2.505"),
            STAGING_REVISION,
        ),
        (
            "negative fee",
            base().set("SELLER_FEE_PROFESSIONAL_PER_MT_USD", "-1"),
            STAGING_REVISION,
        ),
        (
            "bad boolean",
            base().set("KYC_EXECUTION_GATE_ENABLED", "maybe"),
            STAGING_REVISION,
        ),
        (
            "ttl bound",
            base().set("MARKET_SUPPORT_MAX_TTL_HOURS", "721"),
            STAGING_REVISION,
        ),
        (
            "default consent",
            base().set("MARKET_SUPPORT_ENABLED", "true"),
            STAGING_REVISION,
        ),
        (
            "control character",
            base().set("EMAIL_FROM", "\"a\\tb\""),
            STAGING_REVISION,
        ),
    ];
    let staging_cases = cases
        .into_iter()
        .map(|(name, fixture, revision)| (name, "staging", fixture, revision));
    let production_case = (
        "unreviewed production revision",
        "production",
        EnvFile::production(),
        "p0_20260101_other",
    );
    for (name, environment, fixture, revision) in
        staging_cases.chain(std::iter::once(production_case))
    {
        let scratch = Scratch::new();
        let environment_file = fixture.write(&scratch.path("be"));
        let python = python_prepare_runtime(
            environment,
            &environment_file,
            &scratch.path("python"),
            revision,
        );
        assert_ne!(python.status.code(), Some(0), "{name}: Python refuses");
        let output = scratch.path("rust");
        let rust = rust_prepare_runtime(environment, &environment_file, &output, revision);
        assert_eq!(
            rust.status.code(),
            Some(PRECONDITION),
            "{name}: Rust refuses"
        );
        assert!(
            !output.join("runtime.env").exists(),
            "{name}: nothing published"
        );
        for stream in [&rust.stdout, &rust.stderr] {
            assert!(
                !contains(stream, JWT_SECRET) && !contains(stream, RESEND_KEY),
                "{name}: no secret in output"
            );
        }
    }
}

#[test]
fn vendored_auth_domain_datasets_match_manifest() {
    let root = repository_root();
    let datasets = std::fs::read(root.join("deploy/rust/auth-domain-datasets.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("deploy/rust/auth-domain-datasets.manifest.json")).unwrap(),
    )
    .unwrap();
    let digest: String = sha2::Sha256::digest(&datasets)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(manifest["sha256"].as_str(), Some(digest.as_str()));
    let parsed: serde_json::Value = serde_json::from_slice(&datasets).unwrap();
    for list in ["public", "disposable"] {
        let values: Vec<&str> = parsed[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            manifest["counts"][list].as_u64(),
            Some(values.len() as u64),
            "{list} count"
        );
        assert!(
            values.windows(2).all(|pair| pair[0] < pair[1]),
            "{list} sorted and unique"
        );
    }
    // The generator reproduces the committed bytes from the pinned packages.
    let check = Command::new(python())
        .arg("-I")
        .arg(root.join("deploy/scripts/generate-auth-domain-datasets.py"))
        .arg("--check")
        .output()
        .unwrap();
    assert_ok(&check, "generator --check");
}

// ---- jobs ----------------------------------------------------------------------------

#[test]
fn news_and_prune_envs_have_no_mail_jwt_or_migrator_key() {
    let (_scratch, output) = prepared_staging_full();
    for name in ["news", "prune-analytics"] {
        let delivery = format!("jobs/{name}.env");
        let path = output.join(&delivery);
        let projected = read_environment_file(&path);
        let keys: BTreeSet<String> = projected.keys().cloned().collect();
        assert_eq!(keys, policy_keys(&delivery), "{name} keys");
        assert_eq!(
            projected["DATABASE_URL"],
            "postgresql://verdaxis_app_staging:app%24pw@127.0.0.1:5432/verdaxis_staging",
            "{name}: the app role in libpq form"
        );
        let bytes = std::fs::read(&path).unwrap();
        for secret in [
            MIGRATOR_PASSWORD,
            RESEND_KEY,
            JWT_SECRET,
            ADMIN_PASSWORD,
            TELEGRAM_TOKEN,
        ] {
            assert!(!contains(&bytes, secret), "{name} leaks {secret}");
        }
        for key in ["RESEND", "EMAIL_FROM", "JWT", "MIGRATOR"] {
            assert!(!contains(&bytes, key), "{name} names {key}");
        }
    }
    let news = read_environment_file(&output.join("jobs/news.env"));
    assert_eq!(news["GEMINI_API_KEY"], GEMINI_KEY);
    assert_eq!(news["WEBZ_API_TOKEN"], WEBZ_TOKEN);
    let prune = read_environment_file(&output.join("jobs/prune-analytics.env"));
    assert!(!prune.contains_key("GEMINI_API_KEY") && !prune.contains_key("WEBZ_API_TOKEN"));
}

#[test]
fn auth_and_reminder_envs_have_mail_but_no_jwt_signing_key() {
    let (_scratch, output) = prepared_staging_full();
    for name in ["auth-maintenance", "reminders", "kyc-notices"] {
        let delivery = format!("jobs/{name}.env");
        let path = output.join(&delivery);
        let projected = read_environment_file(&path);
        let keys: BTreeSet<String> = projected.keys().cloned().collect();
        assert_eq!(keys, policy_keys(&delivery), "{name} keys");
        assert_eq!(projected["RESEND_API_KEY"], RESEND_KEY, "{name} sends mail");
        let bytes = std::fs::read(&path).unwrap();
        for secret in [JWT_SECRET, MIGRATOR_PASSWORD, GEMINI_KEY, WEBZ_TOKEN] {
            assert!(!contains(&bytes, secret), "{name} leaks {secret}");
        }
        assert!(!contains(&bytes, "JWT"), "{name} names a JWT key");
    }
    let reminders = read_environment_file(&output.join("jobs/reminders.env"));
    assert_eq!(
        reminders["EMAIL_FROM"],
        "Verdaxis <noreply@verdaxis.exchange>"
    );
    assert_eq!(
        reminders["FRONTEND_URL"],
        "https://staging.verdaxis.exchange"
    );
    // A job file is refused as a whole when one of its required keys is absent.
    let scratch = Scratch::new();
    let fixture = EnvFile::staging().without("RESEND_API_KEY");
    let output = scratch.path("be/.runtime-rust");
    let result = rust_prepare_runtime(
        "staging",
        &fixture.write(&scratch.path("be")),
        &output,
        STAGING_REVISION,
    );
    assert_eq!(result.status.code(), Some(PRECONDITION));
    assert!(!output.join("jobs").exists(), "no partial job files");
}

#[test]
fn migrator_url_is_only_in_the_migrate_invocation() {
    let (_scratch, output) = prepared_staging_full();
    let files = written_files(&output);
    let expected: BTreeSet<String> = [
        "runtime.env",
        "auth-domains.json",
        "jobs/news.env",
        "jobs/auth-maintenance.env",
        "jobs/reminders.env",
        "jobs/kyc-notices.env",
        "jobs/prune-analytics.env",
        "jobs/demo.env",
        "jobs/monitor-onboarding.env",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        files.keys().cloned().collect::<BTreeSet<_>>(),
        expected,
        "one file per purpose, no migrate file"
    );
    for (name, bytes) in &files {
        assert!(
            !contains(bytes, MIGRATOR_PASSWORD),
            "{name} carries the migrator password"
        );
        assert!(
            !contains(bytes, "verdaxis_migrator"),
            "{name} names the migrator role"
        );
        assert!(!contains(bytes, "MIGRATOR"), "{name} names a migrator key");
    }
    // The policy grants the migrator URL to the migrate invocation and nothing else.
    let policy =
        std::fs::read_to_string(repository_root().join("deploy/rust/runtime-policy.env")).unwrap();
    let grants: Vec<&str> = policy
        .lines()
        .filter(|line| !line.starts_with('#') && line.contains("MIGRATOR"))
        .collect();
    assert_eq!(
        grants,
        ["migrate\tMIGRATOR_DATABASE_URL\trequired\tinvocation"]
    );
}

// ---- budget --------------------------------------------------------------------------

#[test]
fn budget_matches_configured_authority_not_measured_load() {
    let staging = EnvFile::staging;
    let accepted: Vec<(&str, EnvFile, &[&str])> = vec![
        ("defaults (every budget key absent)", staging(), &[]),
        (
            "Python integer spellings",
            staging()
                .set("UVICORN_WORKERS", " +2 ")
                .set("DB_POOL_SIZE", "3.0")
                .set("DB_MAX_OVERFLOW", "-0")
                .set("DB_MAX_CONNECTIONS", "0200")
                .set("READ_ADMISSION_SLOTS", "8"),
            &[],
        ),
        (
            "both listeners",
            staging(),
            &["--cache-listener", "1", "--stream-listener", "1"],
        ),
    ];
    for (name, fixture, extra) in accepted {
        let scratch = Scratch::new();
        // The database is unreachable: the budget is configured authority, so
        // it never connects and never reads the server's measured load.
        let environment_file = fixture.on_closed_port().write(&scratch.path("be"));
        let rust_out = scratch.path("rust.json");
        let mut rust_arguments = vec!["--output", rust_out.to_str().unwrap()];
        rust_arguments.extend_from_slice(extra);
        assert_ok(
            &rust_prepare_budget("staging", &environment_file, &rust_arguments),
            name,
        );
        let mut expected = match name {
            "Python integer spellings" => serde_json::json!({
                "version": 1, "release_sha": SHA, "services": 2, "workers": 2,
                "pool": 3, "overflow": 0, "reserve": 20,
                "configured_max_connections": 200, "rust_pool": 9,
                "python_budget": 36, "total": 45
            }),
            _ => serde_json::json!({
                "version": 1, "release_sha": SHA, "services": 2, "workers": 4,
                "pool": 2, "overflow": 1, "reserve": 20,
                "configured_max_connections": 100, "rust_pool": 17,
                "python_budget": 52, "total": 69
            }),
        };
        if name == "both listeners" {
            expected["cache_listener"] = serde_json::json!(1);
            expected["stream_listener"] = serde_json::json!(1);
            expected["total"] = serde_json::json!(71);
        }
        use sha2::Digest;
        let digest: String = sha2::Sha256::digest(std::fs::read(&environment_file).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        expected["environment_digest"] = serde_json::json!(digest);
        let artifact = std::fs::read_to_string(&rust_out).unwrap();
        let actual: serde_json::Value = serde_json::from_str(&artifact).unwrap();
        assert_eq!(actual, expected, "{name}: fixed artifact fields");
        assert!(artifact.ends_with('\n'), "{name}: artifact newline");
        // Native verification accepts its artifact and refuses an edited .env.
        let verify = |path: &Path| {
            vec![
                "--output".to_owned(),
                path.display().to_string(),
                "--verify".to_owned(),
            ]
        };
        let mut rust_verify = verify(&rust_out);
        rust_verify.extend(extra.iter().map(|argument| (*argument).to_owned()));
        assert_ok(
            &rust_prepare_budget("staging", &environment_file, &strs(&rust_verify)),
            name,
        );
        std::fs::write(
            &environment_file,
            std::fs::read_to_string(&environment_file).unwrap() + "EXTRA=1\n",
        )
        .unwrap();
        let edited = rust_prepare_budget("staging", &environment_file, &strs(&rust_verify));
        assert_eq!(
            edited.status.code(),
            Some(PRECONDITION),
            "{name}: changed .env"
        );
    }

    let refused: Vec<(&str, EnvFile, &[&str])> = vec![
        ("capacity", staging().set("DB_MAX_OVERFLOW", "100"), &[]),
        (
            "deployed service count",
            staging().set("DB_SERVICE_COUNT", "3"),
            &[],
        ),
        (
            "deployed reserve",
            staging().set("DB_RESERVED_CONNECTIONS", "10"),
            &[],
        ),
        (
            "reserve not below maximum",
            staging().set("DB_RESERVED_CONNECTIONS", "100"),
            &[],
        ),
        ("zero workers", staging().set("UVICORN_WORKERS", "0"), &[]),
        ("fractional pool", staging().set("DB_POOL_SIZE", "4.5"), &[]),
        (
            "underscore digits",
            staging().set("DB_POOL_SIZE", "1_0"),
            &[],
        ),
        ("slots", staging().set("READ_ADMISSION_SLOTS", "x"), &[]),
        (
            "stream listener needs four workers",
            staging().set("UVICORN_WORKERS", "2"),
            &["--stream-listener", "1"],
        ),
        (
            "total over maximum",
            // 52 Python + 17 Rust = 69 connections against a maximum of 68.
            staging().set("DB_MAX_CONNECTIONS", "68"),
            &[],
        ),
    ];
    for (name, fixture, extra) in refused {
        let scratch = Scratch::new();
        let environment_file = fixture.write(&scratch.path("be"));
        let output = scratch.path("budget.json");
        let mut arguments = vec!["--output", output.to_str().unwrap()];
        arguments.extend_from_slice(extra);
        let rust = rust_prepare_budget("staging", &environment_file, &arguments);
        assert_eq!(
            rust.status.code(),
            Some(PRECONDITION),
            "{name}: Rust refuses"
        );
    }
}

// ---- files ---------------------------------------------------------------------------

#[test]
fn runtime_files_are_regular_0600_and_never_symlinks() {
    let (scratch, output) = prepared_staging_full();
    let directories = [output.clone(), output.join("jobs")];
    for directory in &directories {
        let metadata = std::fs::symlink_metadata(directory).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.mode() & 0o777, 0o700, "{}", directory.display());
    }
    for name in written_files(&output).keys() {
        let metadata = std::fs::symlink_metadata(output.join(name)).unwrap();
        assert!(metadata.file_type().is_file(), "{name} is regular");
        assert_eq!(metadata.mode() & 0o777, 0o600, "{name} is 0600");
        assert_eq!(metadata.nlink(), 1, "{name} has one link");
    }
    // A second run replaces every file in place, with no temporary left behind.
    let environment_file = scratch.path("be/.env");
    assert_ok(
        &rust_prepare_runtime("staging", &environment_file, &output, STAGING_REVISION),
        "rerun",
    );
    assert!(
        written_files(&output)
            .keys()
            .all(|name| !name.contains(".tmp"))
    );

    // A symlink at any published path is refused and its target is untouched.
    let decoy = scratch.path("decoy");
    std::fs::write(&decoy, "decoy").unwrap();
    for name in ["runtime.env", "auth-domains.json", "jobs/news.env"] {
        let fresh = Scratch::new();
        let environment_file = EnvFile::staging().write(&fresh.path("be"));
        let output = fresh.path("be/.runtime-rust");
        std::fs::create_dir_all(output.join("jobs")).unwrap();
        symlink(&decoy, output.join(name)).unwrap();
        let result = rust_prepare_runtime("staging", &environment_file, &output, STAGING_REVISION);
        assert_eq!(result.status.code(), Some(PRECONDITION), "symlinked {name}");
        assert_eq!(
            std::fs::read_to_string(&decoy).unwrap(),
            "decoy",
            "{name}: target unchanged"
        );
    }
    for name in [".runtime-rust", ".runtime-rust/jobs"] {
        let fresh = Scratch::new();
        let environment_file = EnvFile::staging().write(&fresh.path("be"));
        let elsewhere = fresh.path("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        if name.ends_with("jobs") {
            std::fs::create_dir(fresh.path("be/.runtime-rust")).unwrap();
        }
        symlink(&elsewhere, fresh.path("be").join(name)).unwrap();
        let result = rust_prepare_runtime(
            "staging",
            &environment_file,
            &fresh.path("be/.runtime-rust"),
            STAGING_REVISION,
        );
        assert_eq!(
            result.status.code(),
            Some(PRECONDITION),
            "symlinked directory {name}"
        );
        assert_eq!(
            std::fs::read_dir(&elsewhere).unwrap().count(),
            0,
            "{name}: nothing written through it"
        );
    }
    // The deployed .env itself must be a regular file, never a link.
    let fresh = Scratch::new();
    let real = EnvFile::staging().write(&fresh.path("real"));
    std::fs::create_dir(fresh.path("be")).unwrap();
    symlink(&real, fresh.path("be/.env")).unwrap();
    let result = rust_prepare_runtime(
        "staging",
        &fresh.path("be/.env"),
        &fresh.path("be/.runtime-rust"),
        STAGING_REVISION,
    );
    assert_eq!(result.status.code(), Some(PRECONDITION), "symlinked .env");
    // The budget artifact path follows the same rule.
    let budget = fresh.path("budget.json");
    symlink(&decoy, &budget).unwrap();
    let result = rust_prepare_budget("staging", &real, &["--output", budget.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(PRECONDITION), "symlinked budget");
    assert_eq!(std::fs::read_to_string(&decoy).unwrap(), "decoy");
}

// ---- preflight -----------------------------------------------------------------------

#[test]
fn preflight_rejects_wrong_database_role_and_environment() {
    let preflight = |environment: &str, fixture: EnvFile| {
        let scratch = Scratch::new();
        // Port 1 on loopback is closed: an identity that passes every file
        // check reaches the connection and fails there (Dependency), so a
        // Precondition exit proves the refusal came before any connection.
        let fixture = fixture.on_closed_port();
        let environment_file = fixture.write(&scratch.path("be"));
        binary()
            .args([
                "ops",
                "preflight",
                "--environment",
                environment,
                "--release-sha",
                SHA,
            ])
            .arg("--environment-file")
            .arg(&environment_file)
            .output()
            .unwrap()
    };
    assert_eq!(
        preflight("staging", EnvFile::staging()).status.code(),
        Some(DEPENDENCY)
    );
    assert_eq!(
        preflight("production", EnvFile::production()).status.code(),
        Some(DEPENDENCY)
    );
    let cases = [
        ("environment", "staging", EnvFile::production()),
        ("environment", "production", EnvFile::staging()),
        (
            "release",
            "staging",
            EnvFile::staging().set("RELEASE_SHA", &"e".repeat(40)),
        ),
        (
            "production role in staging",
            "staging",
            EnvFile::staging().set(
                "DATABASE_URL",
                "postgresql://verdaxis_app:pw@127.0.0.1:5432/verdaxis_staging",
            ),
        ),
        (
            "staging role in production",
            "production",
            EnvFile::production().set(
                "DATABASE_URL",
                "postgresql://verdaxis_app_staging:pw@127.0.0.1:5432/verdaxis_prod",
            ),
        ),
        (
            "migrator role as the app",
            "staging",
            EnvFile::staging().set(
                "DATABASE_URL",
                "postgresql://verdaxis_migrator_staging:pw@127.0.0.1:5432/verdaxis_staging",
            ),
        ),
        (
            "other database",
            "staging",
            EnvFile::staging().set(
                "DATABASE_URL",
                "postgresql://verdaxis_app_staging:pw@127.0.0.1:5432/verdaxis_prod",
            ),
        ),
        (
            "missing database URL",
            "staging",
            EnvFile::staging().set("DATABASE_URL", ""),
        ),
    ];
    for (name, environment, fixture) in cases {
        let output = preflight(environment, fixture);
        assert_eq!(output.status.code(), Some(PRECONDITION), "{name}");
        for stream in [&output.stdout, &output.stderr] {
            assert!(
                !contains(stream, "pw@") && !contains(stream, MIGRATOR_PASSWORD),
                "{name}: no credential in output"
            );
        }
    }
}
