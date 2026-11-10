//! Phase 10 Task 19: the signup-canary cleanup against the disposable
//! PostgreSQL of `run_postgres.sh`. Each case clones
//! `RUST_INGEST_TEMPLATE_DATABASE` (the baseline plus the real ACL
//! convergence) as `verdaxis_staging`, seeds rows as the administrator and
//! cleans up as the staging application role, which is what the route uses.
//! The cases run one at a time.
use std::str::FromStr;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::cli::canary::{CleanupError, Counts, cleanup};

const APP: &str = "verdaxis_app_staging";
const APP_PASSWORD: &str = "disposable_staging_pw";
const DATABASE: &str = "verdaxis_staging";
const DOMAIN: &str = "staging-20261104.canary.verdaxis.exchange";
const EMAIL: &str = "canary+staging-20261104@staging-20261104.canary.verdaxis.exchange";
const TOKEN: &str = "monitor-disposable-token";

fn admin_options() -> PgConnectOptions {
    let url = std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
        .expect("RUST_TEMPLATE_ADMIN_DATABASE_URL must name the disposable server");
    PgConnectOptions::from_str(&url).expect("parse the administrator URL")
}

async fn admin(database: &str, statement: &str) {
    let mut connection = PgConnection::connect_with(&admin_options().database(database))
        .await
        .unwrap();
    // Fixed test statements, written in this file.
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

async fn scalar(sql: &str) -> i64 {
    let mut connection = PgConnection::connect_with(&admin_options().database(DATABASE))
        .await
        .unwrap();
    let value = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value
}

/// A fresh `verdaxis_staging` with no user or organization rows, and a pool
/// of the application role. Its sessions default to read-only transactions,
/// as the runtime read pool that serves the route does.
async fn fresh_staging() -> PgPool {
    let template = std::env::var("RUST_INGEST_TEMPLATE_DATABASE")
        .expect("RUST_INGEST_TEMPLATE_DATABASE names the converged template");
    admin(
        "postgres",
        &format!("DROP DATABASE IF EXISTS {DATABASE} WITH (FORCE)"),
    )
    .await;
    admin(
        "postgres",
        &format!("CREATE DATABASE {DATABASE} TEMPLATE {template}"),
    )
    .await;
    let options = admin_options()
        .username(APP)
        .password(APP_PASSWORD)
        .database(DATABASE)
        .options([("default_transaction_read_only", "on")]);
    PgPoolOptions::new()
        .max_connections(10)
        .connect_with(options)
        .await
        .unwrap()
}

/// An organization with this domain, as the administrator.
async fn organization(domain: &str) -> Uuid {
    let id = Uuid::new_v4();
    admin(
        DATABASE,
        &format!(
            "INSERT INTO organizations (id, name, type, domain) VALUES ('{id}', 'Canary', 'BUYER', '{domain}')"
        ),
    )
    .await;
    id
}

/// A user as registration leaves it: a member of the organization, with its
/// first status transition (cascaded on delete).
async fn user(email: &str, organization: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    let organization = organization.map_or("NULL".to_owned(), |id| format!("'{id}'"));
    admin(
        DATABASE,
        &format!(
            "INSERT INTO users (id, email, password_hash, organization_id) \
             VALUES ('{id}', '{email}', 'x', {organization}); \
             INSERT INTO user_status_transitions (id, user_id, to_status, effective_at, provenance) \
             VALUES (gen_random_uuid(), '{id}', 'PENDING', now(), 'registration')"
        ),
    )
    .await;
    id
}

async fn kyc_document(user: Uuid, organization: Uuid) {
    admin(
        DATABASE,
        &format!(
            "INSERT INTO kyc_documents (id, submission_id, user_id, organization_id, kind, \
             content_type, size_bytes, key_id, wrapped_key, key_nonce, data_nonce, ciphertext) \
             VALUES (gen_random_uuid(), gen_random_uuid(), '{user}', '{organization}', 'IDENTITY', \
             'application/pdf', 1, 'k1', '\\x00', '\\x00', '\\x00', '\\x00')"
        ),
    )
    .await;
}

async fn rows(table: &str, id: Uuid) -> i64 {
    scalar(&format!("SELECT count(*) FROM {table} WHERE id = '{id}'")).await
}

const BOTH: Counts = Counts {
    deleted_users: 1,
    deleted_orgs: 1,
};

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn canary_deletes_the_user_before_the_organization() {
    let pool = fresh_staging().await;
    let organization = organization(DOMAIN).await;
    let user = user(EMAIL, Some(organization)).await;
    // The organization cannot go while its member exists (the foreign key has
    // no action), so a deleted organization proves the user went first.
    assert_eq!(cleanup(&pool, EMAIL, DOMAIN).await, Ok(BOTH));
    assert_eq!(
        (
            rows("users", user).await,
            rows("organizations", organization).await
        ),
        (0, 0)
    );
    assert_eq!(
        scalar(&format!(
            "SELECT count(*) FROM user_status_transitions WHERE user_id = '{user}'"
        ))
        .await,
        0
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn canary_deletes_the_organization_only_when_it_has_no_members_and_matches_the_domain() {
    let pool = fresh_staging().await;
    // Another member keeps the organization.
    let shared = organization(DOMAIN).await;
    user(EMAIL, Some(shared)).await;
    let member = user("someone@example.com", Some(shared)).await;
    assert_eq!(
        cleanup(&pool, EMAIL, DOMAIN).await,
        Ok(Counts {
            deleted_users: 1,
            deleted_orgs: 0
        })
    );
    assert_eq!(
        (
            rows("organizations", shared).await,
            rows("users", member).await
        ),
        (1, 1)
    );

    // The user's organization has another domain: it stays, and the lookup by
    // the canary domain is not used because the user's organization exists.
    let pool = fresh_staging().await;
    let foreign = organization("example.com").await;
    let canary_named = organization(DOMAIN).await;
    user(EMAIL, Some(foreign)).await;
    assert_eq!(
        cleanup(&pool, EMAIL, DOMAIN).await,
        Ok(Counts {
            deleted_users: 1,
            deleted_orgs: 0
        })
    );
    assert_eq!(
        (
            rows("organizations", foreign).await,
            rows("organizations", canary_named).await
        ),
        (1, 1)
    );

    // No user left (an earlier run deleted it): the empty canary organization
    // is found by its domain and deleted.
    let pool = fresh_staging().await;
    let orphan = organization(DOMAIN).await;
    assert_eq!(
        cleanup(&pool, EMAIL, DOMAIN).await,
        Ok(Counts {
            deleted_users: 0,
            deleted_orgs: 1
        })
    );
    assert_eq!(rows("organizations", orphan).await, 0);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn canary_retained_kyc_documents_block_both_deletions_with_409() {
    // A document of the user, or one of only its organization, refuses the
    // whole cleanup; nothing is deleted.
    for document_of_user in [true, false] {
        let pool = fresh_staging().await;
        let organization = organization(DOMAIN).await;
        let user = user(EMAIL, Some(organization)).await;
        let owner = if document_of_user {
            user
        } else {
            // The document's user is another member, so only the organization
            // id links it to the canary.
            self::user("someone@example.com", Some(organization)).await
        };
        kyc_document(owner, organization).await;
        assert_eq!(
            cleanup(&pool, EMAIL, DOMAIN).await,
            Err(CleanupError::Retained)
        );
        assert_eq!(
            (
                rows("users", user).await,
                rows("organizations", organization).await
            ),
            (1, 1)
        );
    }
    // The route answers Python's 409 body.
    let pool = fresh_staging().await;
    let organization = organization(DOMAIN).await;
    let user = user(EMAIL, Some(organization)).await;
    kyc_document(user, organization).await;
    let (status, body) = route(&pool).await;
    assert_eq!(status, 409);
    assert_eq!(
        body,
        json!({"detail": {
            "code": "KYC_DOCUMENTS_RETAINED",
            "message": "Retained KYC documents prevent deletion of this account or organization",
        }})
    );
    assert_eq!(rows("users", user).await, 1);
}

async fn route(pool: &PgPool) -> (u16, serde_json::Value) {
    let app = verdaxis_read::routes::monitor_canary::routes(Some(TOKEN.to_owned()), pool.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(verdaxis_read::routes::monitor_canary::PATH)
                .header("content-type", "application/json")
                .header("x-monitor-token", TOKEN)
                .body(Body::from(json!({"email": EMAIL}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn canary_repeat_returns_zeros() {
    let pool = fresh_staging().await;
    let organization = organization(DOMAIN).await;
    user(EMAIL, Some(organization)).await;
    assert_eq!(
        route(&pool).await,
        (200, json!({"deleted_users": 1, "deleted_orgs": 1}))
    );
    assert_eq!(
        route(&pool).await,
        (200, json!({"deleted_users": 0, "deleted_orgs": 0}))
    );
    assert_eq!(cleanup(&pool, EMAIL, DOMAIN).await, Ok(Counts::default()));
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn canary_concurrent_cleanup_deletes_once() {
    let pool = fresh_staging().await;
    let organization = organization(DOMAIN).await;
    let user = user(EMAIL, Some(organization)).await;
    // Hold the user row so that every cleanup has started, and read what it
    // can, before any of them can delete; then release them together.
    let mut holder = PgConnection::connect_with(&admin_options().database(DATABASE))
        .await
        .unwrap();
    let mut hold = holder.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .execute(&mut *hold)
        .await
        .unwrap();
    let runs: Vec<_> = (0..8)
        .map(|_| {
            let pool = pool.clone();
            tokio::spawn(async move { cleanup(&pool, EMAIL, DOMAIN).await })
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    hold.commit().await.unwrap();
    let mut total = Counts::default();
    for run in runs {
        let counts = run.await.unwrap().expect("no cleanup fails");
        total.deleted_users += counts.deleted_users;
        total.deleted_orgs += counts.deleted_orgs;
    }
    assert_eq!(total, BOTH, "each row is deleted by exactly one cleanup");
    assert_eq!(
        (
            rows("users", user).await,
            rows("organizations", organization).await
        ),
        (0, 0)
    );
}
