//! Phase 4 Task 17b: legacy dashboard (P02, P03) and identified user listing (P04).
//!
//! The four cases below are the task's required assertions:
//!
//! * `p4_t17b_c01` — the legacy dashboard definitions stay separate from the
//!   product-analytics tabs and daily buckets are UTC days.
//! * `p4_t17b_c02` — the identified user listing is administrator-only, applies
//!   its filters, and reports absent values as absent.
//! * `p4_t17b_c03` — the three route contracts, admission order and token quota.
//! * `p4_t17b_c04` — the search and paging bounds.
//!
//! Every case needs disposable PostgreSQL: the projection and the authority
//! order read real tables. They are compiled here from their plan paths and the
//! whole target is empty unless the `contract-tests` feature is enabled, so an
//! offline workspace run neither needs a database nor silently passes them.

#![cfg(feature = "contract-tests")]

pub use verdaxis_read::*;

#[path = "../src/analytics/legacy_dashboard.rs"]
mod legacy_dashboard;
#[path = "../src/analytics/users.rs"]
mod users;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

use verdaxis_read::auth::VerificationSettings;

const SECRET: &str = "analytics-legacy-test-secret-at-least-32-chars";
const ISSUER: &str = "analytics-legacy-test";
const FORWARDED: &str = "203.0.113.77";

// Fixed identifiers so a failure names the row that broke the rule. These are
// also the seed's primary keys.
const ORG_REAL_A: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000a1);
const ORG_REAL_C: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000a3);

const USER_BUYER_A: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b1);
const USER_SUPPLIER_A: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b2);
const USER_BUYER_B: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b3);
const USER_ADMIN: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b4);
const USER_ORGLESS: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b6);
const USER_SUPPLIER_C: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b7);
const USER_ADMIN_TWO: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b8);
const USER_TIE_NINE: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000b9);
const USER_TIE_TEN: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000ba);
const USER_UNKNOWN_TOKEN: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000ff);

fn verification() -> VerificationSettings {
    VerificationSettings::new(SECRET.into(), None, ISSUER.into(), ISSUER.into())
}

fn token(user: Uuid) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({
            "sub": user,
            "type": "access",
            "iss": ISSUER,
            "aud": ISSUER,
            "jti": Uuid::new_v4(),
            "iat": Utc::now().timestamp() - 1,
            "exp": Utc::now().timestamp() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

fn config(url: String) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 20,
        configured_max_connections: 100,
    }
}

fn app(pool: sqlx::PgPool, url: String, limits: auth::limits::SharedLimits) -> Router {
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&url)
        .unwrap();
    let state = AppState::new(
        config(url),
        runtime::db::Pools { read: pool, health },
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let verification = Arc::new(verification());
    Router::new()
        .merge(legacy_dashboard::router(
            state.clone(),
            verification.clone(),
            limits.clone(),
        ))
        .merge(users::router(state, verification, limits))
}

/// The serving process always supplies the peer address; `oneshot` does not, so
/// every request carries the loopback peer and the client Caddy forwards.
fn send(path: &str, bearer: Option<&str>, forwarded: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41_000))));
    request
        .headers_mut()
        .insert("x-forwarded-for", forwarded.parse().unwrap());
    if let Some(bearer) = bearer {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {bearer}").parse().unwrap());
    }
    request
}

async fn get(
    app: &Router,
    path: &str,
    bearer: Option<&str>,
    forwarded: &str,
) -> (StatusCode, Value, Option<String>) {
    let response = app
        .clone()
        .oneshot(send(path, bearer, forwarded))
        .await
        .unwrap();
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 1 << 22).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, retry_after)
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    url: String,
    app: Router,
}

impl Fixture {
    async fn new() -> Self {
        let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator required");
        let runtime =
            std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime required");
        let m = url::Url::parse(&migrator).unwrap();
        let r = url::Url::parse(&runtime).unwrap();
        assert_eq!(m.host_str(), Some("127.0.0.1"));
        assert_eq!(r.host_str(), m.host_str());
        assert_eq!(r.port(), m.port());
        assert_eq!(r.path(), m.path());
        assert!(r.path().ends_with("_test"));
        assert_eq!(r.username(), "rust_app");
        assert_eq!(m.username(), "rust_migrator");
        assert_ne!(r.port_or_known_default(), Some(5432));
        let mut admin = PgConnection::connect(&migrator).await.unwrap();
        let schema = format!("p4_legacy_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("fixtures/analytics_legacy_schema.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/analytics_legacy_seed.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app;\
             GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app;\
             GRANT UPDATE ON users TO rust_app"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |conn, _| {
                let sql = search_path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&runtime)
            .await
            .unwrap();
        let limits = auth::limits::SharedLimits::default();
        let app = app(pool.clone(), runtime.clone(), limits);
        Self {
            pool,
            admin,
            schema,
            url: runtime,
            app,
        }
    }

    async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.admin)
        .await
        .unwrap();
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
        let (status, body, _) = get(&self.app, path, bearer, FORWARDED).await;
        (status, body)
    }

    /// The UTC day the seed used, read from the seed's own clock.
    async fn db_today(&mut self) -> chrono::NaiveDate {
        sqlx::query_scalar("SELECT (now() AT TIME ZONE 'UTC')::date")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
}

fn overview_selection(body: &Value) -> Value {
    json!({
        "total_users": body["total_users"],
        "active_users_7d": body["active_users_7d"],
        "total_organizations": body["total_organizations"],
        "total_orders": body["total_orders"],
        "open_orders": body["open_orders"],
        "total_trades": body["total_trades"],
        "confirmed_trades": body["confirmed_trades"],
        "total_volume_mt": body["total_volume_mt"],
        "total_revenue_usd": body["total_revenue_usd"],
        "total_gmv_usd": body["total_gmv_usd"],
    })
}

/// P02 and P03: the legacy market-member definition, not stored provenance.
#[tokio::test]
async fn p4_t17b_c01() {
    let mut f = Fixture::new().await;
    let admin = token(USER_ADMIN);

    let (status, body) = f.get("/api/admin/analytics/overview", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Seven market accounts (buyer A, supplier A, buyer B, orgless buyer,
    // supplier C, plus the two tiebreak buyers in Alpha Shipping). The
    // administrator and the NULL-role account are not market members, and
    // the demo organization has no account at all.
    assert_eq!(
        overview_selection(&body),
        json!({
            "total_users": 7,
            "active_users_7d": 1,
            "total_organizations": 3,
            "total_orders": 5,
            "open_orders": 3,
            "total_trades": 4,
            "confirmed_trades": 3,
            "total_volume_mt": 35.0,
            "total_revenue_usd": 5.0,
            "total_gmv_usd": 1000.0,
        })
    );
    // The response is exactly the Python field set.
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 10, "{body}");

    // The same window in the daily read counts calendar days in UTC. The
    // demo, administrator-only and NULL-role organizations are excluded by
    // the same definition even though their rows exist.
    let (status, body) = f
        .get("/api/admin/analytics/daily?days=3", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 3, "{body}");
    // The seed places rows relative to the database clock, so the expected
    // day comes from the same clock rather than from a second reading of
    // the host time.
    let today = f.db_today().await;
    let dates: Vec<String> = rows
        .iter()
        .map(|row| row["date"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        dates,
        vec![
            (today - Duration::days(2)).to_string(),
            (today - Duration::days(1)).to_string(),
            today.to_string(),
        ]
    );
    assert_eq!(
        rows[2],
        json!({
            "date": today.to_string(),
            "orders_placed": 3,
            "trades_executed": 1,
            "volume_mt": 10.0,
            "gmv_usd": 1000.0,
            "commission_usd": 5.0,
        })
    );
    assert_eq!(
        rows[1],
        json!({
            "date": (today - Duration::days(1)).to_string(),
            "orders_placed": 1,
            "trades_executed": 1,
            "volume_mt": 20.0,
            "gmv_usd": 2000.0,
            "commission_usd": 10.0,
        })
    );
    // A pending trade is not "executed", so the day is a genuine zero.
    assert_eq!(
        rows[0],
        json!({
            "date": (today - Duration::days(2)).to_string(),
            "orders_placed": 0,
            "trades_executed": 0,
            "volume_mt": 0.0,
            "gmv_usd": 0.0,
            "commission_usd": 0.0,
        })
    );

    // The dense series always spans the requested number of days.
    for days in [1, 30, 365] {
        let (status, body) = f
            .get(
                &format!("/api/admin/analytics/daily?days={days}"),
                Some(&admin),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), days as usize);
    }
    f.finish().await;
}

/// P04: administrator-only, filtered, and honest about absent values.
#[tokio::test]
async fn p4_t17b_c02() {
    let f = Fixture::new().await;
    let admin = token(USER_ADMIN);
    let buyer = token(USER_BUYER_A);

    // A non-administrator is refused identically by all three routes, even
    // when the query would identify a specific account.
    let mut refusals = Vec::new();
    for path in [
        "/api/admin/analytics/overview",
        "/api/admin/analytics/daily",
        "/api/admin/analytics/users?search=alpha%40example.test",
    ] {
        let (status, body) = f.get(path, Some(&buyer)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        refusals.push(body);
    }
    assert!(refusals.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(refusals[0]["detail"]["code"], "ADMIN_REQUIRED");

    let (status, body) = f.get("/api/admin/analytics/users", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 7);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 7);
    // Newest account first; the administrator and the NULL-role account are
    // never listed.
    let ids: Vec<&str> = items
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![
            USER_BUYER_A.to_string(),
            USER_SUPPLIER_A.to_string(),
            USER_BUYER_B.to_string(),
            USER_ORGLESS.to_string(),
            USER_SUPPLIER_C.to_string(),
            // The tiebreak pair shares one created_at to the microsecond;
            // the greater id is listed first, pinning `(created_at, id)`.
            USER_TIE_TEN.to_string(),
            USER_TIE_NINE.to_string(),
        ]
    );

    let buyer_a = &items[0];
    // The listing row is exactly the fifteen contract fields; a sixteenth
    // (for example an accidentally projected secret) must fail here.
    assert_eq!(buyer_a.as_object().unwrap().len(), 15, "{buyer_a}");
    assert_eq!(buyer_a["email"], "alpha@example.test");
    assert_eq!(buyer_a["first_name"], "Pat");
    assert_eq!(buyer_a["last_name"], "Lee");
    assert_eq!(buyer_a["role"], "BUYER");
    assert_eq!(buyer_a["status"], "APPROVED");
    assert_eq!(buyer_a["organization_id"], ORG_REAL_A.to_string());
    assert_eq!(buyer_a["org_name"], "Alpha Shipping");
    assert_eq!(buyer_a["org_type"], "SHIPPING_LINE");
    assert_eq!(buyer_a["org_provenance"], "REAL");
    assert_eq!(buyer_a["email_verified"], true);
    assert_eq!(buyer_a["must_change_password"], false);
    assert_eq!(buyer_a["org_has_orders"], true);
    assert!(buyer_a["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(buyer_a["last_login"].as_str().unwrap().ends_with('Z'));

    let supplier_a = &items[1];
    assert_eq!(supplier_a["org_type"], "FUEL_SUPPLIER");
    assert_eq!(supplier_a["org_has_orders"], true);
    assert_eq!(supplier_a["email_verified"], true);

    let buyer_b = &items[2];
    assert_eq!(buyer_b["status"], "PENDING");
    assert_eq!(buyer_b["email_verified"], false);
    assert_eq!(buyer_b["must_change_password"], true);
    assert_eq!(buyer_b["last_login"], Value::Null);
    assert_eq!(buyer_b["first_name"], Value::Null);
    assert_eq!(buyer_b["last_name"], Value::Null);

    // An account without an organization reports absent organization data,
    // never an invented zero or empty label.
    let orgless = &items[3];
    assert_eq!(orgless["organization_id"], Value::Null);
    assert_eq!(orgless["org_name"], Value::Null);
    assert_eq!(orgless["org_type"], Value::Null);
    assert_eq!(orgless["org_provenance"], Value::Null);
    assert_eq!(orgless["org_has_orders"], false);
    assert_eq!(orgless["role"], "BUYER");

    // An organization with no order is reported as such.
    let supplier_c = &items[4];
    assert_eq!(supplier_c["organization_id"], ORG_REAL_C.to_string());
    assert_eq!(supplier_c["org_has_orders"], false);

    // Status filters are the closed account-status set, case-insensitive,
    // and `all` is the explicit no-filter spelling.
    for (filter, expected) in [
        ("APPROVED", 6),
        ("approved", 4),
        ("PENDING", 1),
        ("pending", 1),
        ("REJECTED", 0),
        ("all", 7),
        ("ALL", 7),
    ] {
        let (status, body) = f
            .get(
                &format!("/api/admin/analytics/users?status={filter}"),
                Some(&admin),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{filter}: {body}");
        assert_eq!(body["total"], expected, "{filter}");
    }

    // Name and email search are case-insensitive substring matches.
    for (search, expected) in [("pat", 1), ("PARK", 1), ("example.test", 7), ("nobody", 0)] {
        let (status, body) = f
            .get(
                &format!("/api/admin/analytics/users?search={search}"),
                Some(&admin),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{search}: {body}");
        assert_eq!(body["total"], expected, "{search}");
    }
    f.finish().await;
}

/// The three contracts, the admission order, and the token quota.
#[tokio::test]
async fn p4_t17b_c03() {
    let f = Fixture::new().await;
    let admin = token(USER_ADMIN);
    let buyer = token(USER_BUYER_A);

    for path in [
        "/api/admin/analytics/overview",
        "/api/admin/analytics/daily?days=30",
        "/api/admin/analytics/users?limit=50",
    ] {
        let (status, body) = f.get(path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {body}");
        assert_eq!(body["detail"]["code"], "INVALID_CREDENTIALS");
        let (status, body) = f.get(path, Some("not-a-token")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {body}");
    }

    // The account row decides admission, not the token alone.
    let (status, body) = f
        .get(
            "/api/admin/analytics/users",
            Some(&token(USER_UNKNOWN_TOKEN)),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // Authentication and authority refusals leave an unrelated identity
    // able to make an ordinary request afterwards. The refused identities
    // (an invalid token and a non-administrator) have no success path on this
    // route, so their own counters cannot be compared against a served
    // request here; what this observes is that the shared counter authority
    // keeps serving after refusals, and the route order (quota after the role
    // refusal) is pinned by construction in the adapter.
    for _ in 0..60 {
        let (status, _) = f
            .get("/api/admin/analytics/overview", Some("not-a-token"))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    for _ in 0..60 {
        let (status, _) = f.get("/api/admin/analytics/overview", Some(&buyer)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, body) = f.get("/api/admin/analytics/overview", Some(&buyer)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = f.get("/api/admin/analytics/overview", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Declared query bounds refuse with 422 before the read.
    for (path, code) in [
        ("/api/admin/analytics/daily?days=0", "greater_than_equal"),
        ("/api/admin/analytics/daily?days=366", "less_than_equal"),
        ("/api/admin/analytics/daily?days=thirty", "int_parsing"),
    ] {
        let (status, body) = f.get(path, Some(&admin)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {body}");
        assert!(
            body["detail"].as_array().unwrap()[0]["type"]
                .as_str()
                .unwrap()
                .starts_with(code),
            "{path}: {body}"
        );
    }

    // The default page is the documented 30-day series.
    let (status, body) = f.get("/api/admin/analytics/daily", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 30);

    // Every route carries its own 60/minute token bucket and reports the
    // window reset. The three routes do not share a counter.
    let f_app = &f.app;
    for _ in 0..60 {
        let (status, body, _) = get(
            f_app,
            "/api/admin/analytics/users?limit=1",
            Some(&admin),
            FORWARDED,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body, retry_after) = get(
        f_app,
        "/api/admin/analytics/users?limit=1",
        Some(&admin),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["detail"]["code"], "RATE_LIMITED");
    assert!(
        retry_after
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|seconds| (1..=60).contains(&seconds)),
        "{retry_after:?}"
    );
    // Another token has its own bucket.
    let second = token(USER_ADMIN_TWO);
    let (status, body, _) = get(
        f_app,
        "/api/admin/analytics/users",
        Some(&second),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The daily and overview buckets are still fresh.
    let (status, body, _) = get(f_app, "/api/admin/analytics/daily", Some(&admin), FORWARDED).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body, _) = get(
        f_app,
        "/api/admin/analytics/overview",
        Some(&admin),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The limit counts a fresh window for a different forwarded client. A
    // second administrator sends the same number of requests from a new
    // address and is admitted, so the counter is keyed by identity, not by
    // the process.
    let second_limits = auth::limits::SharedLimits::default();
    let isolated = app(f.pool.clone(), f.url.clone(), second_limits);
    for _ in 0..60 {
        let (status, _, _) = get(
            &isolated,
            "/api/admin/analytics/users",
            Some(&second),
            FORWARDED,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _, _) = get(
        &isolated,
        "/api/admin/analytics/users",
        Some(&second),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    drop(isolated);
    f.finish().await;
}

/// Search and paging bounds on the identified listing.
#[tokio::test]
async fn p4_t17b_c04() {
    let f = Fixture::new().await;
    let admin = token(USER_ADMIN);

    // Paging is bounded and `total` is the number of matching rows, not the
    // size of the page.
    for (query, count, first) in [
        ("limit=2", 2, USER_BUYER_A),
        ("limit=100", 7, USER_BUYER_A),
        ("limit=1&offset=1", 1, USER_SUPPLIER_A),
        ("offset=4", 3, USER_SUPPLIER_C),
        ("offset=6", 1, USER_TIE_NINE),
        ("offset=7", 0, USER_UNKNOWN_TOKEN),
        ("offset=10000", 0, USER_UNKNOWN_TOKEN),
    ] {
        let (status, body) = f
            .get(&format!("/api/admin/analytics/users?{query}"), Some(&admin))
            .await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        assert_eq!(body["total"], 7, "{query}");
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), count, "{query}: {body}");
        if count > 0 {
            assert_eq!(items[0]["id"], first.to_string(), "{query}");
        }
    }

    for query in [
        "limit=0",
        "limit=101",
        "limit=abc",
        "offset=-1",
        "offset=10001",
        "offset=abc",
    ] {
        let (status, body) = f
            .get(&format!("/api/admin/analytics/users?{query}"), Some(&admin))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}: {body}");
        assert!(body["detail"].is_array(), "{query}: {body}");
    }

    // The search term is capped at 200 characters; the boundary itself is
    // accepted.
    let at_bound = "a".repeat(200);
    let (status, body) = f
        .get(
            &format!("/api/admin/analytics/users?search={at_bound}"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0);

    let over_bound = "a".repeat(201);
    let (status, body) = f
        .get(
            &format!("/api/admin/analytics/users?search={over_bound}"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let detail = body["detail"].as_array().unwrap();
    assert_eq!(detail[0]["loc"], json!(["query", "search"]));

    // An unknown status is a client error, not an empty page.
    let (status, body) = f
        .get("/api/admin/analytics/users?status=ARCHIVED", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["detail"]["code"], "INVALID_STATUS_FILTER");

    // A double-invalid query keeps the typed-validation envelope (422, both
    // errors reported): the typed framework validates every parameter before
    // the handler runs, so the 400 status filter never shadows them.
    let (status, body) = f
        .get(
            "/api/admin/analytics/users?status=ARCHIVED&limit=0",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let detail = body["detail"].as_array().unwrap();
    assert!(
        detail
            .iter()
            .any(|error| error["loc"] == json!(["query", "limit"])),
        "{body}"
    );

    // An empty search behaves as no filter, as the reference does.
    let (status, body) = f
        .get("/api/admin/analytics/users?search=", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 7);

    f.finish().await;
}
