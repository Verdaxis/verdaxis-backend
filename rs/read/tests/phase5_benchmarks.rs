//! Benchmark query, catalog and quote contracts.

pub use verdaxis_read::{ApiError, AppState, Json, MarketPolicy, market, routes, run_market_read};
#[path = "../src/routes/benchmark_quotes.rs"]
mod benchmark_quotes;

use axum::{body::{Body, to_bytes}, http::Request};
use chrono::{DateTime, Utc};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

fn now() -> DateTime<Utc> {
    "2026-10-23T00:00:00Z".parse().unwrap()
}

fn policy() -> MarketPolicy {
    serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()
}

fn seeded_key(policy: &MarketPolicy) -> (String, Uuid) {
    let seed = &routes::curves_canonical::benchmarks::policy()["seed_quotes"][0];
    let code = seed["market_product"].as_str().unwrap().to_owned();
    let point = seed["delivery_point_id"].as_str().unwrap().parse().unwrap();
    assert!(policy.products.iter().any(|p| p.code == code && p.lanes.contains(&point)));
    (code, point)
}

#[test]
fn benchmark_query_cases() {
    let policy = policy();
    let (code, point) = seeded_key(&policy);
    let query = benchmark_quotes::parse_query(None, &policy, now()).unwrap();
    assert!(query.market_product.is_none());
    assert!(query.delivery_point_id.is_none());
    assert_eq!(query.availability_window, "SPOT");
    for window in ["SPOT", "2027-02", "2027-Q2", "2028-CAL"] {
        let raw = format!("market_product={code}&delivery_point_id={point}&availability_window={window}");
        let query = benchmark_quotes::parse_query(Some(&raw), &policy, now()).unwrap();
        assert_eq!(query.market_product.as_deref(), Some(code.as_str()));
        assert_eq!(query.delivery_point_id, Some(point));
        assert_eq!(query.availability_window, window);
    }
    for raw in [
        "delivery_point_id=not-a-uuid", "delivery_point_id=", "market_product=",
        "market_product=not-a-product", "availability_window=", "availability_window=2027-13",
        "availability_window=0000-01", "availability_window=soon", "availability_window=SPOT&availability_window=2027-01",
    ] {
        let error = benchmark_quotes::parse_query(Some(raw), &policy, now()).unwrap_err();
        assert_eq!(error.status, 422, "{raw}");
        assert_eq!(error.detail["code"], "INVALID_INPUT");
    }
    let alias = benchmark_quotes::parse_query(Some("availability_window=Q1_2026"), &policy, now()).unwrap();
    assert_eq!(alias.availability_window, "2026-Q1");
    assert!(benchmark_quotes::parse_query(Some(&format!("market_product={code}")), &policy, now()).is_ok());
    assert!(benchmark_quotes::parse_query(Some(&format!("delivery_point_id={point}")), &policy, now()).is_ok());
}

fn state(policy: MarketPolicy, pool: sqlx::PgPool) -> AppState {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(), environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(), migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(), read_admission_slots: 1, pool_max: 2,
        python_budget: 52, configured_max_connections: 100, prepared_budget: None,
        compliance_eur_usd_rate: None,
    };
    AppState::new(config, verdaxis_read::runtime::db::Pools { read: pool.clone(), health: pool }, policy).unwrap()
}

async fn body(response: axum::response::Response) -> Json {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn benchmark_query_cases_http() {
    let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test").unwrap();
    let app = benchmark_quotes::routes().with_state(state(policy(), pool));
    for uri in ["/api/benchmarks", "/api/benchmarks?market_product=", "/api/benchmarks?delivery_point_id=bad"] {
        let response = app.clone().oneshot(Request::builder().uri(uri).header("authorization", "Bearer unused").body(Body::empty()).unwrap()).await.unwrap();
        if uri == "/api/benchmarks" {
            assert_eq!(response.status(), 200);
            let value = body(response).await;
            assert_eq!(value["items"], json!([]));
            assert!(DateTime::parse_from_rfc3339(value["generated_at"].as_str().unwrap()).is_ok());
        } else {
            assert_eq!(response.status(), 422);
            assert_eq!(body(response).await["detail"]["code"], "INVALID_INPUT");
        }
    }
}

// Temporary tables shadow application tables only on this disposable connection.
async fn fixture(connection: &mut sqlx::PgConnection, policy: &MarketPolicy) {
    sqlx::raw_sql("CREATE TEMP TABLE products(id uuid, name text, fuel_type text, fuel_grade text, is_active boolean); CREATE TEMP TABLE delivery_points(id uuid, name text, region text, is_active boolean); CREATE TEMP TABLE benchmarks(market_product text, delivery_point_id uuid, availability_window text, price_per_mt_usd numeric, source text, created_at timestamptz, updated_at timestamptz);").execute(&mut *connection).await.unwrap();
    for p in &policy.products {
        sqlx::query("INSERT INTO products VALUES($1,$2,$3,$4,true)").bind(p.id).bind(&p.name).bind(&p.fuel_type).bind(&p.fuel_grade).execute(&mut *connection).await.unwrap();
    }
    for p in &policy.points {
        sqlx::query("INSERT INTO delivery_points VALUES($1,$2,$3,true)").bind(p.id).bind(&p.name).bind(&p.region).execute(&mut *connection).await.unwrap();
    }
}

#[tokio::test]
async fn benchmark_catalog_cases() {
    let Ok(url) = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL") else { return; };
    use sqlx::Connection;
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    let policy = policy();
    fixture(&mut connection, &policy).await;
    let (code, point) = seeded_key(&policy);
    let raw = format!("market_product={code}&delivery_point_id={point}");
    let query = benchmark_quotes::parse_query(Some(&raw), &policy, now()).unwrap();
    assert!(benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap().is_some());
    for statement in [
        "UPDATE products SET is_active=false", "UPDATE products SET name=name || ' alias'",
        "UPDATE products SET fuel_type=fuel_type || ' alias'", "UPDATE products SET fuel_grade=fuel_grade || ' alias'",
        "UPDATE delivery_points SET is_active=false", "UPDATE delivery_points SET name=name || ' alias'",
        "UPDATE delivery_points SET region=region || ' alias'",
    ] {
        // Each mutation is restored from the attested fixture below.
        sqlx::query(statement).execute(&mut connection).await.unwrap();
        assert!(benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap().is_none(), "{statement}");
        sqlx::query("TRUNCATE products, delivery_points").execute(&mut connection).await.unwrap();
        for p in &policy.products {
            sqlx::query("INSERT INTO products VALUES($1,$2,$3,$4,true)").bind(p.id).bind(&p.name).bind(&p.fuel_type).bind(&p.fuel_grade).execute(&mut connection).await.unwrap();
        }
        for p in &policy.points {
            sqlx::query("INSERT INTO delivery_points VALUES($1,$2,$3,true)").bind(p.id).bind(&p.name).bind(&p.region).execute(&mut connection).await.unwrap();
        }
    }
    let mut restricted = policy.clone();
    restricted.products.iter_mut().find(|p| p.code == code).unwrap().lanes.retain(|id| *id != point);
    assert!(benchmark_quotes::select_quote(&mut connection, &restricted, &query, now()).await.unwrap().is_none());
    // Every catalog lane without an attested seed remains unquoted without an override.
    let seeds = &routes::curves_canonical::benchmarks::policy()["seed_quotes"];
    for p in &policy.products {
        for point in &p.lanes {
            if seeds.as_array().unwrap().iter().any(|s| s["market_product"] == p.code && s["delivery_point_id"] == point.to_string()) { continue; }
            let raw = format!("market_product={}&delivery_point_id={point}", p.code);
            let query = benchmark_quotes::parse_query(Some(&raw), &policy, now()).unwrap();
            assert!(benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap().is_none());
        }
    }
    connection.close().await.unwrap();
}

#[tokio::test]
async fn benchmark_result_cases() {
    let Ok(url) = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL") else { return; };
    use sqlx::Connection;
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    let policy = policy();
    fixture(&mut connection, &policy).await;
    let (code, point) = seeded_key(&policy);
    for window in ["SPOT", "2027-02", "2027-Q2", "2028-CAL"] {
        let raw = format!("market_product={code}&delivery_point_id={point}&availability_window={window}");
        let query = benchmark_quotes::parse_query(Some(&raw), &policy, now()).unwrap();
        let quote = serde_json::to_value(benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap().unwrap()).unwrap();
        let expected = routes::curves_canonical::benchmarks::board_quote(None, &(code.clone(), point.to_string(), window.into()), now());
        assert_eq!(quote["benchmark_price_per_mt_usd"], expected["price"]);
        assert_eq!(quote["source"], "seed_matrix");
        assert_eq!(quote["observed_at"], Json::Null);
        assert_eq!(quote["market_product"], code);
        assert_eq!(quote["delivery_point_id"], point.to_string());
    }
    sqlx::query("INSERT INTO benchmarks VALUES($1,$2,'SPOT',1234.565,'manual', '2026-10-01T00:00:00Z',NULL)").bind(&code).bind(point).execute(&mut connection).await.unwrap();
    let query = benchmark_quotes::parse_query(Some(&format!("market_product={code}&delivery_point_id={point}")), &policy, now()).unwrap();
    let quote = serde_json::to_value(benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap().unwrap()).unwrap();
    assert_eq!(quote["benchmark_price_per_mt_usd"], "1234.56");
    assert_eq!(quote["source"], "manual");
    assert_eq!(quote["observed_at"], "2026-10-01T00:00:00Z");
    sqlx::query("UPDATE benchmarks SET updated_at='2026-10-02T00:00:00Z'").execute(&mut connection).await.unwrap();
    let quote = serde_json::to_value(benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap().unwrap()).unwrap();
    assert_eq!(quote["observed_at"], "2026-10-02T00:00:00Z");
    sqlx::query("INSERT INTO benchmarks SELECT * FROM benchmarks").execute(&mut connection).await.unwrap();
    let error = benchmark_quotes::select_quote(&mut connection, &policy, &query, now()).await.unwrap_err();
    assert_eq!(error.status, 500);
    assert_eq!(error.detail["code"], "READ_FAILED");
    connection.close().await.unwrap();
}
