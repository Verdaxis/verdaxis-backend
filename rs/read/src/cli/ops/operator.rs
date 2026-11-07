//! The trusted operator's database identity (Phase 10 Tasks 21 and 22).
//!
//! Operator commands (`ops ingest signals`, `ops import gena`) connect only as
//! the environment's operator role, never as the app or migrator role. The
//! operator sources `/etc/verdaxis-operator.env`, which names the URL in
//! `OPERATOR_DATABASE_URL`; the URL is checked before connecting and the
//! connection is attested before the first write.
use std::collections::HashMap;
use std::str::FromStr;

use sqlx::Connection;
use sqlx::Row;
use sqlx::postgres::{PgConnectOptions, PgConnection};

use crate::cli::{Environment, Exit};

/// The operator role of each environment.
pub fn operator_role(environment: Environment) -> &'static str {
    match environment {
        Environment::Production => "verdaxis_operator",
        Environment::Staging => "verdaxis_operator_staging",
    }
}

pub fn database_name(environment: Environment) -> &'static str {
    super::runtime_env::identity(environment).database_name
}

/// Connect as the environment's operator and attest the connection. The
/// refusal reason names a rule, never the URL.
pub async fn connect(
    environment: Environment,
    variables: &HashMap<String, String>,
) -> Result<PgConnection, (Exit, &'static str)> {
    let url = variables
        .get("OPERATOR_DATABASE_URL")
        .map(String::as_str)
        .unwrap_or("");
    let options = match PgConnectOptions::from_str(url) {
        Ok(options)
            if options.get_username() == operator_role(environment)
                && options.get_database() == Some(database_name(environment)) =>
        {
            options
        }
        _ => {
            return Err((
                Exit::Precondition,
                "OPERATOR_DATABASE_URL is not this environment's operator",
            ));
        }
    };
    let unreachable = (Exit::Dependency, "the database is unreachable");
    let mut connection = PgConnection::connect_with(&options)
        .await
        .map_err(|_| unreachable)?;
    // Attest the connection that writes: the exact database and role.
    let row = sqlx::query("SELECT current_database(), current_user::text")
        .fetch_one(&mut connection)
        .await
        .map_err(|_| unreachable)?;
    if row.get::<String, _>(0) != database_name(environment)
        || row.get::<String, _>(1) != operator_role(environment)
    {
        return Err((
            Exit::Precondition,
            "the connection is not the operator on this database",
        ));
    }
    Ok(connection)
}
