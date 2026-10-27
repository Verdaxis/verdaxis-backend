//! F01 `POST /api/feedback` and F02 `GET /api/admin/feedback` contract tests.
//!
//! The validation matrix is data-driven from
//! `tests/rust_contract/phase2/feedback.json` and runs without a database:
//! Pydantic validates the body before the endpoint quota and before any write,
//! so these cases prove the ported order and envelopes on their own.
//!
//! The HTTP and database cases need disposable PostgreSQL. They provision their
//! own fixture schema through the migrator URL and skip when it is absent, so
//! an offline `cargo test --workspace` stays green.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use verdaxis_read::auth::AuthenticatedError;
use verdaxis_read::auth::limits::SharedLimits;
use verdaxis_read::auth::{VerificationSettings, verify_access};
use verdaxis_read::crud::feedback::{
    Clock, authenticated_error_response, routes_with_clock, validate_query, validate_submission,
};
use verdaxis_read::routes::ports::pydantic_datetime;
use verdaxis_read::{AppState, Config, MarketPolicy};

const FIXTURE: &str = include_str!("../../../tests/rust_contract/phase2/feedback.json");
const AUTH_VECTORS: &str = include_str!("../../../tests/rust_contract/phase2/auth-vectors.json");
const EVIDENCE_MANIFEST: &str =
    include_str!("../../../tests/rust_contract/phase2/evidence-manifest.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("feedback fixture is valid JSON")
}

// ---------------------------------------------------------------------------
// Validation matrix (no database).
// ---------------------------------------------------------------------------

#[test]
fn submission_validation_matches_the_pinned_fixture() {
    for case in fixture()["submission_cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let result = case["result"].as_str().unwrap();
        match validate_submission(case["body"].clone()) {
            Err(error) => {
                assert_eq!(
                    result, "rejected",
                    "{id}: expected acceptance, got an error"
                );
                assert_eq!(error.status.as_u16(), 422, "{id}");
                assert_eq!(error.detail, case["errors"], "{id}");
                assert!(
                    case.get("message").is_none() && case.get("page").is_none(),
                    "{id}: a rejected case must not carry accepted values"
                );
            }
            Ok(submission) => {
                assert_eq!(
                    result, "accepted",
                    "{id}: expected rejection, got acceptance"
                );
                assert_eq!(
                    submission.message,
                    case["message"].as_str().unwrap(),
                    "{id}"
                );
                match (&submission.page, case.get("page")) {
                    (Some(value), Some(expected)) => {
                        assert_eq!(value, expected.as_str().unwrap(), "{id}")
                    }
                    (None, None) | (None, Some(Value::Null)) => {}
                    (value, expected) => {
                        panic!("{id}: page {value:?} does not match fixture {expected:?}")
                    }
                }
            }
        }
    }
}

#[test]
fn pagination_validation_matches_the_pinned_fixture() {
    for case in fixture()["query_cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let query = case["query"].as_str().unwrap();
        let result = case["result"].as_str().unwrap();
        match validate_query(query) {
            Err(error) => {
                assert_eq!(
                    result, "rejected",
                    "{id}: expected acceptance, got an error"
                );
                assert_eq!(error.status.as_u16(), 422, "{id}");
                assert_eq!(error.detail, case["errors"], "{id}");
            }
            Ok(pagination) => {
                assert_eq!(
                    result, "accepted",
                    "{id}: expected rejection, got acceptance"
                );
                assert_eq!(pagination.limit, case["limit"].as_i64().unwrap(), "{id}");
                assert_eq!(pagination.offset, case["offset"].as_i64().unwrap(), "{id}");
            }
        }
    }
}

/// The case list is the qualification manifest's own declaration for this lane,
/// so the fixture cannot quietly drop a case the plan requires.
#[test]
fn required_cases_match_the_evidence_manifest() {
    let manifest: Value = serde_json::from_str(EVIDENCE_MANIFEST).expect("manifest");
    let declared: Vec<&str> = manifest["required_scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| matches!(row["route_id"].as_str(), Some("F01" | "F02")))
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    let fixture = fixture();
    let owned: Vec<&str> = fixture["required_cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(owned, declared);
}

/// The fixture's quotas are the shared authority's, not a second opinion.
#[test]
fn fixture_limits_are_the_shared_policy() {
    let limits = &fixture()["limits"];
    for (path, key) in [
        ("/api/feedback", "feedback"),
        ("/api/admin/feedback", "admin-feedback"),
    ] {
        let (name, per_minute, window) =
            verdaxis_read::auth::limits::endpoint_policy(path).expect("limited endpoint");
        assert_eq!(name, key, "{path} belongs to a different bucket");
        assert_eq!(window, 60, "{path}");
        assert_eq!(
            limits[key.replace('-', "_")]["per_minute"]
                .as_u64()
                .unwrap(),
            u64::from(per_minute),
            "{path}"
        );
    }
    assert_eq!(
        limits["admin_prefix_per_minute"].as_u64().unwrap(),
        300,
        "the shared /api/admin/ prefix bucket"
    );
}

#[tokio::test]
async fn ambiguous_commit_uses_the_crud_database_envelope() {
    let response = authenticated_error_response(AuthenticatedError::AmbiguousCommit);
    assert_eq!(response.status().as_u16(), 500);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"detail": "Database operation failed."})
    );
}

// ---------------------------------------------------------------------------
// Database and HTTP cases.
// ---------------------------------------------------------------------------

struct Harness {
    state: Arc<AppState>,
    limits: SharedLimits,
    settings: Arc<VerificationSettings>,
    token: String,
    subject: Uuid,
    now: DateTime<Utc>,
    migrator: sqlx::PgPool,
}

impl Harness {
    fn app(&self) -> axum::Router {
        let router = routes_with_clock(
            self.settings.clone(),
            self.limits.clone(),
            Clock::Fixed(self.now),
        );
        verdaxis_read::runtime::http::middleware_with_limits(
            router.with_state((*self.state).clone()),
            self.limits.clone(),
            "staging",
        )
    }
}

fn post_feedback(token: Option<&str>, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/feedback")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn post_raw(token: Option<&str>, raw: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/feedback")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(raw.to_owned())).unwrap()
}

fn get_admin(token: &str, query: &str) -> Request<Body> {
    let uri = if query.is_empty() {
        "/api/admin/feedback".to_owned()
    } else {
        format!("/api/admin/feedback?{query}")
    };
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn send(app: &axum::Router, request: Request<Body>) -> (u16, Value, HeaderMap) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, headers)
}

async fn harness() -> Option<Harness> {
    let url = std::env::var("RUST_TEST_DATABASE_URL").ok()?;
    let migrator_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
        .expect("the migrator URL is required alongside the app URL");
    let migrator = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&migrator_url)
        .await
        .expect("connect the fixture migrator");
    provision(&migrator).await;

    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        phase2: Default::default(),
    };
    verdaxis_read::validate_config(&config).expect("valid disposable configuration");
    let pools = verdaxis_read::connect(&config)
        .await
        .expect("connect the app role");
    let state = Arc::new(
        AppState::new(
            config,
            pools,
            MarketPolicy {
                source_sha: verdaxis_read::compiled_sha().into(),
                products: vec![],
                points: vec![],
            },
        )
        .expect("state"),
    );

    let fixture: Value = serde_json::from_str(AUTH_VECTORS).unwrap();
    let settings = Arc::new(VerificationSettings::new(
        fixture["settings"]["current"].as_str().unwrap().into(),
        Some(fixture["settings"]["previous"].as_str().unwrap().into()),
        fixture["settings"]["issuer"].as_str().unwrap().into(),
        fixture["settings"]["audience"].as_str().unwrap().into(),
    ));
    let token = fixture["vectors"][0]["token"].as_str().unwrap().to_owned();
    let now = DateTime::from_timestamp(fixture["now"].as_i64().unwrap(), 0).unwrap();
    let subject = verify_access(&token, &settings, now).unwrap().subject;
    Some(Harness {
        state,
        limits: SharedLimits::default(),
        settings,
        token,
        subject,
        now,
        migrator,
    })
}

/// Create the fixture schema and clear it.
///
/// `feedback_entries.user_id` intentionally has no foreign key here: the admin
/// outer join must be able to show a feedback row whose account is gone, and
/// the real schema's `ON DELETE CASCADE` would make that branch unreachable.
/// The `ck_feedback_test_message` checkpoint is the deliberate database-failure
/// trigger for the CRUD error envelope.
async fn provision(migrator: &sqlx::PgPool) {
    for statement in [
        "CREATE TABLE IF NOT EXISTS organizations (id uuid PRIMARY KEY, name text NOT NULL)",
        "CREATE TABLE IF NOT EXISTS users (\
             id uuid PRIMARY KEY, email text NOT NULL, first_name text, last_name text, \
             organization_id uuid, role text, status text NOT NULL DEFAULT 'APPROVED', \
             email_verified boolean NOT NULL DEFAULT true, \
             must_change_password boolean NOT NULL DEFAULT false, \
             password_changed_at timestamptz, authentication_revoked_at timestamptz)",
        "CREATE TABLE IF NOT EXISTS feedback_entries (\
             id uuid PRIMARY KEY, user_id uuid NOT NULL, message text NOT NULL, page text, \
             created_at timestamptz NOT NULL, \
             CONSTRAINT ck_feedback_test_message CHECK (message <> 'force-database-error'))",
        "CREATE TABLE IF NOT EXISTS market_event_outbox (id uuid PRIMARY KEY)",
        "CREATE TABLE IF NOT EXISTS notifications (id uuid PRIMARY KEY)",
        "GRANT USAGE ON SCHEMA public TO rust_app",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON organizations, users, feedback_entries, \
             market_event_outbox, notifications TO rust_app",
        "TRUNCATE feedback_entries, users, organizations, market_event_outbox, notifications",
    ] {
        sqlx::query(statement)
            .execute(migrator)
            .await
            .unwrap_or_else(|error| panic!("provision failed: {error}\n{statement}"));
    }
}

async fn seed_user(
    migrator: &sqlx::PgPool,
    id: Uuid,
    role: Option<&str>,
    status: &str,
    verified: bool,
    organization: Option<Uuid>,
) {
    sqlx::query(
        "INSERT INTO users \
         (id, email, organization_id, role, status, email_verified, must_change_password) \
         VALUES ($1, $2, $3, $4, $5, $6, false)",
    )
    .bind(id)
    .bind(format!("{id}@phase2.invalid"))
    .bind(organization)
    .bind(role)
    .bind(status)
    .bind(verified)
    .execute(migrator)
    .await
    .unwrap();
}

async fn seed_entry(
    migrator: &sqlx::PgPool,
    user_id: Option<Uuid>,
    message: &str,
    page: Option<&str>,
    created_at: DateTime<Utc>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO feedback_entries (id, user_id, message, page, created_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(user_id.unwrap_or_else(Uuid::new_v4))
    .bind(message)
    .bind(page)
    .bind(created_at)
    .execute(migrator)
    .await
    .unwrap();
    id
}

async fn count(migrator: &sqlx::PgPool, table: &str) -> i64 {
    let query = match table {
        "feedback_entries" => "SELECT count(*) FROM feedback_entries",
        "market_event_outbox" => "SELECT count(*) FROM market_event_outbox",
        "notifications" => "SELECT count(*) FROM notifications",
        _ => panic!("unknown fixture table"),
    };
    sqlx::query_scalar(query).fetch_one(migrator).await.unwrap()
}

#[tokio::test]
async fn submission_commits_and_drops_repeated_idempotency_keys() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let app = harness.app();

    let mut created = Vec::new();
    for _ in 0..2 {
        let request = post_feedback(
            Some(&harness.token),
            json!({"message": "  Payload dropped  ", "page": "/app/marketplace"}),
        );
        let request = {
            let (mut parts, body) = request.into_parts();
            parts
                .headers
                .insert("idempotency-key", "same-key".parse().unwrap());
            Request::from_parts(parts, body)
        };
        let (status, body, _) = send(&app, request).await;
        assert_eq!(status, 201, "{body}");
        created.push(body);
    }
    // The route owns no idempotency store: the same key must not suppress the
    // second row.
    assert_eq!(count(&harness.migrator, "feedback_entries").await, 2);
    assert_ne!(created[0]["id"], created[1]["id"]);
    let expected_instant = pydantic_datetime(harness.now);
    for body in &created {
        assert!(Uuid::parse_str(body["id"].as_str().unwrap()).is_ok());
        assert_eq!(body["created_at"], expected_instant);
    }
    let stored: String =
        sqlx::query_scalar("SELECT message FROM feedback_entries ORDER BY created_at LIMIT 1")
            .fetch_one(&harness.migrator)
            .await
            .unwrap();
    let stored_page: Option<String> =
        sqlx::query_scalar("SELECT page FROM feedback_entries ORDER BY created_at LIMIT 1")
            .fetch_one(&harness.migrator)
            .await
            .unwrap();
    assert_eq!(stored, "Payload dropped");
    assert_eq!(stored_page.as_deref(), Some("/app/marketplace"));
}

#[tokio::test]
async fn invalid_page_metadata_is_dropped_not_rejected() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let app = harness.app();

    for page in [
        json!("https://evil.example/app?x=1"),
        json!("/app#fragment"),
        json!("//app"),
        json!("/app?x=1"),
        Value::Null,
    ] {
        let (status, body, _) = send(
            &app,
            post_feedback(
                Some(&harness.token),
                json!({"message": "kept", "page": page}),
            ),
        )
        .await;
        assert_eq!(status, 201, "{body}");
    }
    let pages: Vec<Option<String>> = sqlx::query_scalar("SELECT page FROM feedback_entries")
        .fetch_all(&harness.migrator)
        .await
        .unwrap();
    assert!(pages.iter().all(Option::is_none), "{pages:?}");
}

#[tokio::test]
async fn admin_listing_joins_the_account_and_organization() {
    let Some(harness) = harness().await else {
        return;
    };
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind("Pelagic Fuels")
        .execute(&harness.migrator)
        .await
        .unwrap();
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let author = Uuid::new_v4();
    seed_user(
        &harness.migrator,
        author,
        Some("BUYER"),
        "APPROVED",
        true,
        Some(org),
    )
    .await;
    sqlx::query("UPDATE users SET first_name = 'Ada', last_name = 'Lovelace' WHERE id = $1")
        .bind(author)
        .execute(&harness.migrator)
        .await
        .unwrap();

    let newest = harness.now;
    seed_entry(&harness.migrator, Some(author), "first", None, newest).await;
    seed_entry(
        &harness.migrator,
        Some(author),
        "second",
        Some("/app/marketplace"),
        newest - chrono::Duration::seconds(60),
    )
    .await;
    // A row whose account is gone: every joined column must be an explicit null.
    seed_entry(
        &harness.migrator,
        None,
        "orphan",
        None,
        newest - chrono::Duration::seconds(120),
    )
    .await;

    let (status, body, _) = send(&harness.app(), get_admin(&harness.token, "")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["total"], 3);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let names: Vec<Option<&str>> = items
        .iter()
        .map(|item| item["user_name"].as_str())
        .collect();
    assert!(names.contains(&Some("Ada Lovelace")), "{names:?}");
    assert!(names.contains(&None), "{names:?}");
    let orphan = items
        .iter()
        .find(|item| item["message"] == "orphan")
        .expect("orphan row present");
    assert_eq!(orphan["user_email"], Value::Null);
    assert_eq!(orphan["org_name"], Value::Null);
    let joined = items
        .iter()
        .find(|item| item["message"] == "second")
        .expect("joined row present");
    assert_eq!(joined["org_name"], "Pelagic Fuels");
    assert_eq!(joined["page"], "/app/marketplace");
    assert_eq!(items[0]["message"], "first");
}

#[tokio::test]
async fn admin_listing_totals_ties_and_pagination() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let author = Uuid::new_v4();
    seed_user(
        &harness.migrator,
        author,
        Some("BUYER"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let tied = harness.now;
    for (message, at) in [
        ("tie-a", tied),
        ("tie-b", tied),
        ("earlier", tied - chrono::Duration::seconds(30)),
    ] {
        seed_entry(&harness.migrator, Some(author), message, None, at).await;
    }
    let app = harness.app();

    let (status, body, _) = send(&app, get_admin(&harness.token, "limit=2&offset=0")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["total"], 3);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    let (status, body, _) = send(&app, get_admin(&harness.token, "limit=1&offset=2")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["total"], 3);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["message"], "earlier");

    // A page past the last row keeps the unpaged total.
    let (status, body, _) = send(&app, get_admin(&harness.token, "limit=50&offset=9")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["total"], 3);
    assert!(body["items"].as_array().unwrap().is_empty());

    let (status, body, _) = send(&app, get_admin(&harness.token, "limit=201")).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["detail"][0]["loc"], json!(["query", "limit"]));
}

#[tokio::test]
async fn admin_role_and_account_state_are_enforced() {
    let Some(harness) = harness().await else {
        return;
    };

    // A valid token for an account that is not an admin is refused.
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("BUYER"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let (status, body, _) = send(&harness.app(), get_admin(&harness.token, "")).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body, json!({"detail": "Forbidden"}));

    // An unknown account is a credential failure.
    sqlx::query("TRUNCATE users")
        .execute(&harness.migrator)
        .await
        .unwrap();
    let (status, body, headers) = send(&harness.app(), get_admin(&harness.token, "")).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body, verdaxis_read::auth::AuthError::Credentials.body());
    assert_eq!(headers["www-authenticate"], "Bearer");

    for (status_value, expected) in [
        (
            "PENDING",
            "Account is PENDING. Please wait for admin approval.",
        ),
        (
            "REJECTED",
            "Account is REJECTED. Please wait for admin approval.",
        ),
    ] {
        seed_user(
            &harness.migrator,
            harness.subject,
            Some("ADMIN"),
            status_value,
            true,
            None,
        )
        .await;
        let (status, body, _) = send(&harness.app(), get_admin(&harness.token, "")).await;
        assert_eq!(status, 403, "{body}");
        assert_eq!(body, json!({"detail": expected}));
        sqlx::query("TRUNCATE users")
            .execute(&harness.migrator)
            .await
            .unwrap();
    }

    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    sqlx::query("UPDATE users SET must_change_password = true WHERE id = $1")
        .bind(harness.subject)
        .execute(&harness.migrator)
        .await
        .unwrap();
    let (status, body, _) = send(&harness.app(), get_admin(&harness.token, "")).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body, json!({"detail": "Password change required."}));
}

#[tokio::test]
async fn submission_writes_no_other_row() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let (status, body, _) = send(
        &harness.app(),
        post_feedback(Some(&harness.token), json!({"message": "voluntary"})),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(count(&harness.migrator, "feedback_entries").await, 1);
    // No analytics call, email, outbox row, or notification is produced.
    assert_eq!(count(&harness.migrator, "market_event_outbox").await, 0);
    assert_eq!(count(&harness.migrator, "notifications").await, 0);
}

#[tokio::test]
async fn database_failure_uses_the_crud_envelope() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let (status, body, _) = send(
        &harness.app(),
        post_feedback(
            Some(&harness.token),
            json!({"message": "force-database-error"}),
        ),
    )
    .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body, json!({"detail": "Database operation failed."}));
    assert_eq!(count(&harness.migrator, "feedback_entries").await, 0);
}

#[tokio::test]
async fn ordering_and_boundaries_through_http() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let app = harness.app();

    // Malformed JSON is refused before authentication.
    let (status, body, _) = send(&app, post_raw(None, "{")).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["detail"][0]["type"], "json_invalid");

    // A missing or unknown bearer is refused before the body is validated.
    let (status, body, headers) = send(&app, post_raw(None, "{}")).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body, json!({"detail": "Not authenticated"}));
    assert_eq!(headers["www-authenticate"], "Bearer");
    let (status, _, _) = send(&app, post_raw(Some("not-a-jwt"), "{}")).await;
    assert_eq!(status, 401);

    // A valid bearer reaches body validation.
    let (status, body, _) = send(&app, post_raw(Some(&harness.token), "{}")).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["detail"][0]["loc"], json!(["body", "message"]));

    // Missing bodies still resolve authentication before required-body validation.
    let (status, _, _) = send(&app, post_raw(None, "")).await;
    assert_eq!(status, 401);

    // An empty body is a missing required body, not a JSON syntax error.
    let (status, body, _) = send(&app, post_raw(Some(&harness.token), "")).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["detail"][0]["loc"], json!(["body"]));

    // The exact paths answer the wrong-method and slash boundaries.
    let (status, _, _) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/api/feedback")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 405);
    let (status, _, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/feedback/")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", harness.token))
            .body(Body::from("{\"message\":\"x\"}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 404);

    // Market Support never becomes a mutation scope for feedback.
    let (status, body, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/feedback")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", harness.token))
            .header("x-verdaxis-market-support-context", "1")
            .body(Body::from("{\"message\":\"x\"}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        body["detail"]["code"],
        "MARKET_SUPPORT_MUTATION_NOT_ALLOWED"
    );
}

#[tokio::test]
async fn endpoint_quota_follows_validation() {
    let Some(harness) = harness().await else {
        return;
    };
    seed_user(
        &harness.migrator,
        harness.subject,
        Some("ADMIN"),
        "APPROVED",
        true,
        None,
    )
    .await;
    let app = harness.app();

    // Six rejected bodies must not consume the five-per-minute endpoint quota.
    for _ in 0..6 {
        let (status, _, _) = send(&app, post_feedback(Some(&harness.token), json!({}))).await;
        assert_eq!(status, 422);
    }
    for index in 0..5 {
        let (status, body, _) = send(
            &app,
            post_feedback(
                Some(&harness.token),
                json!({"message": format!("message {index}")}),
            ),
        )
        .await;
        assert_eq!(status, 201, "{body}");
    }
    let (status, _, _) = send(
        &app,
        post_feedback(Some(&harness.token), json!({"message": "over the limit"})),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(count(&harness.migrator, "feedback_entries").await, 5);
}
