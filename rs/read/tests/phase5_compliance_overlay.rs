//! Pricing-overlay route: pure projection, HTTP order and populated PostgreSQL.
//!
//! `compliance_overlay_cases`, `compliance_overlay_limits` and the catalog case
//! are pure. `compliance_overlay_http_cases` drives the real route through its
//! middleware without a database: the body bound, the bearer, the Market
//! Support refusal and the method answers are decided before any domain work.
//! The `#[cfg(feature = "contract-tests")]` cases use the disposable market
//! fixture that `run_market_contract.sh` seeds: the authenticated order and
//! scoped fleet counts, the 500 ms actor-lock and 5 s statement deadlines, the
//! one-statement snapshot, the real 5 s statement deadline and the accepted
//! read index. That runner sets `RUST_OVERLAY_REQUIRED=1`, so a missing fixture
//! refuses instead of passing empty; `run_postgres.sh` names all five populated
//! cases in its skip list, so the foundation suite reports them as skipped.

pub use verdaxis_read::{
    ApiError, ApiErrorKind, AppState, Config, MarketPolicy, auth, market, routes, runtime,
};

#[path = "../src/compliance/pricing.rs"]
pub mod pricing;
mod compliance {
    pub use crate::pricing;
    pub use verdaxis_read::compliance::http;
}
#[path = "../src/routes/compliance_overlay.rs"]
mod overlay;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use overlay::{OverlayAuthority, OverlayRequest, alcohol_family, routes as overlay_routes};
use pricing::AlcoholFamily;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_SECRET: &str = "phase5-overlay-http-test-secret-minimum-32-chars";
const TEST_ISSUER: &str = "verdaxis-test-api";
const TEST_AUDIENCE: &str = "verdaxis-test-web";

fn fixture_policy() -> MarketPolicy {
    let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
    let document: Value = serde_json::from_slice(bytes).unwrap();
    market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap()
}

fn settings() -> auth::VerificationSettings {
    auth::VerificationSettings::new(
        TEST_SECRET.into(),
        None,
        TEST_ISSUER.into(),
        TEST_AUDIENCE.into(),
    )
}

fn test_config(
    database_url: String,
    compliance_eur_usd_rate: Option<rust_decimal::Decimal>,
) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url,
        db_session_time_zone: "Asia/Singapore".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    }
}

/// A state whose read pool never connects: every request must be refused by
/// the HTTP order before it can reach a domain statement.
fn offline_state() -> AppState {
    let config = test_config(
        "postgres://offline_app:offline_password@127.0.0.1:5432/offline_test".into(),
        None,
    );
    let read = sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.read_admission_slots)
        .connect_lazy(&config.database_url)
        .unwrap();
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&config.database_url)
        .unwrap();
    AppState::new(
        config,
        runtime::db::Pools { read, health },
        fixture_policy(),
    )
    .unwrap()
}

fn authority() -> OverlayAuthority {
    OverlayAuthority::new(settings(), auth::limits::SharedLimits::default())
}

fn overlay_app(state: AppState) -> axum::Router {
    overlay_routes(authority()).with_state(state)
}

async fn post(app: axum::Router, body: Body, bearer: Option<&str>, support: bool) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(compliance::http::OVERLAY_PATH);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if support {
        builder = builder.header("x-verdaxis-market-support-context", "overlay-case");
    }
    app.oneshot(builder.body(body).unwrap()).await.unwrap()
}

async fn get(app: axum::Router, uri: &str) -> Response {
    app.oneshot(
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
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

#[test]
fn compliance_overlay_catalog_cases() {
    let policy = fixture_policy();
    for product in &policy.products {
        let expected = match product.fuel_type.as_str() {
            "Methanol" => Some(AlcoholFamily::Methanol),
            "Ethanol" => Some(AlcoholFamily::Ethanol),
            _ => None,
        };
        assert_eq!(alcohol_family(&product.fuel_type), expected);
    }
    for label in ["methanol", "METHANOL", "Bio-Methanol", "", "unknown"] {
        assert_eq!(alcohol_family(label), None);
    }
}

#[test]
fn compliance_overlay_limits() {
    let id = Uuid::new_v4();
    let request = OverlayRequest::validate(json!({"order_ids": [id, id]})).unwrap();
    assert_eq!(request.order_ids(), &[id]);
    assert_eq!(request.year(), 2026);
    assert!(OverlayRequest::validate(json!({"order_ids": []})).is_ok());
    assert!(OverlayRequest::validate(json!({"order_ids": vec![id; 100]})).is_ok());
    assert!(OverlayRequest::validate(json!({"order_ids": vec![id; 101]})).is_err());
    for value in [
        json!({"order_ids": ["not-a-uuid"]}),
        json!({"order_ids": [], "year": 2024}),
        json!({"order_ids": [], "year": 10000}),
        json!({"order_ids": [], "year": 2026.5}),
        json!({"order_ids": [], "extra": true}),
        json!({}),
    ] {
        assert!(OverlayRequest::validate(value).is_err());
    }
    for year in [2025, 9999] {
        assert!(OverlayRequest::validate(json!({"order_ids": [], "year": year})).is_ok());
    }
}

#[test]
fn compliance_overlay_cases() {
    let _ = overlay::select_overlay;
    use overlay::{OverlayRow, project};
    use rust_decimal::Decimal;
    let first = Uuid::new_v4();
    let missing = Uuid::new_v4();
    let request = OverlayRequest::validate(json!({"order_ids": [first, missing]})).unwrap();
    let response = project(
        &request,
        vec![
            OverlayRow {
                requested_id: Some(first),
                fuel_type: Some("Methanol".into()),
                ci: Some(Decimal::from(31)),
                lcv: None,
                fleet_vessel_count: 7,
            },
            OverlayRow {
                requested_id: Some(missing),
                fuel_type: None,
                ci: None,
                lcv: None,
                fleet_vessel_count: 7,
            },
        ],
        None,
    )
    .unwrap();
    assert_eq!(response.overlays.len(), 2);
    assert!(response.overlays[&missing].is_none());
    let comparison = response.overlays[&first].as_ref().unwrap();
    assert_eq!(comparison.lcv_basis, pricing::ValueBasis::ProductDefault);
    assert_eq!(comparison.penalty_avoided_usd_per_mt, Decimal::ZERO);
    assert_eq!(
        comparison.financial_benefit_status,
        pricing::FinancialBenefitStatus::Unpriced
    );
    assert_eq!(response.assumptions.fleet_vessel_count, 7);

    // Python serializes the two legacy penalty fields from `Decimal("0.00")`,
    // so the client reads a two-decimal string, not `"0"`.
    let value = serde_json::to_value(&response).expect("overlay response");
    for key in ["penalty_avoided_eur_per_mt", "penalty_avoided_usd_per_mt"] {
        assert_eq!(
            value["overlays"][&first.to_string()][key],
            "0.00",
            "{key} keeps Python's spelling"
        );
    }

    let empty = OverlayRequest::validate(json!({"order_ids": []})).unwrap();
    let response = project(
        &empty,
        vec![OverlayRow {
            requested_id: None,
            fuel_type: None,
            ci: None,
            lcv: None,
            fleet_vessel_count: 7,
        }],
        None,
    )
    .unwrap();
    assert!(response.overlays.is_empty());
    assert_eq!(response.assumptions.fleet_vessel_count, 7);
}

/// The configured exchange rate reaches every response, and an absent override
/// keeps the pricing core's published default.
#[test]
fn compliance_overlay_fx_cases() {
    use overlay::{OverlayRow, project};
    use rust_decimal::Decimal;
    let request = OverlayRequest::validate(json!({"order_ids": []})).unwrap();
    let rows = || {
        vec![OverlayRow {
            requested_id: None,
            fuel_type: None,
            ci: None,
            lcv: None,
            fleet_vessel_count: 0,
        }]
    };
    let configured = project(&request, rows(), Some(Decimal::new(110, 2))).unwrap();
    assert_eq!(configured.assumptions.eur_usd_rate, Decimal::new(110, 2));
    assert_eq!(
        serde_json::to_value(&configured).unwrap()["assumptions"]["eur_usd_rate"],
        "1.10"
    );
    let default = project(&request, rows(), None).unwrap();
    assert_eq!(
        serde_json::to_value(&default).unwrap()["assumptions"]["eur_usd_rate"],
        "1.08"
    );
    // The core still refuses a nonpositive configured rate.
    assert!(project(&request, rows(), Some(Decimal::ZERO)).is_err());
}

/// The runner guard: the market-contract runner states that it carries the
/// populated matrix, and then a missing URL is a failure, not a skip.
#[cfg(feature = "contract-tests")]
#[test]
fn compliance_overlay_fixture_guard() {
    assert_eq!(
        fixture_target(FixtureState {
            url: Some("postgres://fixture".into()),
            required: false,
            manifest: false,
        }),
        Some("postgres://fixture".into())
    );
    assert_eq!(
        fixture_target(FixtureState {
            url: None,
            required: false,
            manifest: false,
        }),
        None
    );
    // A manifest without its URL is a broken market-contract invocation.
    for manifest in [false, true] {
        let refused = std::panic::catch_unwind(|| {
            fixture_target(FixtureState {
                url: None,
                required: true,
                manifest,
            })
        });
        assert!(refused.is_err(), "a required runner must not skip");
    }
    let refused = std::panic::catch_unwind(|| {
        fixture_target(FixtureState {
            url: None,
            required: false,
            manifest: true,
        })
    });
    assert!(refused.is_err(), "a manifest without its URL must not skip");
}

#[test]
fn compliance_overlay_statement_shape() {
    let sql = include_str!("../sql/compliance_overlay.sql");
    assert!(sql.contains("requested_ids"));
    assert!(sql.contains("fleet_count"));
    assert!(sql.contains("LEFT JOIN eligible_orders"));
    assert_eq!(sql.matches(';').count(), 0);
}

/// The HTTP order the client sees, with no database reachable.
#[tokio::test]
async fn compliance_overlay_http_cases() {
    let invalid = serde_json::to_vec(&json!({"order_ids": ["not-a-uuid"], "year": 1999})).unwrap();

    // A syntactically valid body with invalid semantics is a 401 without a
    // bearer: no typed rule runs first.
    let response = post(
        overlay_app(offline_state()),
        Body::from(invalid.clone()),
        None,
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["www-authenticate"], "Bearer");
    assert_eq!(json_body(response).await["detail"], "Not authenticated");

    // The accepted middleware refuses a Market Support POST at the boundary;
    // the route never runs, and the refusal keeps the typed envelope.
    let response = runtime::http::middleware(overlay_app(offline_state()))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(compliance::http::OVERLAY_PATH)
                .header("x-verdaxis-market-support-context", "overlay-case")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(response).await["detail"]["code"],
        "MARKET_SUPPORT_MUTATION_NOT_ALLOWED"
    );

    // The raw 101-ID bound is a semantic rule: an anonymous caller is refused
    // by authentication, not by the body's contents.
    let too_many = serde_json::to_vec(&json!({"order_ids": vec![Uuid::new_v4(); 101]})).unwrap();
    let response = post(
        overlay_app(offline_state()),
        Body::from(too_many),
        None,
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Size and syntax may precede authentication; neither reaches a domain rule.
    let oversized = vec![b'x'; compliance::http::POST_BODY_MAX_BYTES + 1];
    let response = post(
        overlay_app(offline_state()),
        Body::from(oversized),
        None,
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(response).await;
    assert_eq!(body["detail"]["code"], "INVALID_INPUT");
    assert_eq!(
        body["detail"]["message"],
        format!(
            "Request body is larger than {} bytes.",
            compliance::http::POST_BODY_MAX_BYTES
        )
    );

    for body in [Body::from("{not json"), Body::empty()] {
        let response = post(overlay_app(offline_state()), body, None, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json_body(response).await["detail"]["code"], "INVALID_INPUT");
    }

    // The body bound is inclusive: exactly 65536 bytes passes the syntax stage
    // and is then refused by authentication, never by size.
    let prefix = "{\"order_ids\":[],\"pad\":\"";
    let suffix = "\"}";
    let padding = compliance::http::POST_BODY_MAX_BYTES - prefix.len() - suffix.len();
    let exact = format!("{prefix}{}{suffix}", "x".repeat(padding));
    assert_eq!(exact.len(), compliance::http::POST_BODY_MAX_BYTES);
    let response = post(overlay_app(offline_state()), Body::from(exact), None, false).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // The route is POST-only. The composed serving router owns the exact
    // method-and-slash policy for the group (Task 14); here the module's own
    // answer is asserted.
    let response = get(overlay_app(offline_state()), compliance::http::OVERLAY_PATH).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()["allow"], "POST");
}

#[cfg(feature = "contract-tests")]
fn signed_token(subject: Uuid) -> String {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    let claims = json!({
        "sub": subject,
        "type": "access",
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "jti": Uuid::new_v4().to_string(),
        "iat": now.timestamp(),
        "exp": (now + Duration::hours(1)).timestamp(),
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET.as_bytes()),
    )
    .unwrap()
}

/// The three inputs that decide whether this runner carries the populated
/// overlay matrix. Kept as a value so the rule is testable without process
/// environment mutation.
#[cfg(feature = "contract-tests")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureState {
    pub url: Option<String>,
    pub required: bool,
    pub manifest: bool,
}

/// The populated fixture URL, or `None` in a runner that does not carry it.
///
/// `run_market_contract.sh` sets `RUST_OVERLAY_REQUIRED=1` because it seeds the
/// fixture; there, a missing URL or manifest is a broken invocation and must
/// refuse rather than report an empty pass. A manifest without its URL is the
/// same mistake in any runner. Only a runner that says nothing and sets
/// neither value skips this matrix, and `run_postgres.sh` names the cases in its
/// skip list instead.
#[cfg(feature = "contract-tests")]
pub fn fixture_target(state: FixtureState) -> Option<String> {
    match (state.url, state.required, state.manifest) {
        (Some(url), _, _) => Some(url),
        (None, true, _) => panic!("the overlay fixture requires RUST_MARKET_TEST_DATABASE_URL"),
        (None, false, true) => {
            panic!("the overlay fixture manifest is set without its database URL")
        }
        (None, false, false) => None,
    }
}

#[cfg(feature = "contract-tests")]
fn fixture_url() -> Option<String> {
    fixture_target(FixtureState {
        url: std::env::var("RUST_MARKET_TEST_DATABASE_URL").ok(),
        required: std::env::var("RUST_OVERLAY_REQUIRED").is_ok_and(|value| value == "1"),
        manifest: std::env::var("RUST_OVERLAY_TEST_CASES").is_ok(),
    })
}

#[cfg(feature = "contract-tests")]
async fn writer_pool() -> sqlx::PgPool {
    let url = std::env::var("RUST_MARKET_WRITER_URL")
        .expect("disposable market writer URL")
        .replace("postgresql+asyncpg://", "postgres://");
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("writer pool")
}

#[cfg(feature = "contract-tests")]
async fn user_id(pool: &sqlx::PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email=$1")
        .bind(email)
        .fetch_one(pool)
        .await
        .expect("seeded fixture account")
}

/// A current account that belongs to no organization. The disposable database
/// is discarded after the run, so the row is not removed.
#[cfg(feature = "contract-tests")]
async fn no_organization_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,email,password_hash,role,status,organization_id, \
         email_verified,must_change_password,kyc_status) \
         VALUES ($1,$2,'fixture-hash','BUYER','APPROVED',NULL,true,false,'APPROVED')",
    )
    .bind(id)
    .bind(format!("overlay-no-org-{id}@rust-market.test"))
    .execute(pool)
    .await
    .expect("no-organization account");
    id
}

/// The market-contract runner grants its app role `SELECT` on every table and
/// no write privilege. The route's transaction takes the current-account row in
/// update mode, which PostgreSQL lets only a role with `UPDATE` on the table
/// perform, so the authenticated cases grant that one privilege on the
/// disposable database first, as the deployed runtime role already holds it for
/// its own account writes.
#[cfg(feature = "contract-tests")]
async fn grant_actor_lock(writer: &sqlx::PgPool, database_url: &str) {
    let options = database_url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .expect("disposable fixture URL");
    assert_eq!(
        options.get_username(),
        "rust_app",
        "the market-contract runner's app role"
    );
    sqlx::query("GRANT UPDATE ON users TO rust_app")
        .execute(writer)
        .await
        .expect("current-account lock privilege");
}

#[cfg(feature = "contract-tests")]
async fn authenticated_state(config: &Config) -> AppState {
    let pools = verdaxis_read::connect(config).await.expect("read pools");
    AppState::new(config.clone(), pools, fixture_policy()).expect("app state")
}

#[cfg(feature = "contract-tests")]
fn manifest() -> Value {
    serde_json::from_slice(
        &std::fs::read(
            std::env::var("RUST_OVERLAY_TEST_CASES").expect("overlay fixture manifest path"),
        )
        .expect("overlay fixture manifest must exist"),
    )
    .expect("valid overlay fixture manifest")
}

#[cfg(feature = "contract-tests")]
fn scope_count(manifest: &Value, name: &str) -> u64 {
    manifest["scopes"]
        .as_array()
        .expect("required actor scopes")
        .iter()
        .find(|scope| scope["name"] == name)
        .unwrap_or_else(|| panic!("required scope {name}"))["count"]
        .as_u64()
        .expect("exact fixture count")
}

#[cfg(feature = "contract-tests")]
fn uuids(manifest: &Value, key: &str) -> Vec<Uuid> {
    manifest[key]
        .as_array()
        .expect("required fixture ID list")
        .iter()
        .map(|value| value.as_str().unwrap().parse().unwrap())
        .collect()
}

#[cfg(feature = "contract-tests")]
fn decimal(value: &Value) -> rust_decimal::Decimal {
    value
        .as_str()
        .expect("decimal as a string")
        .parse()
        .expect("a decimal")
}

/// The authenticated route against the populated market fixture: the account
/// and support order, the scoped fleet count, every requested key, the
/// eligible-ASK map and the reporting-year assumptions.
#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn compliance_overlay_authenticated_cases() {
    use rust_decimal::Decimal;

    let Some(url) = fixture_url() else { return };
    let writer = writer_pool().await;
    let config = test_config(url.clone(), None);
    grant_actor_lock(&writer, &url).await;
    let state = authenticated_state(&config).await;
    let manifest = manifest();
    let member = user_id(&writer, "fixture-real@rust-market.test").await;
    let admin = user_id(&writer, "fixture-admin@rust-market.test").await;
    let pending = user_id(&writer, "fixture-pending@rust-market.test").await;
    let no_organization = no_organization_user(&writer).await;
    let member_token = signed_token(member);
    let admin_token = signed_token(admin);
    let pending_token = signed_token(pending);
    let no_organization_token = signed_token(no_organization);
    let visible = uuids(&manifest, "visible_ids");
    let hidden = uuids(&manifest, "hidden_ids");
    assert!(visible.len() >= 2, "two qualified fixture orders required");
    assert!(hidden.len() >= 4, "hidden fixture orders required");
    let mixed: Vec<Uuid> = visible.iter().chain(&hidden).copied().collect();
    let mixed_body = serde_json::to_vec(&json!({"order_ids": mixed})).unwrap();
    let empty_body = serde_json::to_vec(&json!({"order_ids": []})).unwrap();
    let invalid = serde_json::to_vec(&json!({"order_ids": ["not-a-uuid"], "year": 1999})).unwrap();
    let too_many = serde_json::to_vec(&json!({"order_ids": vec![Uuid::new_v4(); 101]})).unwrap();

    // Authentication precedes every semantic refusal, and a current account
    // that the accepted policy denies never reaches the typed rules.
    for (token, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(&pending_token), StatusCode::FORBIDDEN),
    ] {
        for body in [invalid.clone(), too_many.clone(), empty_body.clone()] {
            let response = post(
                overlay_app(state.clone()),
                Body::from(body),
                token.map(String::as_str),
                false,
            )
            .await;
            assert_eq!(response.status(), status);
        }
    }

    // A Market Support request is refused before the typed rules, for every
    // body, including an empty one.
    for body in [invalid.clone(), too_many.clone(), empty_body.clone()] {
        let response = post(
            overlay_app(state.clone()),
            Body::from(body),
            Some(member_token.as_str()),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(response).await["detail"]["code"],
            "MARKET_SUPPORT_MUTATION_NOT_ALLOWED"
        );
    }

    // Authenticated, outside Market Support mode: the typed input rules decide.
    let response = post(
        overlay_app(state.clone()),
        Body::from(invalid.clone()),
        Some(member_token.as_str()),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(response).await["detail"]["code"], "INVALID_INPUT");
    let response = post(
        overlay_app(state.clone()),
        Body::from(too_many.clone()),
        Some(member_token.as_str()),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(response).await["detail"]["code"], "INVALID_INPUT");

    // An empty request still carries the exact actor-scoped fleet count.
    for (token, scope) in [
        (&member_token, "member"),
        (&admin_token, "administrator"),
        (&no_organization_token, "no_organization"),
    ] {
        let response = post(
            overlay_app(state.clone()),
            Body::from(empty_body.clone()),
            Some(token.as_str()),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{scope}");
        let body = json_body(response).await;
        assert_eq!(body["overlays"].as_object().unwrap().len(), 0);
        assert_eq!(
            body["assumptions"]["fleet_vessel_count"],
            json!(scope_count(&manifest, scope))
        );
        assert_eq!(
            body["assumptions"]["fleet_intensity_basis"],
            "DEFAULT_VLSFO"
        );
        assert_eq!(body["assumptions"]["year"], 2026);
        assert_eq!(body["assumptions"]["year_target"], "89.3368");
        assert_eq!(body["assumptions"]["eur_usd_rate"], "1.08");
        assert_eq!(body["assumptions"]["ghgie_actual_gco2_mj"], "91.16");
        assert_eq!(body["assumptions"]["vlsfo_baseline_gco2_mj"], "91.16");
        assert_eq!(body["assumptions"]["penalty_eur_per_tonne"], "2400");
        assert_eq!(
            body["assumptions"]["excluded_factors"],
            json!([
                "RFNBO_MULTIPLIER",
                "DEFICIT_ESCALATION",
                "EXTRA_EU_VOYAGE_SCOPE"
            ])
        );
    }

    // A configured COMPLIANCE_EUR_USD_RATE reaches the response through the
    // route's own configuration, as Python's `_effective_eur_usd_rate` does.
    let fx_state = authenticated_state(&test_config(
        url.clone(),
        Some(rust_decimal::Decimal::new(110, 2)),
    ))
    .await;
    let fx_body = json_body(
        post(
            overlay_app(fx_state),
            Body::from(empty_body.clone()),
            Some(member_token.as_str()),
            false,
        )
        .await,
    )
    .await;
    assert_eq!(fx_body["assumptions"]["eur_usd_rate"], "1.10");

    // Every requested key is present, hidden rows are indistinguishable from a
    // missing id, and the two qualified rows keep distinct lifecycle values.
    let response = post(
        overlay_app(state.clone()),
        Body::from(mixed_body.clone()),
        Some(member_token.as_str()),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let overlays = body["overlays"].as_object().unwrap();
    assert_eq!(overlays.len(), mixed.len());
    for id in &hidden {
        assert!(overlays[&id.to_string()].is_null(), "hidden {id}");
    }
    for id in &visible {
        let overlay = &overlays[&id.to_string()];
        assert!(!overlay.is_null(), "visible {id}");
        assert_eq!(overlay["financial_benefit_status"], "UNPRICED");
        assert_eq!(
            decimal(&overlay["penalty_avoided_eur_per_mt"]),
            Decimal::ZERO
        );
        assert_eq!(
            decimal(&overlay["penalty_avoided_usd_per_mt"]),
            Decimal::ZERO
        );
        assert_eq!(overlay["ci_basis"], "LISTING");
        let (fuel, ci, lcv): (String, Option<Decimal>, Option<Decimal>) = sqlx::query_as(
            "SELECT p.fuel_type, o.carbon_intensity_gco2_mj, o.energy_density_mj_kg \
             FROM orderbook_orders o JOIN products p ON p.id=o.product_id WHERE o.id=$1",
        )
        .bind(id)
        .fetch_one(&writer)
        .await
        .expect("qualified fixture row");
        let expected = pricing::LifecycleOverlayCore::compute(alcohol_family(&fuel), ci, lcv)
            .unwrap()
            .unwrap();
        assert_eq!(decimal(&overlay["ci_gco2_mj"]), expected.ci_gco2_mj);
        assert_eq!(decimal(&overlay["lcv_mj_kg"]), expected.lcv_mj_kg);
        assert_eq!(
            decimal(&overlay["tco2e_avoided_per_mt"]),
            expected.tco2e_avoided_per_mt
        );
        assert_eq!(
            overlay["lcv_basis"],
            serde_json::to_value(expected.lcv_basis).unwrap()
        );
    }
    assert_ne!(
        overlays[&visible[0].to_string()],
        overlays[&visible[1].to_string()],
        "qualified fixtures must have distinct lifecycle values"
    );

    // The administrator reads the same marketplace ASK map with the global
    // count; the count is context, not a different projection.
    let admin_body = json_body(
        post(
            overlay_app(state.clone()),
            Body::from(mixed_body.clone()),
            Some(admin_token.as_str()),
            false,
        )
        .await,
    )
    .await;
    assert_eq!(
        admin_body["assumptions"]["fleet_vessel_count"],
        json!(scope_count(&manifest, "administrator"))
    );
    assert_eq!(admin_body["overlays"], body["overlays"]);

    // Duplicates collapse in the response and still consume the raw 100 bound.
    let duplicate = serde_json::to_vec(&json!({"order_ids": vec![visible[0]; 100]})).unwrap();
    let duplicate_body = json_body(
        post(
            overlay_app(state.clone()),
            Body::from(duplicate),
            Some(member_token.as_str()),
            false,
        )
        .await,
    )
    .await;
    assert_eq!(duplicate_body["overlays"].as_object().unwrap().len(), 1);
    assert_eq!(
        duplicate_body["assumptions"]["fleet_vessel_count"],
        json!(scope_count(&manifest, "member"))
    );

    // The reporting year reaches the one shared schedule.
    let response = post(
        overlay_app(state.clone()),
        Body::from(serde_json::to_vec(&json!({"order_ids": [], "year": 2035})).unwrap()),
        Some(member_token.as_str()),
        false,
    )
    .await;
    // Python's own Decimal spelling for the 2035 limit: 91.16 * 0.855 keeps
    // the product's scale, so the string has five decimal places.
    assert_eq!(
        json_body(response).await["assumptions"]["year_target"],
        "77.94180"
    );
}

/// The scoped deadlines are applied before the actor lock; a held account row
/// is refused inside the 500 ms lock wait, and a domain timeout maps to the
/// typed read envelope.
#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn compliance_overlay_timeout_cases() {
    use axum::response::IntoResponse as _;
    use std::time::{Duration, Instant};

    let Some(url) = fixture_url() else { return };
    let writer = writer_pool().await;
    let config = test_config(url.clone(), None);
    grant_actor_lock(&writer, &url).await;
    let state = authenticated_state(&config).await;
    let member = user_id(&writer, "fixture-real@rust-market.test").await;
    let token = signed_token(member);

    let access =
        auth::verify_access(&token, &settings(), chrono::Utc::now()).expect("signed access token");
    let (lock_wait, deadline) = auth::run_authenticated(
        &state,
        access,
        auth::AuthenticatedRequest::new("POST", compliance::http::OVERLAY_PATH, false),
        |transaction| {
            Box::pin(async move {
                let lock_wait: String = sqlx::query_scalar("SHOW lock_timeout")
                    .fetch_one(&mut *transaction.connection())
                    .await
                    .expect("lock timeout");
                let deadline: String = sqlx::query_scalar("SHOW statement_timeout")
                    .fetch_one(&mut *transaction.connection())
                    .await
                    .expect("statement timeout");
                Ok::<_, ApiError>((lock_wait, deadline))
            })
        },
    )
    .await
    .expect("the overlay helper transaction");
    assert_eq!(lock_wait, "500ms");
    assert_eq!(deadline, "5s");

    // Warm the pool, then hold the actor row from an independent connection.
    let empty = || Body::from(b"{\"order_ids\":[]}".to_vec());
    let response = post(
        overlay_app(state.clone()),
        empty(),
        Some(token.as_str()),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut holder = writer.begin().await.expect("actor holder");
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(member)
        .fetch_one(&mut *holder)
        .await
        .expect("hold the actor row");
    let started = Instant::now();
    let response = post(
        overlay_app(state.clone()),
        empty(),
        Some(token.as_str()),
        false,
    )
    .await;
    let elapsed = started.elapsed();
    // The accepted helper owns this refusal; in scoped-read mode the
    // actor-lock timeout maps to the read-unavailable response with one
    // retry hint, and the 500 ms wait bounds how long the client waits.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(
        json_body(response).await["detail"]["code"],
        "READ_UNAVAILABLE"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "the 500 ms lock wait must refuse before two seconds: {elapsed:?}"
    );
    holder.rollback().await.expect("release the actor row");

    // A statement that passes the shared 5 s deadline inside the admitted
    // closure is refused by PostgreSQL itself, and the route's own failure
    // mapping answers a typed read-unavailable response with one retry hint.
    let access =
        auth::verify_access(&token, &settings(), chrono::Utc::now()).expect("signed access token");
    let started = Instant::now();
    let refused: Result<(), auth::AuthenticatedError> = auth::run_authenticated(
        &state,
        access,
        auth::AuthenticatedRequest::new("POST", compliance::http::OVERLAY_PATH, false),
        |transaction| {
            Box::pin(async move {
                let error = sqlx::query("SELECT pg_sleep(6)")
                    .execute(&mut *transaction.connection())
                    .await
                    .expect_err("the scoped statement deadline must refuse the sleep");
                Err(overlay::overlay_failure(overlay::OverlayError::Database(
                    error,
                )))
            })
        },
    )
    .await;
    let elapsed = started.elapsed();
    let Err(auth::AuthenticatedError::Database(error)) = refused else {
        panic!("a statement deadline is a database refusal: {refused:?}");
    };
    let mapped = error.into_response();
    assert_eq!(mapped.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(mapped.headers()["retry-after"], "1");
    assert_eq!(
        json_body(mapped).await["detail"]["code"],
        "READ_UNAVAILABLE"
    );
    assert!(
        elapsed >= Duration::from_secs(4),
        "the statement must run to the five-second deadline: {elapsed:?}"
    );
}

/// One response is one statement, so its ASK map and fleet count cannot come
/// from different committed states.
///
/// The claim is checked at the connection: SQLx statement logging counts each
/// executed statement, and the response must account for exactly one. A
/// table-lock barrier cannot prove this statement's snapshot because
/// PostgreSQL takes a query's relation locks before its READ COMMITTED
/// snapshot, so a change committed while the reader waits is visible to it;
/// and changing an order row from a test would cross the Phase 2 write
/// boundary. The count therefore follows a committed change on its own, and
/// the map keeps the committed order values across both calls.
#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn compliance_overlay_snapshot_cases() {
    use rust_decimal::Decimal;
    use sqlx::{ConnectOptions as _, Connection as _};
    use tracing_subscriber::layer::SubscriberExt as _;

    let Some(url) = fixture_url() else { return };
    let writer = writer_pool().await;
    let config = test_config(url.clone(), None);
    let manifest = manifest();
    let member = user_id(&writer, "fixture-real@rust-market.test").await;
    let order = uuids(&manifest, "visible_ids")[0];
    let member_count = scope_count(&manifest, "member");
    let organization: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
            .bind(member)
            .fetch_one(&writer)
            .await
            .expect("member organization");
    let committed_ci: Option<Decimal> =
        sqlx::query_scalar("SELECT carbon_intensity_gco2_mj FROM orderbook_orders WHERE id=$1")
            .bind(order)
            .fetch_one(&writer)
            .await
            .expect("fixture ASK row");
    let committed_ci = committed_ci.expect("the fixture ASK declares a CI");
    let actor = auth::CurrentActor {
        id: member,
        organization_id: organization,
        role: Some("BUYER".into()),
        email_verified: true,
    };

    // A connection whose statements are logged, so the test counts them.
    let options = url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .expect("disposable fixture URL")
        .log_statements("debug".parse().expect("log level"));
    let mut connection = sqlx::PgConnection::connect_with(&options)
        .await
        .expect("logging connection");
    let executed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let policy = fixture_policy();
    let request = OverlayRequest::validate(json!({"order_ids": [order]})).unwrap();
    // The guard keeps this connection's statement events on the test's own
    // current-thread runtime for the whole awaited call.
    let subscriber = tracing_subscriber::registry().with(OneStatement(executed.clone()));
    let guard = tracing::subscriber::set_default(subscriber);
    executed.lock().unwrap().clear();
    let response = overlay::select_overlay(
        &mut connection,
        &policy,
        &market::visibility::VisibilityClock::at(chrono::Utc::now()),
        &config,
        &actor,
        &request,
    )
    .await
    .expect("overlay response");
    drop(guard);
    let statements = executed.lock().unwrap().clone();
    assert_eq!(
        statements.len(),
        1,
        "one statement per response: {statements:?}"
    );
    assert!(
        statements[0].contains("fleet_count"),
        "the one statement carries the fleet count: {statements:?}"
    );
    assert_eq!(response.assumptions.fleet_vessel_count, member_count);
    let overlay = response.overlays[&order].as_ref().expect("visible overlay");
    assert_eq!(overlay.ci_gco2_mj, committed_ci);

    // A committed vessel insert moves the count on the next response; the map
    // still names the committed order values.
    let snapshot_vessel = Uuid::new_v4();
    sqlx::query("INSERT INTO vessels (id,organization_id,name,imo_number) VALUES ($1,$2,$3,$4)")
        .bind(snapshot_vessel)
        .bind(organization)
        .bind(format!("Snapshot {snapshot_vessel}"))
        .bind(snapshot_vessel.to_string())
        .execute(&writer)
        .await
        .expect("insert a member vessel");
    executed.lock().unwrap().clear();
    let subscriber = tracing_subscriber::registry().with(OneStatement(executed.clone()));
    let guard = tracing::subscriber::set_default(subscriber);
    let after = overlay::select_overlay(
        &mut connection,
        &policy,
        &market::visibility::VisibilityClock::at(chrono::Utc::now()),
        &config,
        &actor,
        &request,
    )
    .await
    .expect("second overlay response");
    drop(guard);
    let statements = executed.lock().unwrap().clone();
    assert_eq!(
        statements.len(),
        1,
        "one statement per response: {statements:?}"
    );
    assert_eq!(after.assumptions.fleet_vessel_count, member_count + 1);
    let after_overlay = after.overlays[&order].as_ref().expect("visible overlay");
    assert_eq!(after_overlay.ci_gco2_mj, committed_ci);
    assert_eq!(
        after_overlay.tco2e_avoided_per_mt,
        overlay.tco2e_avoided_per_mt
    );

    // Restore the fixture so the other populated cases see the seeded values.
    sqlx::query("DELETE FROM vessels WHERE id=$1")
        .bind(snapshot_vessel)
        .execute(&writer)
        .await
        .expect("remove the snapshot vessel");
    let restored: i64 = sqlx::query_scalar("SELECT count(*) FROM vessels WHERE organization_id=$1")
        .bind(organization)
        .fetch_one(&writer)
        .await
        .expect("restored member count");
    assert_eq!(restored as u64, member_count);
    connection
        .close()
        .await
        .expect("close the logging connection");
}

/// Records the summary of each SQLx statement event into a shared list.
#[cfg(feature = "contract-tests")]
struct OneStatement(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

#[cfg(feature = "contract-tests")]
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for OneStatement {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut summary = String::new();
        event.record(&mut SummaryVisitor(&mut summary));
        self.0.lock().expect("statement list").push(summary);
    }
}

#[cfg(feature = "contract-tests")]
struct SummaryVisitor<'a>(&'a mut String);

#[cfg(feature = "contract-tests")]
impl tracing::field::Visit for SummaryVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        // `summary` is the first four words; `db.statement` carries the whole
        // statement when it was shortened, so both are kept.
        if matches!(field.name(), "summary" | "db.statement") {
            self.0.push_str(value);
            self.0.push('\n');
        }
    }

    /// Every traced field of a statement event is a string or a number; a
    /// debug-only field must not be mistaken for the statement summary.
    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}
}

/// The accepted read index exists and is not unique.
#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn compliance_overlay_index_cases() {
    let Some(_url) = fixture_url() else { return };
    let writer = writer_pool().await;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname, indexdef FROM pg_indexes WHERE schemaname='public' \
         AND indexname IN ('ix_p5_vessels_org_id','ix_p5_producers_cod_name_id', \
         'ix_p5_producers_status_cod_id') ORDER BY indexname",
    )
    .fetch_all(&writer)
    .await
    .expect("index catalog");
    assert_eq!(
        rows.len(),
        3,
        "the accepted read indexes must exist: {rows:?}"
    );
    let definition = |name: &str| {
        rows.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, definition)| definition.as_str())
            .unwrap_or_else(|| panic!("missing index {name}"))
    };
    assert!(definition("ix_p5_vessels_org_id").contains("(organization_id, id)"));
    assert!(definition("ix_p5_producers_cod_name_id").contains("(cod_year, name, id)"));
    assert!(definition("ix_p5_producers_status_cod_id").contains("(status, cod_year, id)"));
    for (_, definition) in &rows {
        assert!(
            !definition.contains("UNIQUE"),
            "an additive index is never unique"
        );
    }
}

#[test]
#[ignore = "Task 11: actual frontend feature fixture after the composed route"]
fn compliance_overlay_client_cases() {
    panic!("Task 11 must supply the coordinated client fixture before client acceptance");
}

#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn compliance_overlay_postgres_cases() {
    use auth::CurrentActor;
    use chrono::Utc;
    use sqlx::Row;
    let url =
        std::env::var("RUST_MARKET_TEST_DATABASE_URL").expect("disposable market fixture URL");
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    use sqlx::Connection;
    let policy = fixture_policy();
    let config = test_config(url, None);
    verdaxis_read::validate_config(&config).unwrap();
    let clock = market::visibility::VisibilityClock::at(
        std::env::var("RUST_MARKET_TEST_CLOCK")
            .ok()
            .map(|value| value.parse().unwrap())
            .unwrap_or_else(Utc::now),
    );
    let visible = market::visibility::select_public_order_ids(
        &mut connection,
        &policy,
        &clock,
        &config,
        &market::visibility::PublicOrderFilters {
            side: Some("ASK"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!visible.is_empty(), "fixture must contain public ASKs");
    let request =
        OverlayRequest::validate(json!({"order_ids": &visible[..visible.len().min(100)]})).unwrap();
    let actor = CurrentActor {
        id: Uuid::new_v4(),
        organization_id: None,
        role: Some("BUYER".into()),
        email_verified: true,
    };
    let response =
        overlay::select_overlay(&mut connection, &policy, &clock, &config, &actor, &request)
            .await
            .unwrap();
    assert!(
        response.overlays.values().any(Option::is_some),
        "fixture must exercise priced lifecycle values"
    );
    for id in request.order_ids() {
        let row = sqlx::query("SELECT p.fuel_type, o.carbon_intensity_gco2_mj AS ci, o.energy_density_mj_kg AS lcv FROM orderbook_orders o JOIN products p ON p.id=o.product_id WHERE o.id=$1").bind(id).fetch_one(&mut connection).await.unwrap();
        let fuel: String = row.get("fuel_type");
        let expected = pricing::LifecycleOverlayCore::compute(
            alcohol_family(&fuel),
            row.get("ci"),
            row.get("lcv"),
        )
        .unwrap();
        assert_eq!(
            response.overlays[id],
            expected.map(overlay::OverlayListing::from)
        );
    }
    let fixture = manifest();
    let qualified = uuids(&fixture, "visible_ids");
    let hidden = uuids(&fixture, "hidden_ids");
    assert!(
        qualified.len() >= 2,
        "two qualified fixture orders required"
    );
    assert!(hidden.len() >= 4, "hidden ASK and BID fixtures required");
    let mut hidden_ask = false;
    let mut hidden_bid = false;
    for id in &hidden {
        let row = sqlx::query(
            "SELECT o.side::text AS side, p.fuel_type, o.carbon_intensity_gco2_mj AS ci, \
             o.energy_density_mj_kg AS lcv FROM orderbook_orders o \
             JOIN products p ON p.id=o.product_id WHERE o.id=$1",
        )
        .bind(id)
        .fetch_one(&mut connection)
        .await
        .expect("known hidden fixture order must exist");
        let side: String = row.get("side");
        hidden_ask |= side == "ASK";
        hidden_bid |= side == "BID";
        let fuel: String = row.get("fuel_type");
        assert!(
            pricing::LifecycleOverlayCore::compute(
                alcohol_family(&fuel),
                row.get("ci"),
                row.get("lcv"),
            )
            .unwrap()
            .is_some(),
            "hidden fixture must have computable values"
        );
    }
    assert!(hidden_ask && hidden_bid, "known ASK and BID required");
    let absent = Uuid::new_v4();
    let mixed: Vec<Uuid> = qualified
        .iter()
        .chain(&hidden)
        .copied()
        .chain([absent])
        .collect();
    let request = OverlayRequest::validate(json!({"order_ids": mixed})).unwrap();
    let response =
        overlay::select_overlay(&mut connection, &policy, &clock, &config, &actor, &request)
            .await
            .unwrap();
    assert_eq!(response.overlays.len(), request.order_ids().len());
    for id in hidden.iter().chain([&absent]) {
        assert_eq!(response.overlays[id], None, "null overlay for {id}");
    }
    for id in &qualified {
        assert!(visible.contains(id), "qualified fixture must be public");
        assert!(
            response.overlays[id].is_some(),
            "qualified overlay for {id}"
        );
    }
    assert_ne!(
        response.overlays[&qualified[0]], response.overlays[&qualified[1]],
        "qualified fixtures must have distinct lifecycle values"
    );
    let scopes = fixture["scopes"].as_array().expect("required actor scopes");
    assert_eq!(scopes.len(), 5, "all actor scopes are required");
    for (scope, (name, role, count)) in scopes.iter().zip([
        ("administrator", "ADMIN", 6),
        ("member", "BUYER", 2),
        ("other_member", "BUYER", 3),
        ("empty_member", "BUYER", 0),
        ("no_organization", "BUYER", 0),
    ]) {
        assert_eq!(scope["name"], name);
        assert_eq!(scope["role"], role);
        assert_eq!(scope["count"], count);
        let organization_id = scope["organization_id"]
            .as_str()
            .map(|value| value.parse().unwrap());
        if matches!(name, "member" | "other_member" | "empty_member") {
            assert!(organization_id.is_some(), "member organization is required");
        } else {
            assert!(organization_id.is_none());
        }
        let scoped_actor = CurrentActor {
            organization_id,
            role: Some(scope["role"].as_str().unwrap().into()),
            ..actor.clone()
        };
        let expected_count = scope["count"].as_u64().expect("exact fixture count");
        for ids in [vec![], mixed.clone(), vec![qualified[0]; 100]] {
            let request = OverlayRequest::validate(json!({"order_ids": ids})).unwrap();
            let response = overlay::select_overlay(
                &mut connection,
                &policy,
                &clock,
                &config,
                &scoped_actor,
                &request,
            )
            .await
            .unwrap();
            assert_eq!(response.overlays.len(), request.order_ids().len());
            assert_eq!(
                response.assumptions.fleet_vessel_count,
                expected_count,
                "fleet scope {} with {} requested IDs",
                scope["name"],
                request.order_ids().len()
            );
        }
    }
}
