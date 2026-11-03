//! `acl converge`: run the committed runtime ACL convergence as the object
//! owner (Phase 10 Task 7), replacing `scripts/converge_runtime_acls.py`.
//!
//! The deployed `.env` names both database URLs. They must be this
//! environment's app and migrator roles on one database and one endpoint, with
//! distinct roles and credentials, exactly as the Python helper required.
//! `psql` then runs the verified bundle's SQL in one transaction as the
//! migrator. The migrator password reaches only the `psql` child, through its
//! environment; it is never an argument and never printed.
use std::path::{Path, PathBuf};
use std::process::Command;

use super::ops::runtime_env::read_environment_file;
use super::{AclArgs, Environment, Exit};

/// The host's PostgreSQL client. A fixed path: `PATH` never chooses it.
pub const PSQL: &str = "/usr/bin/psql";
const CONVERGENCE_SQL: &str = "deploy/postgres/converge_runtime_object_acls.sql";
const BUNDLE_FILES: [&str; 2] = ["deploy/postgres/app_acl_policy.sql", CONVERGENCE_SQL];
const DEFAULT_PORT: u16 = 5432;

/// The roles of one deployed environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roles {
    pub database_name: &'static str,
    pub app_role: &'static str,
    pub migrator_role: &'static str,
    pub backup_role: &'static str,
}

pub fn roles(environment: Environment) -> Roles {
    match environment {
        Environment::Production => Roles {
            database_name: "verdaxis_prod",
            app_role: "verdaxis_app",
            migrator_role: "verdaxis_migrator",
            backup_role: "verdaxis_backup",
        },
        Environment::Staging => Roles {
            database_name: "verdaxis_staging",
            app_role: "verdaxis_app_staging",
            migrator_role: "verdaxis_migrator_staging",
            backup_role: "verdaxis_backup_staging",
        },
    }
}

/// Where and as whom `psql` connects.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub roles: Roles,
    pub host: String,
    pub port: u16,
    pub password: String,
}

/// One database URL as SQLAlchemy's `make_url` read it: the scheme, the
/// percent-decoded user and password, the host, the port and the database.
struct ParsedUrl {
    scheme: String,
    user: String,
    password: String,
    host: String,
    port: u16,
    database: String,
    has_query: bool,
}

fn parse_url(text: &str) -> Option<ParsedUrl> {
    let url = url::Url::parse(text).ok()?;
    let decode = |part: &str| {
        percent_encoding::percent_decode_str(part)
            .decode_utf8()
            .ok()
            .map(|value| value.into_owned())
    };
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    Some(ParsedUrl {
        scheme: url.scheme().to_owned(),
        user: decode(url.username())?,
        password: decode(url.password().unwrap_or(""))?,
        host,
        port: url.port().unwrap_or(DEFAULT_PORT),
        database: decode(url.path().trim_start_matches('/'))?,
        has_query: url.query().is_some_and(|query| !query.is_empty()),
    })
}

/// A present, non-blank URL, trimmed.
fn required_url(text: Option<&str>) -> Result<&str, &'static str> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or("explicit DATABASE_URL and MIGRATOR_DATABASE_URL are required")
}

/// Bind both URLs to the environment's roles, refusing as Python refused.
pub fn resolve_target(
    environment: Environment,
    app_text: Option<&str>,
    migrator_text: Option<&str>,
) -> Result<Target, &'static str> {
    let roles = roles(environment);
    let (app_text, migrator_text) = (required_url(app_text)?, required_url(migrator_text)?);
    let (app, migrator) = match (parse_url(app_text), parse_url(migrator_text)) {
        (Some(app), Some(migrator)) => (app, migrator),
        _ => return Err("the database URLs must be valid PostgreSQL URLs"),
    };
    if !app.scheme.starts_with("postgresql")
        || !migrator.scheme.starts_with("postgresql")
        || app.has_query
        || migrator.has_query
    {
        return Err("the database URLs must be PostgreSQL URLs without query routing");
    }
    if app.database != roles.database_name || app.user != roles.app_role {
        return Err("DATABASE_URL does not match the application identity");
    }
    if migrator.database != roles.database_name || migrator.user != roles.migrator_role {
        return Err("MIGRATOR_DATABASE_URL does not match the migrator identity");
    }
    if (&app.host, app.port) != (&migrator.host, migrator.port) {
        return Err("both database URLs must use the same endpoint");
    }
    if app_text == migrator_text
        || app.user == migrator.user
        || app.password.is_empty()
        || migrator.password.is_empty()
        || migrator.host.is_empty()
    {
        return Err("the app and migrator URLs must use distinct roles and credentials");
    }
    Ok(Target {
        roles,
        host: migrator.host,
        port: migrator.port,
        password: migrator.password,
    })
}

/// Both bundle files must be regular files, never links.
fn attest_bundle(bundle_root: &Path) -> Result<PathBuf, &'static str> {
    let root = bundle_root
        .canonicalize()
        .map_err(|_| "the ACL bundle root is missing")?;
    for relative in BUNDLE_FILES {
        match std::fs::symlink_metadata(root.join(relative)) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            _ => return Err("the ACL bundle must hold both regular SQL files"),
        }
    }
    Ok(root)
}

/// The `psql` command for `target`: one transaction, stop on the first
/// error, the role names as psql variables. The password is in the child
/// environment only.
pub fn psql_command(psql: &Path, bundle_root: &Path, target: &Target) -> Command {
    let mut command = Command::new(psql);
    command
        .args(["-X", "--single-transaction", "--set", "ON_ERROR_STOP=1"])
        .args([
            "--set",
            &format!("database_name={}", target.roles.database_name),
        ])
        .args(["--set", &format!("app_role={}", target.roles.app_role)])
        .args([
            "--set",
            &format!("migrator_role={}", target.roles.migrator_role),
        ])
        .args([
            "--set",
            &format!("backup_role={}", target.roles.backup_role),
        ])
        .arg("--file")
        .arg(bundle_root.join(CONVERGENCE_SQL))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PGHOST", &target.host)
        .env("PGPORT", target.port.to_string())
        .env("PGDATABASE", target.roles.database_name)
        .env("PGUSER", target.roles.migrator_role)
        .env("PGPASSWORD", &target.password);
    for name in ["LANG", "LC_ALL", "SSL_CERT_FILE", "SSL_CERT_DIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

/// [`run`] with the `psql` path as a parameter.
pub fn converge_with(psql: &Path, args: &AclArgs) -> Result<(), (Exit, &'static str)> {
    let refused = |reason: &'static str| (Exit::Precondition, reason);
    let bundle_root = attest_bundle(&args.bundle_root).map_err(refused)?;
    let values = read_environment_file(&args.env_file)
        .map_err(|exit| (exit, "the deployed environment file is unreadable"))?;
    let value = |key: &str| values.get(key).and_then(Option::as_deref);
    if value("ENVIRONMENT") != Some(args.identity.environment.as_str())
        || value("RELEASE_SHA") != Some(args.identity.release_sha.as_str())
    {
        return Err(refused("the deployed identity does not match this release"));
    }
    let target = resolve_target(
        args.identity.environment,
        value("DATABASE_URL"),
        value("MIGRATOR_DATABASE_URL"),
    )
    .map_err(refused)?;
    if !args.apply {
        return Ok(());
    }
    let status = psql_command(psql, &bundle_root, &target)
        .status()
        .map_err(|_| (Exit::Dependency, "psql could not start"))?;
    if status.success() {
        Ok(())
    } else {
        Err((Exit::Dependency, "owner-executable ACL convergence failed"))
    }
}

/// `acl converge`: without `--apply`, only the checks; with it, `psql`.
pub fn run(args: &AclArgs) -> Result<(), (Exit, &'static str)> {
    converge_with(Path::new(PSQL), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str =
        "postgresql+asyncpg://verdaxis_app_staging:app%24pw@127.0.0.1:5432/verdaxis_staging";
    const MIGRATOR: &str =
        "postgresql+asyncpg://verdaxis_migrator_staging:mig%40pw@127.0.0.1/verdaxis_staging";

    #[test]
    fn resolves_the_migrator_endpoint_and_decoded_password() {
        let target = resolve_target(Environment::Staging, Some(APP), Some(MIGRATOR)).unwrap();
        assert_eq!(target.roles, roles(Environment::Staging));
        assert_eq!((target.host.as_str(), target.port), ("127.0.0.1", 5432));
        assert_eq!(target.password, "mig@pw");
    }

    #[test]
    fn refuses_what_the_python_helper_refused() {
        let other_port = MIGRATOR.replace("127.0.0.1/", "127.0.0.1:5433/");
        let production_app = APP.replace("verdaxis_app_staging", "verdaxis_app");
        let same_role =
            MIGRATOR.replace("verdaxis_migrator_staging:mig", "verdaxis_app_staging:mig");
        let no_password = MIGRATOR.replace(":mig%40pw", "");
        let with_query = format!("{APP}?sslmode=disable");
        let libpq_scheme = APP.replace("postgresql+asyncpg", "postgres");
        for (app, migrator) in [
            (None, Some(MIGRATOR)),
            (Some(APP), Some("  ")),
            (Some(APP), Some(other_port.as_str())),
            (Some(production_app.as_str()), Some(MIGRATOR)),
            (Some(APP), Some(same_role.as_str())),
            (Some(APP), Some(no_password.as_str())),
            (Some(with_query.as_str()), Some(MIGRATOR)),
            (Some(libpq_scheme.as_str()), Some(MIGRATOR)),
            (Some(APP), Some(APP)),
        ] {
            assert!(
                resolve_target(Environment::Staging, app, migrator).is_err(),
                "{app:?} {migrator:?}"
            );
        }
        assert!(resolve_target(Environment::Production, Some(APP), Some(MIGRATOR)).is_err());
    }

    #[test]
    fn psql_gets_the_password_only_in_its_environment() {
        let target = resolve_target(Environment::Staging, Some(APP), Some(MIGRATOR)).unwrap();
        let command = psql_command(Path::new(PSQL), Path::new("/bundle"), &target);
        let arguments: Vec<String> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert!(
            arguments
                .iter()
                .all(|argument| !argument.contains("mig@pw"))
        );
        assert!(arguments.contains(&"migrator_role=verdaxis_migrator_staging".to_owned()));
        assert_eq!(
            arguments.last().map(String::as_str),
            Some("/bundle/deploy/postgres/converge_runtime_object_acls.sql")
        );
        let password = command
            .get_envs()
            .find(|(key, _)| *key == "PGPASSWORD")
            .and_then(|(_, value)| value);
        assert_eq!(password.and_then(|value| value.to_str()), Some("mig@pw"));
    }
}
