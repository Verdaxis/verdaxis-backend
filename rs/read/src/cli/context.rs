//! Service configuration and the purpose-bound context interface.
use super::{Environment, Exit, JobKind, MonitorKind, ServeArgs};
use crate::Config;
use crate::runtime::config::{config_from_deployed_environment, deployed_arguments};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

const MAX_BUDGET_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Serve,
    Job(JobKind),
    Monitor(MonitorKind),
    Migrate,
    Operator,
}

pub struct RunContext {
    pub environment: Environment,
    pub release_sha: String,
    pub now: DateTime<Utc>,
    pub pool: PgPool,
    pub migrator: Option<PgPool>,
}

/// Connections one job or operator process may hold. Jobs are not part of the
/// HTTP budget; they run inside the 20-connection maintenance reserve.
const JOB_POOL_MAX_CONNECTIONS: u32 = 4;
const JOB_ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl RunContext {
    /// Open the purpose-bound context from this process's environment, which
    /// the unit loads from the purpose's projected file (`ops prepare-runtime`).
    ///
    /// Each job runs its own deploy-state preflight and attests the target on
    /// the connection it writes with (Task 3). This supplies the credentials:
    /// `migrate` gets only the migrator URL of its invocation; every other
    /// purpose gets only the app `DATABASE_URL`. Never substitute one role's
    /// pool for another's. `serve` has its own configuration path.
    pub async fn open(
        purpose: Purpose,
        environment: Environment,
        release_sha: &str,
    ) -> Result<Self, Exit> {
        let variables: HashMap<String, String> = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        Self::open_with(purpose, environment, release_sha, &variables)
    }

    /// [`open`](Self::open) with an explicit environment. Every check runs
    /// before the pool exists; the pool connects lazily, so an unreachable
    /// database is reported by the first query as a dependency failure.
    pub fn open_with(
        purpose: Purpose,
        environment: Environment,
        release_sha: &str,
        variables: &HashMap<String, String>,
    ) -> Result<Self, Exit> {
        let value = |key: &str| variables.get(key).map(String::as_str).unwrap_or("");
        super::gate::require_identity(
            value("ENVIRONMENT"),
            value("RELEASE_SHA"),
            environment,
            release_sha,
        )?;
        let bindings: super::ops::dotenv::Bindings = variables
            .iter()
            .map(|(key, value)| (key.clone(), Some(value.clone())))
            .collect();
        let (url, is_migrator) = match purpose {
            Purpose::Serve => return Err(Exit::Usage),
            Purpose::Migrate => (migrator_url(variables, environment)?, true),
            Purpose::Job(_) | Purpose::Monitor(_) | Purpose::Operator => (
                super::ops::runtime_env::app_database_url(&bindings, environment)
                    .map_err(|_| Exit::Precondition)?,
                false,
            ),
        };
        let options = PgConnectOptions::from_str(&url).map_err(|_| Exit::Precondition)?;
        let pool = PgPoolOptions::new()
            .max_connections(JOB_POOL_MAX_CONNECTIONS)
            .acquire_timeout(JOB_ACQUIRE_TIMEOUT)
            .connect_lazy_with(options);
        Ok(Self {
            environment,
            release_sha: release_sha.to_owned(),
            now: Utc::now(),
            migrator: is_migrator.then(|| pool.clone()),
            pool,
        })
    }
}

/// The migrator role of each deployed environment.
fn migrator_role(environment: Environment) -> &'static str {
    match environment {
        Environment::Production => "verdaxis_migrator",
        Environment::Staging => "verdaxis_migrator_staging",
    }
}

/// `MIGRATOR_DATABASE_URL` of the `migrate` invocation, which must name this
/// environment's migrator role and database.
fn migrator_url(
    variables: &HashMap<String, String>,
    environment: Environment,
) -> Result<String, Exit> {
    let raw = variables
        .get("MIGRATOR_DATABASE_URL")
        .ok_or(Exit::Precondition)?;
    let options = PgConnectOptions::from_str(raw).map_err(|_| Exit::Precondition)?;
    let database = super::ops::runtime_env::identity(environment).database_name;
    if options.get_username() != migrator_role(environment)
        || options.get_database() != Some(database)
    {
        return Err(Exit::Precondition);
    }
    Ok(raw.clone())
}

pub async fn open(
    purpose: Purpose,
    environment: Environment,
    release_sha: &str,
) -> Result<RunContext, Exit> {
    RunContext::open(purpose, environment, release_sha).await
}

pub fn load_serve_config(args: &ServeArgs) -> Result<Config, Exit> {
    // Check raw environment encoding before any optional value can default.
    if std::env::var_os("FLEET_ARTIFACT_PATH").is_some_and(|value| value.into_string().is_err()) {
        return Err(Exit::Precondition);
    }
    if let Ok(document) = std::env::var("RUST_CONFIG") {
        return serde_json::from_str(&document).map_err(|_| Exit::Dependency);
    }
    let bind = args.bind.ok_or(Exit::Usage)?;
    let budget_file = args.budget_file.as_ref().ok_or(Exit::Usage)?;
    // Reuse the existing deployed argument/config validation, not a second
    // interpretation of environment authority.
    let arguments = vec![
        "--bind".into(),
        bind.to_string(),
        "--budget-file".into(),
        budget_file.to_str().ok_or(Exit::Usage)?.to_owned(),
    ];
    let (bind, budget_file) = deployed_arguments(&arguments).map_err(|_| Exit::Usage)?;
    let budget = read_prepared_budget(&budget_file)?;
    let environment: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    config_from_deployed_environment(&environment, bind, &budget).map_err(|_| Exit::Precondition)
}

/// The configuration of the one-shot producer bootstrap: the flag, then
/// `--budget-file <absolute path>`. It binds no socket, so the bind address is
/// a placeholder that is never used.
pub fn load_bootstrap_config(arguments: &[String]) -> Option<Config> {
    let budget_file = crate::runtime::config::bootstrap_arguments(arguments).ok()?;
    let budget = read_prepared_budget(&budget_file).ok()?;
    let environment: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let bind = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    config_from_deployed_environment(&environment, bind, &budget).ok()
}

fn read_prepared_budget(path: &Path) -> Result<String, Exit> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Exit::Precondition)?;
    if !metadata.is_file() || metadata.len() > MAX_BUDGET_BYTES {
        return Err(Exit::Precondition);
    }
    let file = std::fs::File::open(path).map_err(|_| Exit::Dependency)?;
    let mut document = String::new();
    file.take(MAX_BUDGET_BYTES + 1)
        .read_to_string(&mut document)
        .map_err(|_| Exit::Dependency)?;
    if document.len() as u64 > MAX_BUDGET_BYTES {
        return Err(Exit::Precondition);
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn variables(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut variables: HashMap<String, String> = [
            ("ENVIRONMENT", "staging"),
            ("RELEASE_SHA", SHA),
            (
                "DATABASE_URL",
                "postgresql://verdaxis_app_staging:pw@127.0.0.1:1/verdaxis_staging",
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
        for (key, value) in pairs {
            variables.insert((*key).to_owned(), (*value).to_owned());
        }
        variables
    }

    fn open(purpose: Purpose, variables: &HashMap<String, String>) -> Result<RunContext, Exit> {
        RunContext::open_with(purpose, Environment::Staging, SHA, variables)
    }

    #[tokio::test]
    async fn a_job_gets_only_the_app_pool() {
        let context = open(Purpose::Job(JobKind::News), &variables(&[])).unwrap();
        assert_eq!(
            context.pool.connect_options().get_username(),
            "verdaxis_app_staging"
        );
        assert!(context.migrator.is_none());
    }

    #[tokio::test]
    async fn migrate_gets_only_the_migrator_pool() {
        let migrator = "postgresql://verdaxis_migrator_staging:pw@127.0.0.1:1/verdaxis_staging";
        let context = open(
            Purpose::Migrate,
            &variables(&[("MIGRATOR_DATABASE_URL", migrator)]),
        )
        .unwrap();
        let pool = context.migrator.as_ref().unwrap();
        assert_eq!(
            pool.connect_options().get_username(),
            "verdaxis_migrator_staging"
        );
        assert_eq!(
            context.pool.connect_options().get_username(),
            "verdaxis_migrator_staging"
        );
    }

    #[tokio::test]
    async fn open_refuses_another_identity_or_role() {
        let app_as_migrator = "postgresql://verdaxis_app_staging:pw@127.0.0.1:1/verdaxis_staging";
        let production_migrator = "postgresql://verdaxis_migrator:pw@127.0.0.1:1/verdaxis_prod";
        for (purpose, pairs) in [
            (
                Purpose::Job(JobKind::News),
                vec![("ENVIRONMENT", "production")],
            ),
            (Purpose::Job(JobKind::News), vec![("RELEASE_SHA", "f")]),
            (
                Purpose::Job(JobKind::Reminders),
                vec![(
                    "DATABASE_URL",
                    "postgresql://verdaxis_migrator_staging:pw@127.0.0.1:1/verdaxis_staging",
                )],
            ),
            (
                Purpose::Operator,
                vec![(
                    "DATABASE_URL",
                    "postgresql://verdaxis_app:pw@127.0.0.1:1/verdaxis_prod",
                )],
            ),
            (Purpose::Migrate, vec![]),
            (
                Purpose::Migrate,
                vec![("MIGRATOR_DATABASE_URL", app_as_migrator)],
            ),
            (
                Purpose::Migrate,
                vec![("MIGRATOR_DATABASE_URL", production_migrator)],
            ),
        ] {
            assert!(
                open(purpose, &variables(&pairs)).is_err(),
                "{purpose:?} {pairs:?}"
            );
        }
        assert!(matches!(
            open(Purpose::Serve, &variables(&[])),
            Err(Exit::Usage)
        ));
    }
}
