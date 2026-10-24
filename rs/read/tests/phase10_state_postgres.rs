//! Phase 10 Task 3 target attestation. Needs a disposable PostgreSQL only.
use sqlx::{Connection, PgConnection};
use verdaxis_read::cli::Environment;
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::gate::{expected_role, require_target};

async fn connect(variable: &str) -> PgConnection {
    let url = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable} must name a disposable database"));
    PgConnection::connect(&url)
        .await
        .expect("connect a disposable database")
}

async fn current_role(connection: &mut PgConnection) -> String {
    sqlx::query_scalar("SELECT current_user")
        .fetch_one(&mut *connection)
        .await
        .expect("read the connection role")
}

#[tokio::test]
async fn target_attestation_rejects_wrong_database_and_role() {
    let database =
        std::env::var("RUST_TEST_DATABASE_NAME").unwrap_or_else(|_| "verdaxis_rust_test".into());
    let mut connection = connect("RUST_TEST_DATABASE_URL").await;
    let role = current_role(&mut connection).await;
    let matching = [Environment::Production, Environment::Staging]
        .into_iter()
        .find(|environment| expected_role(*environment) == role);

    match matching {
        Some(environment) => {
            require_target(&mut connection, environment, &database)
                .await
                .expect("the deployed role on its own database is accepted");
            let wrong = if environment == Environment::Production {
                Environment::Staging
            } else {
                Environment::Production
            };
            assert_eq!(
                require_target(&mut connection, wrong, &database).await,
                Err(Exit::Precondition),
                "the other environment's role is refused"
            );
        }
        None => {
            for environment in [Environment::Production, Environment::Staging] {
                assert_eq!(
                    require_target(&mut connection, environment, &database).await,
                    Err(Exit::Precondition),
                    "a role that no environment deploys is refused"
                );
            }
        }
    }

    assert_eq!(
        require_target(
            &mut connection,
            matching.unwrap_or(Environment::Staging),
            "verdaxis_wrong_database"
        )
        .await,
        Err(Exit::Precondition),
        "the same role on another database is refused"
    );

    // A different login role is never accepted as an application role.
    if let Ok(url) = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL") {
        let mut owner = PgConnection::connect(&url)
            .await
            .expect("connect the disposable owner");
        let owner_role = current_role(&mut owner).await;
        for environment in [Environment::Production, Environment::Staging] {
            if expected_role(environment) != owner_role {
                assert_eq!(
                    require_target(&mut owner, environment, &database).await,
                    Err(Exit::Precondition),
                    "the owner role is not the {environment:?} application role"
                );
            }
        }
    }
}
