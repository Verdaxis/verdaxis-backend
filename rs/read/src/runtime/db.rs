use super::config::{database_target, validate_budget, validate_config};
use crate::{ApiError, Config};
use sqlx::{ConnectOptions, Row, postgres::PgPoolOptions};
use std::time::Duration;

const ROLE_ATTESTATION_SQL: &str = "SELECT current_database(), current_user, rol.rolcanlogin, rol.rolinherit, \
    rol.rolsuper, rol.rolcreatedb, rol.rolcreaterole, rol.rolreplication, rol.rolbypassrls, \
    EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members AS membership \
    WHERE membership.member = rol.oid OR membership.roleid = rol.oid) \
    FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user";

pub fn log_session_timezone(timezone: &str) {
    tracing::info!(database_timezone = timezone, "database session timezone");
}

#[derive(Clone)]
pub struct Pools {
    pub read: sqlx::PgPool,
    pub health: sqlx::PgPool,
}

pub async fn connect(config: &Config) -> Result<Pools, ApiError> {
    validate_config(config).map_err(|_| ApiError::internal())?;
    if config.environment == "staging" {
        // Trusted runtime preparation supplies this non-secret artifact content.
        // The prestart control plane, not this process, checks the .env digest.
        let budget = std::env::var("RUST_PREPARED_BUDGET").map_err(|_| ApiError::internal())?;
        super::config::validate_prepared_budget(config, &budget)
            .map_err(|_| ApiError::internal())?;
    }
    let target = database_target(config).map_err(|_| ApiError::internal())?;
    // SQLx also reads PGOPTIONS. Refuse ambient session overrides rather than
    // allowing them to replace the server timezone or runtime authority.
    if std::env::var_os("PGOPTIONS").is_some() {
        return Err(ApiError::internal());
    }
    let options = sqlx::postgres::PgConnectOptions::new_without_pgpass()
        .host(&target.host)
        .port(target.port)
        .username(&target.user)
        .password(&target.password)
        .database(&target.database)
        .options([
            ("statement_timeout", "30000"),
            ("lock_timeout", "3000"),
            ("idle_in_transaction_session_timeout", "60000"),
            ("default_transaction_read_only", "on"),
        ])
        .disable_statement_logging();
    // Test-only SQLx row counters; release builds never enable statement events.
    #[cfg(feature = "contract-tests")]
    let options = if config.environment == "test"
        && std::env::var("CONTRACT_GROUP").as_deref() == Ok("curves_canonical")
    {
        options
            .log_statements("debug".parse().expect("log level"))
            .log_slow_statements("debug".parse().expect("log level"), Duration::from_secs(30))
    } else {
        options
    };
    let read = pool_options(config.pool_max - 1, config.db_session_time_zone.clone())
        .connect_with(options.clone())
        .await
        .map_err(|_| ApiError::internal())?;
    if let Err(error) = attest(&read, config, &target.database, &target.user).await {
        read.close().await;
        return Err(error);
    }
    let health = match pool_options(1, config.db_session_time_zone.clone())
        .connect_with(options)
        .await
    {
        Ok(pool) => pool,
        Err(_) => {
            read.close().await;
            return Err(ApiError::internal());
        }
    };
    Ok(Pools { read, health })
}

fn pool_options(max_connections: u32, timezone: String) -> PgPoolOptions {
    PgPoolOptions::new()
        .min_connections(0)
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(move |connection, _metadata| {
            let timezone = timezone.clone();
            Box::pin(async move {
                // SQLx 0.9 sends TimeZone=UTC in its startup packet; .options()
                // cannot override it. set_config is the bound equivalent of SET
                // SESSION, executed once before this connection enters the pool.
                sqlx::query("SELECT set_config('TimeZone', $1, false)")
                    .bind(&timezone)
                    .execute(&mut *connection)
                    .await?;
                let actual: String = sqlx::query_scalar("SHOW TimeZone")
                    .fetch_one(&mut *connection)
                    .await?;
                if actual != timezone {
                    return Err(sqlx::Error::Protocol(
                        "database session timezone mismatch".into(),
                    ));
                }
                Ok(())
            })
        })
}

async fn attest(
    pool: &sqlx::PgPool,
    config: &Config,
    database: &str,
    user: &str,
) -> Result<(), ApiError> {
    let mut connection = pool.acquire().await.map_err(|_| ApiError::internal())?;
    let row = sqlx::query(ROLE_ATTESTATION_SQL)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| ApiError::internal())?;
    let database_matches = row
        .try_get::<String, _>(0)
        .map_err(|_| ApiError::internal())?
        == database;
    let user_matches = row
        .try_get::<String, _>(1)
        .map_err(|_| ApiError::internal())?
        == user;
    let can_login: bool = row.try_get(2).map_err(|_| ApiError::internal())?;
    let mut unsafe_role = !can_login;
    for index in 3..10 {
        unsafe_role |= row
            .try_get::<bool, _>(index)
            .map_err(|_| ApiError::internal())?;
    }
    if !database_matches || !user_matches || unsafe_role {
        return Err(ApiError::internal());
    }
    let observed: String = sqlx::query_scalar("SHOW max_connections")
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| ApiError::internal())?;
    validate_budget(
        config
            .python_budget
            .checked_add(config.pool_max)
            .ok_or_else(ApiError::internal)?,
        config.configured_max_connections,
        observed.parse().map_err(|_| ApiError::internal())?,
    )
    .map_err(|_| ApiError::internal())?;
    let timezone: String = sqlx::query_scalar("SHOW TimeZone")
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| ApiError::internal())?;
    let database_timezone: Option<String> = sqlx::query_scalar(
        "SELECT split_part(setting, '=', 2) FROM pg_catalog.pg_db_role_setting s \
         CROSS JOIN LATERAL unnest(s.setconfig) AS setting \
         WHERE s.setdatabase = (SELECT oid FROM pg_catalog.pg_database WHERE datname = current_database()) \
         AND s.setrole = 0 AND lower(split_part(setting, '=', 1)) = 'timezone'",
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| ApiError::internal())?;
    if timezone != config.db_session_time_zone
        || database_timezone
            .as_ref()
            .is_some_and(|zone| zone != &config.db_session_time_zone)
    {
        tracing::error!("database session timezone does not match configured policy");
        return Err(ApiError::internal());
    }
    log_session_timezone(&timezone);
    Ok(())
}
