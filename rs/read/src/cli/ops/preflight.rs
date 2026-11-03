//! `ops preflight`: the read-only deployed runtime preflight (Phase 10 Task 9),
//! which replaces `scripts/preflight_runtime.py`.
//!
//! Every file check runs before the one connection: the deployed `.env` must
//! name this environment and release, and its `DATABASE_URL` must be this
//! environment's app role and database. The connection then attests, as
//! `verify_database_runtime` did, that PostgreSQL reports the same database and
//! role, that the role is least-privilege, and that `max_connections` holds the
//! configured budget (Python, the Rust pool and the reserve).
use std::str::FromStr;
use std::time::Duration;

use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};

use super::budget;
use super::runtime_env::{
    Refusal, app_database_url, identity, read_environment_file, require_identity,
};
use crate::cli::{Exit, PreflightArgs};

/// A loopback PostgreSQL answers at once; a slow answer is an unhealthy one.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The connected identity and role attributes (`app/database.py`).
const ROLE_ATTESTATION_SQL: &str = "SELECT current_database(), current_user::text, \
     rol.rolcanlogin, rol.rolinherit, rol.rolsuper, rol.rolcreatedb, rol.rolcreaterole, \
     rol.rolreplication, rol.rolbypassrls, EXISTS (\
       SELECT 1 FROM pg_catalog.pg_auth_members AS membership \
       WHERE membership.member = rol.oid OR membership.roleid = rol.oid) \
     FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user";

type RoleRow = (
    String,
    String,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
);

pub async fn run(args: &PreflightArgs) -> Result<(), (Exit, &'static str)> {
    let environment = args.identity.environment;
    let refused = |refusal: Refusal| (Exit::Precondition, refusal.0);
    let values =
        read_environment_file(&args.environment_file).map_err(|exit| (exit, "environment_file"))?;
    require_identity(&values, environment, &args.identity.release_sha).map_err(refused)?;
    let url = app_database_url(&values, environment).map_err(refused)?;
    let settings = budget::configured_settings(&values).map_err(refused)?;
    let configured = budget::derive(
        &settings,
        budget::rust_pool(&values).map_err(refused)?,
        0,
        0,
    )
    .map_err(refused)?;

    let options =
        PgConnectOptions::from_str(&url).map_err(|_| (Exit::Precondition, "database_url"))?;
    let mut connection =
        tokio::time::timeout(CONNECT_TIMEOUT, PgConnection::connect_with(&options))
            .await
            .map_err(|_| (Exit::Dependency, "connect"))?
            .map_err(|_| (Exit::Dependency, "connect"))?;
    let role: RoleRow = sqlx::query_as(ROLE_ATTESTATION_SQL)
        .fetch_one(&mut connection)
        .await
        .map_err(|_| (Exit::Dependency, "attestation"))?;
    let max_connections: String = sqlx::query_scalar("SHOW max_connections")
        .fetch_one(&mut connection)
        .await
        .map_err(|_| (Exit::Dependency, "attestation"))?;
    let _ = connection.close().await;

    let (
        database,
        user,
        can_login,
        inherits,
        superuser,
        create_database,
        create_role,
        replicate,
        bypass_rls,
        memberships,
    ) = role;
    let expected = identity(environment);
    if database != expected.database_name || user != expected.app_role {
        return Err((Exit::Precondition, "database_identity"));
    }
    if superuser
        || !can_login
        || inherits
        || create_database
        || create_role
        || replicate
        || bypass_rls
        || memberships
    {
        return Err((Exit::Precondition, "role_privilege"));
    }
    let observed: u64 = max_connections
        .trim()
        .parse()
        .map_err(|_| (Exit::Dependency, "attestation"))?;
    if configured["total"] > observed {
        return Err((Exit::Precondition, "max_connections"));
    }
    Ok(())
}
