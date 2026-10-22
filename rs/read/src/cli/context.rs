//! Service configuration and the purpose-bound context interface.
use super::{Environment, Exit, JobKind, MonitorKind, ServeArgs};
use crate::Config;
use crate::runtime::config::{config_from_deployed_environment, deployed_arguments};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

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

impl RunContext {
    /// Task 3 supplies identity/target/deploy-state gates; Task 9 supplies the
    /// credential projections. Never substitute the HTTP pool for those roles.
    pub async fn open(
        _purpose: Purpose,
        _environment: Environment,
        _release_sha: &str,
    ) -> Result<Self, Exit> {
        Err(Exit::Precondition)
    }
}

pub async fn open(
    purpose: Purpose,
    environment: Environment,
    release_sha: &str,
) -> Result<RunContext, Exit> {
    RunContext::open(purpose, environment, release_sha).await
}

pub fn load_serve_config(args: &ServeArgs) -> Result<Config, Exit> {
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
