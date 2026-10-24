//! Phase 5 Task 9: fleet, vessel and scenario compliance reads.
//!
//! The ordinary scenarios are `compliance_resource_cases` (typed resource
//! identity after authentication), `compliance_actor_cases` (own, other,
//! no-organization and admin actors plus the exact generic scenario echo),
//! `compliance_label_cases` (generic label bounds and exact echo),
//! `compliance_fleet_limits` (2000 accepted, 2001 refused, complete counts)
//! and `compliance_score_snapshot_cases` (one vessel statement, validated year
//! and mix order, and the stored-value projections). The helper-commit and
//! no-DML assertion lives in `compliance_score_helper_commit_postgres`.
//!
//! The populated tenant matrix runs only against an explicit disposable
//! PostgreSQL database; it seeds the accepted `vessels`, `users` and
//! `organizations` tables and never uses an ambient URL.

pub use verdaxis_read::*;

#[path = "../src/routes/compliance_scores.rs"]
mod scores;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
};
use chrono::{DateTime, Utc};
use routes::market_data_error;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use scores::{
    Clock, FLEET_PATH, MAX_FLEET_VESSELS, MAX_LABEL_BYTES, MAX_LABEL_SCALARS, RouteContext,
    SCENARIO_PATH, ScenarioRequest, VesselPath, VesselRow, fleet_summary, is_admin,
    routes_with_context, scenario_identity, validate_label, vessel_accessible,
};

const SECRET: &str = "phase5-task9-test-secret-minimum-32-characters";
const ISSUER: &str = "verdaxis-task9-test";
const AUDIENCE: &str = "verdaxis-task9-test";
const UNREACHABLE: &str = "postgres://fixture:fixture_pw@127.0.0.1:1/compliance_test";

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-24T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn settings() -> std::sync::Arc<auth::VerificationSettings> {
    std::sync::Arc::new(auth::VerificationSettings::new(
        SECRET.into(),
        None,
        ISSUER.into(),
        AUDIENCE.into(),
    ))
}

fn token(subject: Uuid) -> String {
    let now = now();
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

fn policy() -> MarketPolicy {
    let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
    let document: Value = serde_json::from_slice(bytes).unwrap();
    market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap()
}

fn state(database_url: &str) -> AppState {
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
    AppState::new(config, runtime::db::Pools { read, health }, policy()).unwrap()
}

fn app(database_url: &str) -> Router {
    let context = std::sync::Arc::new(RouteContext {
        settings: settings(),
        limits: auth::limits::SharedLimits::default(),
        clock: Clock::Fixed(now()),
    });
    routes_with_context(context).with_state(state(database_url))
}

fn actor(organization_id: Option<Uuid>, role: &str) -> auth::CurrentActor {
    auth::CurrentActor {
        id: Uuid::new_v4(),
        organization_id,
        role: Some(role.into()),
        email_verified: true,
    }
}

fn vessel(organization_id: Option<Uuid>, name: &str, cii_rating: Option<&str>) -> VesselRow {
    VesselRow {
        id: Uuid::new_v4(),
        name: name.into(),
        cii_rating: cii_rating.map(str::to_owned),
        organization_id,
    }
}

fn request(
    method: &str,
    path: &str,
    token: Option<&str>,
    support: bool,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if support {
        builder = builder.header("X-Verdaxis-Market-Support-Context", "synthetic-context");
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 8 << 20).await.unwrap();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON response body")
    };
    (status, headers, value)
}

fn code(value: &Value) -> &str {
    value["detail"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn compliance_resource_cases() {
    // Authentication comes first: the resource identifier is not typed, and no
    // database connection is attempted, before the bearer token is verified.
    let (status, headers, body) = send(
        &app(UNREACHABLE),
        request(
            "GET",
            "/api/compliance/vessels/not-a-uuid/score",
            None,
            false,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["www-authenticate"], "Bearer");
    assert_eq!(body, json!({"detail": "Not authenticated"}));

    // A resource path is a UUID, and a refusal names the input code.
    let id = Uuid::new_v4();
    assert_eq!(VesselPath::parse(&id.to_string()).unwrap().id(), id);
    let error = VesselPath::parse("not-a-uuid").unwrap_err();
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(code(&json!({"detail": error.detail})), "INVALID_INPUT");

    // The vessel statement is one primary-key lookup with a one-row bound.
    let sql = include_str!("../sql/compliance_vessel.sql");
    assert_eq!(sql.matches("SELECT").count(), 1);
    assert!(sql.contains("WHERE id = $1::uuid"));
    assert!(sql.contains("LIMIT 1"));
    assert_eq!(sql.matches(';').count(), 0);

    // Own, other and admin access. A caller without an organization has no
    // vessel it can read, including a vessel with no organization.
    let organization = Uuid::new_v4();
    let other_organization = Uuid::new_v4();
    let own = vessel(Some(organization), "Own", Some("C"));
    let foreign = vessel(Some(other_organization), "Foreign", Some("E"));
    let unowned = vessel(None, "Unowned", None);
    let user = actor(Some(organization), "BUYER");
    let admin = actor(Some(organization), "ADMIN");
    let no_organization = actor(None, "BUYER");
    assert!(vessel_accessible(&user, &own));
    assert!(!vessel_accessible(&user, &foreign));
    assert!(!vessel_accessible(&user, &unowned));
    assert!(vessel_accessible(&admin, &foreign));
    assert!(is_admin(&admin));
    assert!(!is_admin(&user));
    assert!(!vessel_accessible(&no_organization, &own));
    assert!(!vessel_accessible(&no_organization, &unowned));

    // A valid bearer still reaches account validation before the resource
    // identifier; this environment has no database, so it cannot be a 422.
    let (status, _, _) = send(
        &app(UNREACHABLE),
        request(
            "GET",
            "/api/compliance/vessels/not-a-uuid/score",
            Some(&token(Uuid::new_v4())),
            false,
            None,
        ),
    )
    .await;
    assert_ne!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn compliance_actor_cases() {
    // The fleet statement scopes by the actor's organization unless the role
    // is ADMIN, orders by id and fetches one sentinel row beyond the cap.
    let fleet = include_str!("../sql/compliance_fleet.sql");
    assert_eq!(fleet.matches("SELECT").count(), 1);
    assert!(fleet.contains("$1::boolean"));
    assert!(fleet.contains("$2::uuid"));
    assert!(fleet.contains("organization_id = $2::uuid"));
    assert!(fleet.contains("ORDER BY id"));
    assert!(fleet.contains("LIMIT 2001"));
    assert_eq!(fleet.matches(';').count(), 0);

    // The current account is locked for update with the scoped Phase 5
    // deadlines, for both the GET reads and the scenario POST.
    for (method, path) in [
        ("GET", FLEET_PATH),
        ("GET", "/api/compliance/vessels/{vessel_id}/score"),
    ] {
        let request = auth::AuthenticatedRequest::new(method, path, false);
        assert_eq!(request.lock_mode(), auth::UserLockMode::Update);
        assert!(request.scoped_read_timeouts());
    }
    let post = compliance::http::authenticated_request("POST", SCENARIO_PATH, false);
    assert_eq!(post.lock_mode(), auth::UserLockMode::Update);
    assert!(post.scoped_read_timeouts());

    // A scenario echoes a failed lookup exactly and never exposes another
    // organization's vessel metadata.
    let organization = Uuid::new_v4();
    let other_organization = Uuid::new_v4();
    let user = actor(Some(organization), "BUYER");
    let admin = actor(Some(organization), "ADMIN");
    let no_organization = actor(None, "BUYER");
    let own = vessel(Some(organization), "Own Vessel", Some("A"));
    let foreign = vessel(Some(other_organization), "Foreign Vessel", Some("E"));
    let unowned = vessel(None, "Unowned Vessel", None);

    let own_request = ScenarioRequest::validate(json!({
        "vessel_id": own.id.to_string(),
        "fuel_mix": {"VLSFO": "1"},
        "year": 2026,
    }))
    .unwrap();
    assert_eq!(
        scenario_identity(&own_request, Some(&own), &user),
        ("Own Vessel".to_owned(), Some("A".to_owned()))
    );
    assert_eq!(
        scenario_identity(&own_request, Some(&own), &admin),
        ("Own Vessel".to_owned(), Some("A".to_owned()))
    );

    let foreign_request = ScenarioRequest::validate(json!({
        "vessel_id": foreign.id.to_string(),
        "fuel_mix": {"VLSFO": "1"},
    }))
    .unwrap();
    assert_eq!(
        scenario_identity(&foreign_request, Some(&foreign), &user),
        (format!("Vessel {}", foreign.id), None,)
    );
    assert_eq!(
        scenario_identity(&foreign_request, Some(&foreign), &admin),
        ("Foreign Vessel".to_owned(), Some("E".to_owned()))
    );
    assert_eq!(
        scenario_identity(&own_request, Some(&own), &no_organization),
        (format!("Vessel {}", own.id), None)
    );
    assert_eq!(
        scenario_identity(
            &ScenarioRequest::validate(json!({
                "vessel_id": unowned.id.to_string(),
                "fuel_mix": {"VLSFO": "1"},
            }))
            .unwrap(),
            Some(&unowned),
            &no_organization,
        ),
        (format!("Vessel {}", unowned.id), None)
    );
    assert_eq!(
        scenario_identity(
            &ScenarioRequest::validate(json!({
                "vessel_id": "not-a-uuid",
                "fuel_mix": {"VLSFO": "1"},
            }))
            .unwrap(),
            None,
            &user,
        ),
        ("Vessel not-a-uuid".to_owned(), None)
    );
}

#[test]
fn compliance_label_cases() {
    assert!(validate_label("").is_ok());
    assert!(validate_label("not-a-uuid").is_ok());
    assert!(validate_label("  spaced label  ").is_ok());
    assert!(validate_label(&"a".repeat(MAX_LABEL_SCALARS)).is_ok());
    assert!(validate_label(&"a".repeat(MAX_LABEL_SCALARS + 1)).is_err());
    // A four-byte scalar fills the byte bound at 128 code points.
    let emoji = "🚢".repeat(MAX_LABEL_SCALARS);
    assert_eq!(emoji.len(), MAX_LABEL_BYTES);
    assert!(validate_label(&emoji).is_ok());
    assert!(validate_label(&format!("{emoji}a")).is_err());
    for control in ["\n", "\u{0}", "\u{7f}", "a\tb"] {
        assert!(validate_label(control).is_err());
    }
    let error = validate_label(&"a".repeat(MAX_LABEL_SCALARS + 1)).unwrap_err();
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(code(&json!({"detail": error.detail})), "INVALID_INPUT");

    // The generic identity keeps the submitted text byte for byte.
    for label in [
        "not-a-uuid",
        "  spaced label  ",
        "vessel-α/β",
        "URN:UUID:12345678-1234-1234-1234-123456789012",
        &"x".repeat(MAX_LABEL_SCALARS),
    ] {
        let request = ScenarioRequest::validate(json!({
            "vessel_id": label,
            "fuel_mix": {"VLSFO": "1"},
        }))
        .unwrap();
        assert_eq!(request.vessel_id(), label);
        assert_eq!(
            scenario_identity(&request, None, &actor(None, "BUYER")),
            (format!("Vessel {label}"), None)
        );
    }
}

#[test]
fn compliance_fleet_limits() {
    assert_eq!(MAX_FLEET_VESSELS, 2000);
    assert!(market_data_error::within_limit(2000, MAX_FLEET_VESSELS).is_ok());
    let error = market_data_error::within_limit(2001, MAX_FLEET_VESSELS).unwrap_err();
    assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&json!({"detail": error.detail})), "READ_RESULT_LIMIT");
    assert_eq!(error.retry_after, Some(1));
    assert_eq!(market_data_error::probe_limit(2000), 2001);

    // Counts and the average come from the same complete accepted set.
    let organization = Uuid::new_v4();
    let rows = vec![
        vessel(Some(organization), "A", Some("A")),
        vessel(Some(organization), "B", None),
    ];
    let summary = fleet_summary(&rows).unwrap();
    assert_eq!(summary["total_vessels"], json!(2));
    // Both vessels score below the good band: FuelEU 40 and unpriced ETS 50
    // dominate, so the CII contribution only moves the amber score.
    assert_eq!(summary["green_count"], json!(0));
    assert_eq!(summary["amber_count"], json!(2));
    assert_eq!(summary["red_count"], json!(0));
    assert_eq!(summary["average_score"].as_f64(), Some(51.0));
    assert_eq!(summary["vessels"].as_array().unwrap().len(), 2);
    assert_eq!(summary["vessels"][0]["vessel_id"], rows[0].id.to_string());
    assert_eq!(summary["vessels"][1]["cii"]["rating"], "C");

    let empty = fleet_summary(&[]).unwrap();
    assert_eq!(empty["total_vessels"], json!(0));
    assert_eq!(empty["average_score"].as_f64(), Some(0.0));
    assert_eq!(empty["vessels"].as_array().unwrap().len(), 0);
}

#[test]
fn compliance_score_snapshot_cases() {
    // The fleet read is one statement over the effective scope.
    let fleet = include_str!("../sql/compliance_fleet.sql");
    assert_eq!(fleet.matches("SELECT").count(), 1);
    assert_eq!(fleet.matches(';').count(), 0);

    // A generic scenario keeps the submitted identity and the default mix.
    let generic = ScenarioRequest::validate(json!({
        "vessel_id": "not-a-uuid",
        "fuel_mix": {"VLSFO": "1"},
        "year": 2026,
    }))
    .unwrap();
    let user = actor(None, "BUYER");
    let score = scores::score_scenario(&generic, None, &user).unwrap();
    assert_eq!(score["vessel_id"], "not-a-uuid");
    assert_eq!(score["vessel_name"], "Vessel not-a-uuid");
    assert_eq!(score["eu_ets"]["calculation_status"], "UNPRICED");
    assert_eq!(score["eu_ets"]["total_co2_tonnes"], Value::Null);
    assert_eq!(score["eu_ets"]["estimated_cost_eur"], Value::Null);
    assert_eq!(score["fueleu"]["target_intensity_gco2_mj"], "89.3368");
    assert_eq!(score["cii"]["rating"], "C");

    // An accessible vessel contributes its stored rating; another
    // organization's vessel still uses the generic identity.
    let organization = Uuid::new_v4();
    let own = vessel(Some(organization), "Own Vessel", Some("A"));
    let owner = actor(Some(organization), "BUYER");
    let own_request = ScenarioRequest::validate(json!({
        "vessel_id": own.id.to_string(),
        "fuel_mix": {"VLSFO": "1"},
    }))
    .unwrap();
    let own_score = scores::score_scenario(&own_request, Some(&own), &owner).unwrap();
    assert_eq!(own_score["vessel_name"], "Own Vessel");
    assert_eq!(own_score["cii"]["rating"], "A");
    let foreign_score = scores::score_scenario(&own_request, Some(&own), &user).unwrap();
    assert_eq!(foreign_score["vessel_name"], format!("Vessel {}", own.id));
    assert_eq!(foreign_score["cii"]["rating"], "C");

    // An empty stored rating is no rating, so it takes the neutral C band.
    let empty_rating = vessel(Some(organization), "Empty Rating", Some(""));
    let empty_request = ScenarioRequest::validate(json!({
        "vessel_id": empty_rating.id.to_string(),
        "fuel_mix": {"VLSFO": "1"},
    }))
    .unwrap();
    let empty_score = scores::score_scenario(&empty_request, Some(&empty_rating), &owner).unwrap();
    assert_eq!(empty_score["cii"]["rating"], "C");
    assert_eq!(empty_score["cii"]["score"], json!(60));
    let empty_fleet = fleet_summary(&[empty_rating]).unwrap();
    assert_eq!(empty_fleet["vessels"][0]["cii"]["rating"], "C");
    assert_eq!(empty_fleet["vessels"][0]["cii"]["score"], json!(60));

    // Year and mix validation order: the year is settled before the mix
    // coefficients, and the mix container bound precedes known-fuel rules.
    let year_error = ScenarioRequest::validate(json!({
        "vessel_id": "vessel",
        "fuel_mix": {"NotAFuel": "1"},
        "year": 2024,
    }))
    .unwrap_err();
    assert_eq!(code(&json!({"detail": year_error.detail})), "INVALID_INPUT");
    assert!(
        year_error.detail["message"]
            .as_str()
            .unwrap()
            .contains("year")
    );
    let mut entries = serde_json::Map::new();
    for index in 0..33 {
        entries.insert(format!("NotAFuel{index}"), json!("0"));
    }
    let container_error = ScenarioRequest::validate(json!({
        "vessel_id": "vessel",
        "fuel_mix": entries,
        "year": 2026,
    }))
    .unwrap_err();
    assert!(
        container_error.detail["message"]
            .as_str()
            .unwrap()
            .contains("32")
    );
    let unknown_fuel = ScenarioRequest::validate(json!({
        "vessel_id": "vessel",
        "fuel_mix": {"NotAFuel": "1"},
        "year": 2026,
    }))
    .unwrap_err();
    assert!(
        unknown_fuel.detail["message"]
            .as_str()
            .unwrap()
            .contains("unknown fuel")
    );
    // An unknown field is ignored, not refused.
    let extra = ScenarioRequest::validate(json!({
        "vessel_id": "vessel",
        "fuel_mix": {"VLSFO": "1"},
        "unknown_field": {"nested": true},
    }))
    .unwrap();
    assert_eq!(extra.vessel_id(), "vessel");
    // A string year is not coerced; only a JSON integer is accepted.
    for body in [
        json!({"fuel_mix": {"VLSFO": "1"}}),
        json!({"vessel_id": "vessel"}),
        json!({"vessel_id": "vessel", "fuel_mix": {"VLSFO": "1"}, "year": "2026"}),
        json!({"vessel_id": 7, "fuel_mix": {"VLSFO": "1"}}),
        json!({"vessel_id": "vessel", "fuel_mix": {"VLSFO": "1"}, "year": 2026.5}),
        json!({"vessel_id": "vessel", "fuel_mix": {"VLSFO": "1"}, "year": null}),
    ] {
        let error = ScenarioRequest::validate(body).unwrap_err();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(code(&json!({"detail": error.detail})), "INVALID_INPUT");
    }

    // The shared schedule clamps after 2050 and rejects outside its range.
    for (year, accepted) in [(2025, true), (9999, true), (10000, false), (2024, false)] {
        let result = ScenarioRequest::validate(json!({
            "vessel_id": "vessel",
            "fuel_mix": {"VLSFO": "1"},
            "year": year,
        }));
        assert_eq!(result.is_ok(), accepted, "year {year}");
    }
}

// The populated matrix needs the accepted tables. It runs only against one
// explicitly named disposable database, never an ambient URL: the fixture
// seeds its own organizations, accounts and vessels and asserts nothing about
// market data.
#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection};

    const ORGANIZATION_A: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0001);
    const ORGANIZATION_B: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0002);
    const USER_A: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0011);
    const USER_B: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0012);
    const USER_NONE: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0013);
    const USER_ADMIN: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0014);
    const VESSEL_A: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0021);
    const VESSEL_B: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0022);
    const VESSEL_UNOWNED: Uuid = Uuid::from_u128(0x9a_0000_0000_0000_0000_0000_0000_0023);

    fn urls() -> Option<(String, String)> {
        let application = std::env::var("RUST_COMPLIANCE_TEST_DATABASE_URL").ok()?;
        let writer = std::env::var("RUST_COMPLIANCE_TEST_WRITER_URL").ok()?;
        Some((application, writer))
    }

    async fn reset(connection: &mut PgConnection) {
        sqlx::query("DELETE FROM vessels WHERE id = ANY($1) OR organization_id = ANY($2)")
            .bind([VESSEL_A, VESSEL_B, VESSEL_UNOWNED].as_slice())
            .bind([ORGANIZATION_A, ORGANIZATION_B].as_slice())
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind([USER_A, USER_B, USER_NONE, USER_ADMIN].as_slice())
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("DELETE FROM organizations WHERE id = ANY($1)")
            .bind([ORGANIZATION_A, ORGANIZATION_B].as_slice())
            .execute(&mut *connection)
            .await
            .unwrap();
    }

    async fn seed_actors(connection: &mut PgConnection) {
        sqlx::query(
            "INSERT INTO organizations (id, name, type, verification_status, provenance) VALUES \
             ($1, 'Task 9 Org A', 'SHIPPING_LINE', 'APPROVED', 'REAL'), \
             ($2, 'Task 9 Org B', 'SHIPPING_LINE', 'APPROVED', 'REAL')",
        )
        .bind(ORGANIZATION_A)
        .bind(ORGANIZATION_B)
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, status, must_change_password, \
             email_verified, kyc_status, organization_id, role) VALUES \
             ($1, 'task9-a@example.test', 'synthetic', 'APPROVED', false, true, 'PENDING', $5, 'BUYER'), \
             ($2, 'task9-b@example.test', 'synthetic', 'APPROVED', false, true, 'PENDING', $6, 'BUYER'), \
             ($3, 'task9-none@example.test', 'synthetic', 'APPROVED', false, true, 'PENDING', NULL, 'BUYER'), \
             ($4, 'task9-admin@example.test', 'synthetic', 'APPROVED', false, true, 'PENDING', $5, 'ADMIN')",
        )
        .bind(USER_A)
        .bind(USER_B)
        .bind(USER_NONE)
        .bind(USER_ADMIN)
        .bind(ORGANIZATION_A)
        .bind(ORGANIZATION_B)
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO vessels (id, name, imo_number, organization_id, cii_rating) VALUES \
             ($1, 'Task 9 Vessel A', 'IMO-9A00001', $4, 'A'), \
             ($2, 'Task 9 Vessel B', 'IMO-9B00001', $5, 'E'), \
             ($3, 'Task 9 Vessel Unowned', 'IMO-9U00001', NULL, NULL)",
        )
        .bind(VESSEL_A)
        .bind(VESSEL_B)
        .bind(VESSEL_UNOWNED)
        .bind(ORGANIZATION_A)
        .bind(ORGANIZATION_B)
        .execute(&mut *connection)
        .await
        .unwrap();
    }

    async fn seeded() -> Option<(String, String)> {
        let Some((application, writer_url)) = urls() else {
            // The lane runner sets this so a missing database can never turn a
            // skipped matrix into a green gate.
            assert!(
                std::env::var_os("RUST_COMPLIANCE_REQUIRED").is_none(),
                "RUST_COMPLIANCE_REQUIRED is set but RUST_COMPLIANCE_TEST_DATABASE_URL and \
                 RUST_COMPLIANCE_TEST_WRITER_URL are not both set; the compliance score \
                 database matrix did not run"
            );
            eprintln!(
                "compliance score database matrix not run: \
                 RUST_COMPLIANCE_TEST_DATABASE_URL and RUST_COMPLIANCE_TEST_WRITER_URL are required"
            );
            return None;
        };
        let mut connection = PgConnection::connect(&writer_url).await.unwrap();
        reset(&mut connection).await;
        seed_actors(&mut connection).await;
        Some((application, writer_url))
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

    fn scenario_body(vessel_id: &str) -> Value {
        json!({"vessel_id": vessel_id, "fuel_mix": {"VLSFO": "1"}, "year": 2026})
    }

    #[tokio::test]
    async fn compliance_actor_postgres_matrix() {
        let Some((application, writer_url)) = seeded().await else {
            return;
        };
        let app = app(&application);
        let user_a = token(USER_A);
        let user_b = token(USER_B);
        let user_none = token(USER_NONE);
        let admin = token(USER_ADMIN);

        // Own organization, another organization, no organization and admin.
        let (status, _, body) =
            send(&app, request("GET", FLEET_PATH, Some(&user_a), false, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_vessels"], json!(1));
        assert_eq!(body["vessels"][0]["vessel_id"], VESSEL_A.to_string());
        let (status, _, body) =
            send(&app, request("GET", FLEET_PATH, Some(&user_b), false, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_vessels"], json!(1));
        assert_eq!(body["vessels"][0]["vessel_id"], VESSEL_B.to_string());
        let (status, _, body) = send(
            &app,
            request("GET", FLEET_PATH, Some(&user_none), false, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_vessels"], json!(0));
        assert_eq!(body["vessels"], json!([]));
        let (status, _, body) =
            send(&app, request("GET", FLEET_PATH, Some(&admin), false, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_vessels"], json!(3));

        // A resource UUID is checked after authentication: no token is 401
        // even for a non-UUID path; a valid token is 422 before any lookup.
        let score_path = format!("/api/compliance/vessels/{VESSEL_A}/score");
        let (status, _, _) = send(&app, request("GET", &score_path, None, false, None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, body) = send(
            &app,
            request(
                "GET",
                "/api/compliance/vessels/not-a-uuid/score",
                Some(&user_a),
                false,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(code(&body), "INVALID_INPUT");

        // Missing, foreign and own resources.
        let missing = Uuid::new_v4();
        let (status, _, body) = send(
            &app,
            request(
                "GET",
                &format!("/api/compliance/vessels/{missing}/score"),
                Some(&user_a),
                false,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(code(&body), "VESSEL_NOT_FOUND");
        let (status, _, body) = send(
            &app,
            request(
                "GET",
                &format!("/api/compliance/vessels/{VESSEL_B}/score"),
                Some(&user_a),
                false,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&body), "VESSEL_FORBIDDEN");
        let (status, _, body) = send(
            &app,
            request("GET", &score_path, Some(&user_a), false, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["vessel_name"], "Task 9 Vessel A");
        assert_eq!(body["cii"]["rating"], "A");
        let (status, _, _) = send(
            &app,
            request(
                "GET",
                &format!("/api/compliance/vessels/{VESSEL_B}/score"),
                Some(&admin),
                false,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, body) = send(
            &app,
            request(
                "GET",
                &format!("/api/compliance/vessels/{VESSEL_UNOWNED}/score"),
                Some(&user_none),
                false,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&body), "VESSEL_FORBIDDEN");

        // A Market Support context denies the scenario POST after
        // authentication and before the domain statement; no token stays 401.
        let (status, _, body) = send(
            &app,
            request(
                "POST",
                SCENARIO_PATH,
                Some(&user_a),
                true,
                Some(scenario_body(&VESSEL_A.to_string())),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&body), "MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
        let (status, _, _) = send(
            &app,
            request(
                "POST",
                SCENARIO_PATH,
                None,
                false,
                Some(scenario_body(&VESSEL_A.to_string())),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = send(
            &app,
            request(
                "POST",
                SCENARIO_PATH,
                Some(&user_a),
                false,
                Some(json!({
                    "vessel_id": VESSEL_A.to_string(),
                    "fuel_mix": {"VLSFO": "1"},
                    "year": 2024,
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // The generic scenario keeps the submitted identity exactly for a
        // non-UUID, a missing UUID and another organization's vessel.
        for (label, expected_name) in [
            ("not-a-uuid".to_owned(), "Vessel not-a-uuid".to_owned()),
            (missing.to_string(), format!("Vessel {missing}")),
            (VESSEL_B.to_string(), format!("Vessel {VESSEL_B}")),
        ] {
            let (status, _, body) = send(
                &app,
                request(
                    "POST",
                    SCENARIO_PATH,
                    Some(&user_a),
                    false,
                    Some(scenario_body(&label)),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{label}");
            assert_eq!(body["vessel_id"], label);
            assert_eq!(body["vessel_name"], expected_name);
            assert_eq!(body["eu_ets"]["calculation_status"], "UNPRICED");
            assert_eq!(body["eu_ets"]["estimated_cost_eur"], Value::Null);
        }
        let (status, _, body) = send(
            &app,
            request(
                "POST",
                SCENARIO_PATH,
                Some(&user_a),
                false,
                Some(scenario_body(&VESSEL_A.to_string())),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["vessel_name"], "Task 9 Vessel A");
        assert_eq!(body["cii"]["rating"], "A");

        // The current-account update lock serializes with a concurrent change.
        // The scoped 500 ms deadline refuses the waiting read as the generic
        // unavailable result, which never names the row.
        let mut blocker = PgConnection::connect(&writer_url).await.unwrap();
        let mut transaction = blocker.begin().await.unwrap();
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(USER_A)
            .execute(&mut *transaction)
            .await
            .unwrap();
        let (status, headers, body) =
            send(&app, request("GET", FLEET_PATH, Some(&user_a), false, None)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(code(&body), "READ_UNAVAILABLE");
        assert_eq!(headers["retry-after"], "1");
        transaction.rollback().await.unwrap();
        let (status, _, _) =
            send(&app, request("GET", FLEET_PATH, Some(&user_a), false, None)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn unscoped_route_keeps_its_database_mapping() {
        let Some((application, writer_url)) = seeded().await else {
            return;
        };
        // The app role carries the deployed lock deadline; the unscoped path
        // inherits it instead of setting a transaction-local one.
        let mut observed = PgConnection::connect(&application).await.unwrap();
        let lock: String = sqlx::query_scalar("SHOW lock_timeout")
            .fetch_one(&mut observed)
            .await
            .unwrap();
        assert_eq!(
            lock, "150ms",
            "the compliance matrix needs the app role's lock_timeout default"
        );

        let state = state(&application);
        let access = auth::verify_access(&token(USER_A), &settings(), now()).unwrap();
        let mut blocker = PgConnection::connect(&writer_url).await.unwrap();
        let mut transaction = blocker.begin().await.unwrap();
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(USER_A)
            .execute(&mut *transaction)
            .await
            .unwrap();
        let outcome = auth::run_authenticated(
            &state,
            access,
            auth::AuthenticatedRequest::new("GET", "/api/feedback", false),
            |_| Box::pin(async { Ok(()) }),
        )
        .await;
        transaction.rollback().await.unwrap();
        match outcome {
            Err(auth::AuthenticatedError::Database(error)) => {
                assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(error.detail, json!("Database operation failed."));
            }
            Ok(()) => panic!("an actor-lock refusal must not run the callback"),
            Err(other) => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn compliance_fleet_postgres_limits() {
        let Some((application, writer_url)) = seeded().await else {
            return;
        };
        let mut connection = PgConnection::connect(&writer_url).await.unwrap();
        sqlx::query("DELETE FROM vessels")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO vessels (id, name, imo_number, organization_id) \
             SELECT md5('task9-fleet-' || g)::uuid, 'Task 9 Fleet ' || g, \
                    'IMO-9F' || lpad(g::text, 6, '0'), $1 \
             FROM generate_series(1, 2000) g",
        )
        .bind(ORGANIZATION_A)
        .execute(&mut connection)
        .await
        .unwrap();

        // The accepted migration installed the tenant/vessel index.
        let definition: String = sqlx::query_scalar(
            "SELECT indexdef FROM pg_indexes WHERE schemaname = 'public' \
             AND indexname = 'ix_p5_vessels_org_id'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("accepted vessel index");
        assert_eq!(
            definition,
            "CREATE INDEX ix_p5_vessels_org_id ON public.vessels USING btree \
             (organization_id, id)"
        );

        let app = app(&application);
        let user_a = token(USER_A);
        let admin = token(USER_ADMIN);
        for token in [&user_a, &admin] {
            let (status, _, body) =
                send(&app, request("GET", FLEET_PATH, Some(token), false, None)).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["total_vessels"], json!(2000));
            assert_eq!(body["vessels"].as_array().unwrap().len(), 2000);
            let green = body["green_count"].as_u64().unwrap();
            let amber = body["amber_count"].as_u64().unwrap();
            let red = body["red_count"].as_u64().unwrap();
            assert_eq!(green + amber + red, 2000);
            assert!(body["average_score"].as_f64().is_some());
        }

        // One more vessel refuses the whole result in both scopes.
        sqlx::query(
            "INSERT INTO vessels (id, name, imo_number, organization_id) VALUES \
             ('9a000000-0000-0000-0000-0000000000ff', 'Task 9 Fleet 2001', \
              'IMO-9F999999', $1)",
        )
        .bind(ORGANIZATION_A)
        .execute(&mut connection)
        .await
        .unwrap();
        for token in [&user_a, &admin] {
            let (status, _, body) =
                send(&app, request("GET", FLEET_PATH, Some(token), false, None)).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(code(&body), "READ_RESULT_LIMIT");
            assert!(body.get("vessels").is_none());
        }
    }

    #[tokio::test]
    /// The helper commits its read-only transaction and adds no business,
    /// audit, ledger or outbox row.
    async fn compliance_score_helper_commit_postgres() {
        let Some((application, writer_url)) = seeded().await else {
            return;
        };
        let app = app(&application);
        let user_a = token(USER_A);
        let mut observer = PgConnection::connect(&writer_url).await.unwrap();
        let before = business_counts(&mut observer).await;

        let (status, _, _) =
            send(&app, request("GET", FLEET_PATH, Some(&user_a), false, None)).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(
            &app,
            request(
                "GET",
                &format!("/api/compliance/vessels/{VESSEL_A}/score"),
                Some(&user_a),
                false,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        for body in [
            scenario_body(&VESSEL_A.to_string()),
            scenario_body("not-a-uuid"),
        ] {
            let (status, _, _) = send(
                &app,
                request("POST", SCENARIO_PATH, Some(&user_a), false, Some(body)),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        assert_eq!(
            before,
            business_counts(&mut observer).await,
            "the authenticated helper commit adds no business, audit or outbox row"
        );
    }
}
