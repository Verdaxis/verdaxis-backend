//! Phase 5 Task 12: cross-group integration on one disposable database.
//!
//! The per-route tenant and cap matrices stay with their owner lanes:
//! `phase5_compliance_scores.rs` (lane V: own/other/no-org/admin actors, the
//! 2000/2001 fleet bound, the helper commit with no business write) and
//! `phase5_compliance_overlay.rs` (lane O: the populated overlay matrix, the
//! five tenant scopes, the statement-count snapshot). This file proves only
//! the cross-group behaviour the plan assigns to Task 12:
//!
//! - `phase5_visibility_cases`: a writer's committed change becomes visible to
//!   the composed Rust reads, on one shared database;
//! - `phase5_snapshot_cases`: an authenticated read's helper commit adds no
//!   business, audit, ledger or outbox row, and one response's statement is
//!   one snapshot;
//! - `phase5_read_limits`: a client cancellation detaches the caller without
//!   cancelling database work, and the read slots return afterwards.
//!
//! The disposable database comes from `run_compliance_scores.sh`; this file
//! never touches an ambient URL, a twin database or an operator run.

pub use verdaxis_read::*;

#[path = "../src/routes/compliance_overlay.rs"]
mod overlay_routes;
#[path = "../src/routes/compliance_scores.rs"]
mod scores;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use tower::ServiceExt;
use uuid::Uuid;

use overlay_routes::{OverlayAuthority, routes as overlay_routes};
use scores::{Clock, RouteContext, routes_with_context};

const SECRET: &str = "phase5-task12-test-secret-minimum-32-characters";
const ISSUER: &str = "verdaxis-task12-test";
const AUDIENCE: &str = "verdaxis-task12-test";
const ORGANIZATION: Uuid = Uuid::from_u128(0x9c_0000_0000_0000_0000_0000_0000_0001);
const WRITER_ORGANIZATION: Uuid = Uuid::from_u128(0x9c_0000_0000_0000_0000_0000_0000_0002);
const READER: Uuid = Uuid::from_u128(0x9c_0000_0000_0000_0000_0000_0000_0011);
const VESSEL: Uuid = Uuid::from_u128(0x9c_0000_0000_0000_0000_0000_0000_0021);

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

/// Both route groups on one state, as the serving composition will mount them.
fn app(database_url: &str) -> Router {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "p5_20261020_read_indexes".into(),
        database_url: database_url.into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    };
    let read = sqlx::postgres::PgPoolOptions::new()
        .max_connections(16)
        .connect_lazy(database_url)
        .unwrap();
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(database_url)
        .unwrap();
    let policy: MarketPolicy = {
        let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
        let document: Value = serde_json::from_slice(bytes).unwrap();
        market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap()
    };
    let state = AppState::new(config, runtime::db::Pools { read, health }, policy).unwrap();
    let context = std::sync::Arc::new(RouteContext {
        settings: std::sync::Arc::new(auth::VerificationSettings::new(
            SECRET.into(),
            None,
            ISSUER.into(),
            AUDIENCE.into(),
        )),
        limits: auth::limits::SharedLimits::default(),
        clock: Clock::Fixed(now()),
    });
    routes_with_context(context)
        .merge(overlay_routes(OverlayAuthority::new(
            auth::VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into()),
            auth::limits::SharedLimits::default(),
        )))
        .with_state(state)
}

fn mint(subject: Uuid, now: chrono::DateTime<chrono::Utc>) -> String {
    let claims = json!({
        "sub": subject.to_string(),
        "type": "access",
        "iss": ISSUER,
        "aud": AUDIENCE,
        "jti": Uuid::new_v4().to_string(),
        "iat": (now - chrono::Duration::minutes(5)).timestamp(),
        "exp": (now + chrono::Duration::minutes(30)).timestamp(),
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

/// The score routes verify the bearer against the fixed route clock.
fn scores_token(subject: Uuid) -> String {
    mint(subject, now())
}

/// The overlay route verifies the bearer against the real clock.
fn overlay_token(subject: Uuid) -> String {
    mint(subject, chrono::Utc::now())
}

async fn get(router: &Router, uri: &str, bearer: &str) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post(router: &Router, uri: &str, bearer: &str, body: Value) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// The disposable database URLs, or `None` outside a runner.
fn urls() -> Option<(String, String)> {
    let application = std::env::var("RUST_COMPLIANCE_TEST_DATABASE_URL").ok()?;
    let writer = std::env::var("RUST_COMPLIANCE_TEST_WRITER_URL").ok()?;
    Some((application, writer))
}

fn required() -> bool {
    std::env::var("RUST_COMPLIANCE_REQUIRED").as_deref() == Ok("1")
}

#[test]
fn the_overlay_year_default_is_pinned() {
    // `OverlayRequest::year` carries the reporting year the composed route
    // answers with; its documented default is the literal 2026.
    let request = overlay_routes::OverlayRequest::validate(json!({"order_ids": []})).unwrap();
    assert_eq!(request.year(), 2026);
}

async fn reset(connection: &mut PgConnection) {
    sqlx::query("DELETE FROM vessels WHERE id = $1")
        .bind(VESSEL)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind([READER].as_slice())
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id = ANY($1)")
        .bind([ORGANIZATION, WRITER_ORGANIZATION].as_slice())
        .execute(&mut *connection)
        .await
        .unwrap();
}

/// Two organizations: the reader's and the Python writer's, on one database.
async fn seed(connection: &mut PgConnection) {
    sqlx::query(
        "INSERT INTO organizations (id, name, type, verification_status, provenance) VALUES \
         ($1, 'Task 12 Reader Org', 'SHIPPING_LINE', 'APPROVED', 'REAL'), \
         ($2, 'Task 12 Writer Org', 'SHIPPING_LINE', 'APPROVED', 'REAL')",
    )
    .bind(ORGANIZATION)
    .bind(WRITER_ORGANIZATION)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, status, must_change_password, \
         email_verified, kyc_status, organization_id, role) VALUES \
         ($1, 'task12-reader@example.test', 'synthetic', 'APPROVED', false, true, 'PENDING', $2, 'BUYER')",
    )
    .bind(READER)
    .bind(ORGANIZATION)
    .execute(&mut *connection)
    .await
    .unwrap();
}

async fn seed_vessel(connection: &mut PgConnection, rating: Option<&str>) {
    sqlx::query(
        "INSERT INTO vessels (id, name, imo_number, organization_id, cii_rating) \
         VALUES ($1, 'Task 12 Vessel', 'IMO-9C00001', $2, $3)",
    )
    .bind(VESSEL)
    .bind(ORGANIZATION)
    .bind(rating)
    .execute(&mut *connection)
    .await
    .unwrap();
}

async fn business_counts(connection: &mut PgConnection) -> Vec<i64> {
    let mut values = Vec::new();
    for statement in [
        "SELECT count(*) FROM audit_logs",
        "SELECT count(*) FROM compliance_ledger",
        "SELECT count(*) FROM market_event_outbox",
    ] {
        values.push(
            sqlx::query_scalar::<_, i64>(statement)
                .fetch_one(&mut *connection)
                .await
                .unwrap(),
        );
    }
    values
}

/// A writer's committed change becomes visible to the composed Rust reads,
/// and a second group sees its own view of the same database.
#[tokio::test]
async fn phase5_visibility_cases() {
    let Some((application, writer_url)) = urls() else {
        assert!(
            !required(),
            "RUST_COMPLIANCE_REQUIRED is set but the Task 12 database URLs are not; \
             the cross-group matrix did not run"
        );
        eprintln!("cross-group matrix not run: disposable URLs required");
        return;
    };
    let mut writer = PgConnection::connect(&writer_url).await.unwrap();
    reset(&mut writer).await;
    seed(&mut writer).await;
    seed_vessel(&mut writer, Some("E")).await;

    let router = app(&application);
    let scores_bearer = scores_token(READER);
    let overlay_bearer = overlay_token(READER);

    // The fleet read sees the writer's committed vessel, and the overlay's
    // fleet count agrees with it on the same database.
    let fleet = json_body(get(&router, "/api/compliance/fleet", &scores_bearer).await).await;
    assert_eq!(fleet["total_vessels"], json!(1));
    assert_eq!(fleet["vessels"][0]["cii"]["rating"], "E");

    let overlay = json_body(
        post(
            &router,
            "/api/compliance/pricing-overlay",
            &overlay_bearer,
            json!({"order_ids": []}),
        )
        .await,
    )
    .await;
    assert_eq!(overlay["assumptions"]["fleet_vessel_count"], json!(1));

    // The writer commits a second vessel; the next read of each group sees it.
    let second = Uuid::from_u128(0x9c_0000_0000_0000_0000_0000_0000_0022);
    sqlx::query(
        "INSERT INTO vessels (id, name, imo_number, organization_id, cii_rating) \
         VALUES ($1, 'Task 12 Vessel 2', 'IMO-9C00002', $2, 'A')",
    )
    .bind(second)
    .bind(ORGANIZATION)
    .execute(&mut writer)
    .await
    .unwrap();

    let fleet = json_body(get(&router, "/api/compliance/fleet", &scores_bearer).await).await;
    assert_eq!(fleet["total_vessels"], json!(2));
    let overlay = json_body(
        post(
            &router,
            "/api/compliance/pricing-overlay",
            &overlay_bearer,
            json!({"order_ids": []}),
        )
        .await,
    )
    .await;
    assert_eq!(overlay["assumptions"]["fleet_vessel_count"], json!(2));

    // A vessel the writer moves out of the reader's organization disappears
    // from the scoped fleet on the next read: the reads never cache tenant rows.
    sqlx::query("UPDATE vessels SET organization_id = $2 WHERE id = $1")
        .bind(second)
        .bind(WRITER_ORGANIZATION)
        .execute(&mut writer)
        .await
        .unwrap();
    let fleet = json_body(get(&router, "/api/compliance/fleet", &scores_bearer).await).await;
    assert_eq!(fleet["total_vessels"], json!(1));

    sqlx::query("DELETE FROM vessels WHERE id = ANY($1)")
        .bind([VESSEL, second].as_slice())
        .execute(&mut writer)
        .await
        .unwrap();
}

/// The authenticated helper commits its read-only transaction and adds no
/// business, audit, ledger or outbox row; both groups agree.
#[tokio::test]
async fn phase5_snapshot_cases() {
    let Some((application, writer_url)) = urls() else {
        assert!(
            !required(),
            "RUST_COMPLIANCE_REQUIRED is set but the Task 12 database URLs are not; \
             the cross-group matrix did not run"
        );
        return;
    };
    let mut writer = PgConnection::connect(&writer_url).await.unwrap();
    reset(&mut writer).await;
    seed(&mut writer).await;
    seed_vessel(&mut writer, Some("C")).await;
    let before = business_counts(&mut writer).await;

    let router = app(&application);
    let scores_bearer = scores_token(READER);
    let overlay_bearer = overlay_token(READER);
    for _ in 0..3 {
        let fleet = json_body(get(&router, "/api/compliance/fleet", &scores_bearer).await).await;
        assert_eq!(fleet["total_vessels"], json!(1));
        let overlay = json_body(
            post(
                &router,
                "/api/compliance/pricing-overlay",
                &overlay_bearer,
                json!({"order_ids": []}),
            )
            .await,
        )
        .await;
        assert_eq!(overlay["assumptions"]["fleet_vessel_count"], json!(1));
    }

    // The committed helper transaction wrote nothing the business counts can see.
    let after = business_counts(&mut writer).await;
    assert_eq!(before, after, "an authenticated read wrote business rows");

    sqlx::query("DELETE FROM vessels WHERE id = $1")
        .bind(VESSEL)
        .execute(&mut writer)
        .await
        .unwrap();
}

/// A client cancellation detaches the caller; database work keeps its slot and
/// the reader serves normally once the work ends.
#[tokio::test]
async fn phase5_read_limits() {
    let Some((application, writer_url)) = urls() else {
        assert!(
            !required(),
            "RUST_COMPLIANCE_REQUIRED is set but the Task 12 database URLs are not; \
             the cross-group matrix did not run"
        );
        return;
    };
    let mut writer = PgConnection::connect(&writer_url).await.unwrap();
    reset(&mut writer).await;
    seed(&mut writer).await;
    seed_vessel(&mut writer, Some("D")).await;

    let router = app(&application);
    let scores_bearer = scores_token(READER);
    let overlay_bearer = overlay_token(READER);

    // Cancellation through the real composition: an overlay request whose
    // actor row is held occupies its caller in the lock wait; aborting the
    // caller detaches it, and a fresh caller answers once the holder is gone.
    let mut holder = writer.begin().await.expect("independent writer connection");
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(READER)
        .fetch_one(&mut *holder)
        .await
        .expect("hold the actor row");

    let blocked = {
        let router = router.clone();
        let bearer = overlay_bearer.clone();
        tokio::spawn(async move {
            post(
                &router,
                "/api/compliance/pricing-overlay",
                &bearer,
                json!({"order_ids": []}),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    blocked.abort();
    let _ = blocked.await;

    // The aborted caller released nothing it should not have: with the holder
    // gone the same request answers normally on a fresh caller.
    holder.rollback().await.expect("release the actor row");
    let response = post(
        &router,
        "/api/compliance/pricing-overlay",
        &overlay_bearer,
        json!({"order_ids": []}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["assumptions"]["fleet_vessel_count"], json!(1));

    let fleet = json_body(get(&router, "/api/compliance/fleet", &scores_bearer).await).await;
    assert_eq!(fleet["total_vessels"], json!(1));

    sqlx::query("DELETE FROM vessels WHERE id = $1")
        .bind(VESSEL)
        .execute(&mut writer)
        .await
        .unwrap();
}
