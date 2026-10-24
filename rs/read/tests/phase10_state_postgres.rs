//! Phase 10 Task 3 target attestation. Needs a disposable PostgreSQL only.
//!
//! The harness creates a login role with each deployed name, so accept and deny
//! run for real instead of being inferred from the code under test.
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection};
use std::str::FromStr;
use verdaxis_read::cli::Environment;
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::gate::{expected_role, require_target};

fn url(variable: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must name a disposable database"))
}

/// The database the URL names, read independently of the code under test.
fn database_of(url: &str) -> String {
    PgConnectOptions::from_str(url)
        .expect("parse a disposable URL")
        .get_database()
        .expect("the URL names a database")
        .to_owned()
}

async fn connect(url: &str) -> PgConnection {
    PgConnection::connect(url)
        .await
        .expect("connect a disposable database")
}

async fn identity(connection: &mut PgConnection) -> (String, String) {
    sqlx::query_as("SELECT current_database(), current_user")
        .fetch_one(&mut *connection)
        .await
        .expect("read the connection identity")
}

#[tokio::test]
async fn expected_roles_are_the_deployed_literals() {
    assert_eq!(expected_role(Environment::Production), "verdaxis_app");
    assert_eq!(expected_role(Environment::Staging), "verdaxis_app_staging");
}

#[tokio::test]
async fn target_attestation_rejects_wrong_database_and_role() {
    assert_eq!(expected_role(Environment::Production), "verdaxis_app");
    assert_eq!(expected_role(Environment::Staging), "verdaxis_app_staging");

    // Accept: each deployed role on the database the release uses.
    let production_url = url("RUST_TEST_PRODUCTION_DATABASE_URL");
    let staging_url = url("RUST_TEST_STAGING_DATABASE_URL");
    let database = database_of(&production_url);
    assert_eq!(database_of(&staging_url), database);

    let mut production = connect(&production_url).await;
    assert_eq!(
        identity(&mut production).await,
        ("verdaxis_rust_test".into(), "verdaxis_app".into())
    );
    require_target(&mut production, Environment::Production, &database)
        .await
        .expect("the production role is accepted on its database");

    let mut staging = connect(&staging_url).await;
    assert_eq!(
        identity(&mut staging).await,
        ("verdaxis_rust_test".into(), "verdaxis_app_staging".into())
    );
    require_target(&mut staging, Environment::Staging, &database)
        .await
        .expect("the staging role is accepted on its database");

    // Deny: the other environment's role, with the database matching. Only the
    // role differs, so this isolates the role arm.
    assert_eq!(
        require_target(&mut production, Environment::Staging, &database).await,
        Err(Exit::Precondition),
        "the production role is not the staging role"
    );
    assert_eq!(
        require_target(&mut staging, Environment::Production, &database).await,
        Err(Exit::Precondition),
        "the staging role is not the production role"
    );

    // Deny: correct role, wrong database. Only the database name differs.
    let wrong_url = url("RUST_TEST_WRONG_DATABASE_URL");
    let wrong_database = database_of(&wrong_url);
    assert_ne!(wrong_database, database);
    let mut elsewhere = connect(&wrong_url).await;
    assert_eq!(
        identity(&mut elsewhere).await,
        (wrong_database.clone(), "verdaxis_app".into())
    );
    assert_eq!(
        require_target(&mut elsewhere, Environment::Production, &database).await,
        Err(Exit::Precondition),
        "the production role on another database is refused"
    );
    // The same connection passes under its own database name, so the refusal
    // above is the name and not a broken connection.
    require_target(&mut elsewhere, Environment::Production, &wrong_database)
        .await
        .expect("the same connection passes on its own database name");

    // Deny: an undeployed role on the correct database.
    let mut undeployed = connect(&url("RUST_TEST_DATABASE_URL")).await;
    assert_eq!(
        identity(&mut undeployed).await,
        (database.clone(), "rust_app".into())
    );
    for environment in [Environment::Production, Environment::Staging] {
        assert_eq!(
            require_target(&mut undeployed, environment, &database).await,
            Err(Exit::Precondition),
            "an undeployed role is refused for {environment:?}"
        );
    }
}
