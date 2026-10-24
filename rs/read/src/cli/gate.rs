//! Identity, database-target, deploy-state and `--apply` gates shared by jobs.
//!
//! Every mutating command refuses before its first write when one of these
//! checks fails. They return [`Exit`] rather than messages so a caller cannot
//! echo a credential or a serving-path value by accident.
use super::{Environment, Exit};
use sqlx::PgConnection;
use std::path::Path;

/// Refuse a write unless the caller passed `--apply`. Dry run is the default.
pub fn require_apply(apply: bool) -> Result<(), Exit> {
    if apply { Ok(()) } else { Err(Exit::Usage) }
}

/// The application login role each deployed environment uses.
pub fn expected_role(environment: Environment) -> &'static str {
    match environment {
        Environment::Production => "verdaxis_app",
        Environment::Staging => "verdaxis_app_staging",
    }
}

/// Compare the deployed environment and release identity with the command's
/// exact expected pair. A short, uppercase or abbreviated SHA never matches.
pub fn require_identity(
    actual_env: &str,
    actual_sha: &str,
    expected_env: Environment,
    expected_sha: &str,
) -> Result<(), Exit> {
    if actual_env == expected_env.as_str() && actual_sha == expected_sha {
        Ok(())
    } else {
        Err(Exit::Precondition)
    }
}

/// Refuse ordinary job and operator work while any deploy state path exists.
///
/// `symlink_metadata` does not follow the final component, so a dangling state
/// link is still present state. `Path::exists` follows it and would wrongly
/// report the path absent; that is defect R2 in the Python guards.
pub fn require_deploy_state_absent(state: &Path) -> Result<(), Exit> {
    match std::fs::symlink_metadata(state) {
        Ok(_) => Err(Exit::Precondition),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Exit::Dependency),
    }
}

/// The preflight a job runs before it opens a pool or touches state.
pub fn require_job_preflight(
    actual_env: &str,
    actual_sha: &str,
    expected_env: Environment,
    expected_sha: &str,
    deploy_state: &Path,
) -> Result<(), Exit> {
    require_identity(actual_env, actual_sha, expected_env, expected_sha)?;
    require_deploy_state_absent(deploy_state)
}

/// Attest that this exact connection writes to the expected database under the
/// expected application role. Run it on the connection that will write, not on
/// a separate probe connection.
pub async fn require_target(
    conn: &mut PgConnection,
    environment: Environment,
    database: &str,
) -> Result<(), Exit> {
    let (current_database, current_role): (String, String) =
        sqlx::query_as("SELECT current_database(), current_user")
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| Exit::Dependency)?;
    if current_database == database && current_role == expected_role(environment) {
        Ok(())
    } else {
        Err(Exit::Precondition)
    }
}
