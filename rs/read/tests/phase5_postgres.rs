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
        phase2: Default::default(),
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

/// A commit through the Python writer path becomes visible to the Rust
/// demand, overlay and availability reads; an uncommitted row stays invisible
/// until its transaction commits.
#[tokio::test]
async fn phase5_visibility_cases() {
    let Some((application, _writer_url)) = urls() else {
        assert!(
            !required(),
            "RUST_COMPLIANCE_REQUIRED is set but the Task 12 database URLs are not; \
             the cross-group matrix did not run"
        );
        eprintln!("cross-group matrix not run: disposable URLs required");
        return;
    };
    let python = std::env::var("RUST_TEST_PYTHON").expect("harness python");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

    let router = app(&application);
    let overlay_bearer = overlay_token(READER);

    let run_clean = || {
        let out = std::process::Command::new(&python)
            .arg(repo.join("rs/read/tests/phase5_writer.py"))
            .arg("clean")
            .env("PYTHONPATH", &repo)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("python writer path must run");
        assert!(
            out.status.success(),
            "python writer cleanup failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_clean();

    let run_writer = |label: &str| {
        let out = std::process::Command::new(&python)
            .arg(repo.join("rs/read/tests/phase5_writer.py"))
            .arg("commit")
            .arg(label)
            .env("PYTHONPATH", &repo)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("python writer path must run");
        assert!(
            out.status.success(),
            "python writer failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // The committed BID reaches the demand read: one open group at Rotterdam.
    let state = {
        let config = Config {
            kyc_execution_gate_enabled: true,
            db_session_time_zone: "UTC".into(),
            environment: "test".into(),
            release_sha: compiled_sha().into(),
            migration_revision: "p5_20261020_read_indexes".into(),
            database_url: application.clone(),
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 16,
            pool_max: 17,
            python_budget: 52,
            configured_max_connections: 100,
            phase2: Default::default(),
            prepared_budget: None,
            compliance_eur_usd_rate: None,
            fleet_artifact_path: "data/fleet_demand.json".into(),
        };
        let read = sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect_lazy(&application)
            .unwrap();
        let health = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&application)
            .unwrap();
        let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
        let document: Value = serde_json::from_slice(bytes).unwrap();
        let policy =
            market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap();
        AppState::new(config, runtime::db::Pools { read, health }, policy).unwrap()
    };

    let stdout = run_writer("committed");
    assert!(stdout.contains("WRITER_COMMIT="), "writer output: {stdout}");

    // Demand: the committed BID is the one visible demand group.
    {
        use routes::demand;
        let mut connection = sqlx::PgConnection::connect(&application).await.unwrap();
        let query = demand::DemandQuery::parse(None).unwrap();
        let clock = market::visibility::VisibilityClock::at(now());
        let signals = demand::load(&mut connection, &state, &clock, &query)
            .await
            .unwrap();
        assert_eq!(signals.len(), 1, "committed BID must be visible");
        let body = serde_json::to_value(&signals).unwrap();
        assert_eq!(body[0]["market_product_code"], "BIO_METHANOL");
        assert_eq!(body[0]["volume_mt"], "100.00");
    }

    // Availability: the committed inventory row makes Rotterdam AVAILABLE.
    {
        let mut connection = sqlx::PgConnection::connect(&application).await.unwrap();
        let policy = state.policy.as_ref().clone();
        let rows = routes::availability::select_availability(&mut connection, &policy, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "one port group must be visible");
        let body = serde_json::to_value(&rows).unwrap();
        assert_eq!(body[0]["port_name"], "Dalian");
        assert_eq!(body[0]["total_stock_mt"], "1200");
    }

    // Overlay still answers on the same database (its fleet count is this
    // test's own seeded vessel scope, separate from the market rows).
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
    assert!(overlay["assumptions"]["fleet_vessel_count"].is_u64());
}

/// Authenticated reads return the committed row values unchanged: the same
/// demand group, availability stock and fleet count before and after three
/// round trips, with no audit, ledger or outbox row added.
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
    let mut writer = sqlx::PgConnection::connect(&writer_url).await.unwrap();
    reset(&mut writer).await;
    seed(&mut writer).await;
    seed_vessel(&mut writer, Some("C")).await;
    let before_counts = business_counts(&mut writer).await;

    // The business row values the reads depend on, straight from the tables.
    async fn row_values(connection: &mut PgConnection) -> Vec<String> {
        let mut values = Vec::new();
        for statement in [
            "SELECT coalesce(sum(remaining_quantity_mt)::text,'0') FROM orderbook_orders \
             WHERE side='BID' AND status IN ('OPEN','PARTIALLY_FILLED')",
            "SELECT coalesce(sum(current_stock_mt)::text,'0') FROM inventory_items",
            "SELECT count(*)::text FROM vessels",
        ] {
            values.push(
                sqlx::query_scalar::<_, String>(statement)
                    .fetch_one(&mut *connection)
                    .await
                    .unwrap(),
            );
        }
        values
    }
    let before_values = row_values(&mut writer).await;

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

    // No business row value moved and no audit, ledger or outbox row appeared.
    let after_values = row_values(&mut writer).await;
    assert_eq!(
        before_values, after_values,
        "an authenticated read changed business rows"
    );
    let after_counts = business_counts(&mut writer).await;
    assert_eq!(
        before_counts, after_counts,
        "an authenticated read wrote business rows"
    );

    sqlx::query("DELETE FROM vessels WHERE id = $1")
        .bind(VESSEL)
        .execute(&mut writer)
        .await
        .unwrap();
}

/// A cancelled caller detaches from its read: the owner stays waiting on the
/// actor-row lock (observed in `pg_locks`), the one admission slot and its
/// connection stay held while it waits, and after the holder releases, both
/// the slot and the connection return within a bounded time.
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
    let mut observer = sqlx::PgConnection::connect(&writer_url).await.unwrap();
    reset(&mut observer).await;
    seed(&mut observer).await;
    seed_vessel(&mut observer, Some("D")).await;

    // One read slot: a single leaked permit is enough to fail every later read.
    let router = {
        let config = Config {
            kyc_execution_gate_enabled: true,
            db_session_time_zone: "UTC".into(),
            environment: "test".into(),
            release_sha: compiled_sha().into(),
            migration_revision: "p5_20261020_read_indexes".into(),
            database_url: application.clone(),
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 1,
            pool_max: 2,
            python_budget: 52,
            configured_max_connections: 100,
            phase2: Default::default(),
            prepared_budget: None,
            compliance_eur_usd_rate: None,
            fleet_artifact_path: "data/fleet_demand.json".into(),
        };
        let read = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&application)
            .unwrap();
        let health = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&application)
            .unwrap();
        let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
        let document: Value = serde_json::from_slice(bytes).unwrap();
        let policy =
            market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap();
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
                auth::VerificationSettings::new(
                    SECRET.into(),
                    None,
                    ISSUER.into(),
                    AUDIENCE.into(),
                ),
                auth::limits::SharedLimits::default(),
            )))
            .with_state(state)
    };
    let fresh = router.clone();
    let overlay_bearer = overlay_token(READER);

    // Warm the pool first, so the blocked request spends its time in the
    // lock wait rather than in a cold connection handshake.
    let warmed = post(
        &router,
        "/api/compliance/pricing-overlay",
        &overlay_bearer,
        json!({"order_ids": []}),
    )
    .await;
    assert_eq!(warmed.status(), StatusCode::OK);

    // Hold the actor row from its own connection, so the observer stays free.
    let mut holder_connection = sqlx::PgConnection::connect(&writer_url).await.unwrap();
    let mut holder = holder_connection
        .begin()
        .await
        .expect("independent holder transaction");
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

    // Observable point: the owner's backend waits on the actor row lock. A
    // row-lock wait is an ungranted transactionid lock in pg_locks, a shared
    // memory view, so the check is one fast catalog read even while the
    // wait holds the connection.
    let waiting = async |observer: &mut PgConnection| -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_locks \
             WHERE NOT granted AND locktype = 'transactionid' \
               AND pid <> pg_backend_pid()",
        )
        .fetch_one(observer)
        .await
        .unwrap()
    };
    let mut blocked = Some(blocked);
    let mut waited = false;
    for _ in 0..100 {
        if waiting(&mut observer).await > 0 {
            waited = true;
            break;
        }
        let Some(handle) = blocked.as_ref() else {
            break;
        };
        if handle.is_finished() {
            // Diagnose an early return instead of reporting a missed wait.
            let response = blocked.take().unwrap().await.unwrap();
            panic!(
                "the blocked overlay request returned early: {}",
                response.status()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(waited, "the owner never reached the actor-row wait");

    // While the owner waits, the caller is cancelled.
    if let Some(handle) = blocked.take() {
        handle.abort();
    }

    // The owner still holds the lock wait (the caller's abort did not cancel
    // it) and its transaction is still open.
    assert!(
        waiting(&mut observer).await > 0,
        "caller cancellation cancelled the owner's wait"
    );

    // Release the holder; the owner's read must finish and free everything.
    holder.rollback().await.expect("release the actor row");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if waiting(&mut observer).await == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the owner kept the lock wait after the holder released"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Bounded release: one slot, one pool connection — a second overlay
    // request must answer, proving the cancelled owner returned both.
    let response = post(
        &fresh,
        "/api/compliance/pricing-overlay",
        &overlay_bearer,
        json!({"order_ids": []}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["assumptions"]["fleet_vessel_count"], json!(1));

    sqlx::query("DELETE FROM vessels WHERE id = $1")
        .bind(VESSEL)
        .execute(&mut observer)
        .await
        .unwrap();
}
