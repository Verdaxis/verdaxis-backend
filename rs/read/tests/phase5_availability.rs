//! Availability read contracts and populated PostgreSQL cases.

pub use verdaxis_read::{ApiError, AppState, MarketPolicy, runtime};
pub use verdaxis_read::routes;

#[path = "../src/routes/availability.rs"]
mod availability;
#[path = "support/phase5/public_fixture.rs"]
#[allow(dead_code)]
mod public_fixture;

use availability::{AvailabilityRow, project_rows, select_availability};
use serde_json::{Value, json};
use sqlx::{Connection, Executor};

fn row() -> AvailabilityRow {
    AvailabilityRow {
        port_id: "NLRTM".into(),
        port_name: "Rotterdam".into(),
        market_product: "GREEN_METHANOL".into(),
        lat: Some(0.0),
        lng: Some(0.0),
        total_stock_mt: Some("1000.00".into()),
        avg_price_per_mt: Some("123.46".into()),
        supplier_count: 2,
        unknown_count: 1,
        source_kind: "LIVE_INVENTORY".into(),
        demo_status: "REAL_ONLY".into(),
        availability_level: "LIMITED".into(),
    }
}

#[test]
fn availability_evidence_cases() {
    let response = serde_json::to_value(project_rows(vec![row()]).unwrap()).unwrap();
    assert_eq!(response[0], json!({
        "port_id": "NLRTM", "port_name": "Rotterdam", "lat": 0.0, "lng": 0.0,
        "fuel_type": "GREEN_METHANOL", "market_product_code": "GREEN_METHANOL",
        "total_stock_mt": "1000.00", "supplier_count": 2,
        "availability_level": "LIMITED", "avg_price_per_mt": "123.46",
        "source_kind": "LIVE_INVENTORY", "scope": "DELIVERY_POINT",
        "demo_status": "REAL_ONLY", "unknown_count": 1
    }));
    let mut unknown = row();
    unknown.total_stock_mt = None;
    unknown.avg_price_per_mt = None;
    unknown.supplier_count = 0;
    unknown.source_kind = "UNKNOWN".into();
    unknown.demo_status = "UNKNOWN".into();
    unknown.availability_level = "NONE".into();
    let response = serde_json::to_value(project_rows(vec![unknown]).unwrap()).unwrap();
    assert!(response[0]["total_stock_mt"].is_null());
    assert!(response[0]["avg_price_per_mt"].is_null());
    assert_eq!(response[0]["supplier_count"], 0);
}

#[test]
fn availability_coordinate_cases() {
    let mut missing_lat = row();
    missing_lat.lat = None;
    let mut missing_lng = row();
    missing_lng.lng = None;
    let projected = project_rows(vec![missing_lat, row(), missing_lng]).unwrap();
    assert_eq!(projected.len(), 1);
    let response = serde_json::to_value(projected).unwrap();
    assert_eq!(response[0]["lat"], 0.0);
    assert_eq!(response[0]["lng"], 0.0);
}

#[test]
fn availability_result_limits() {
    assert_eq!(project_rows(vec![row(); 512]).unwrap().len(), 512);
    let error = project_rows(vec![row(); 513]).unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.detail["code"], "READ_RESULT_LIMIT");
    assert_eq!(error.retry_after, Some(1));
    let mut missing = row();
    missing.lat = None;
    assert!(project_rows(vec![missing; 513]).is_err());
}

fn policy() -> MarketPolicy {
    serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()
}

fn config(url: &str) -> verdaxis_read::Config {
    serde_json::from_value(json!({
        "environment": "test", "release_sha": verdaxis_read::compiled_sha(),
        "migration_revision": "p5_20261020_read_indexes", "database_url": url,
        "db_session_time_zone": "UTC", "bind": "127.0.0.1:0",
        "read_admission_slots": 16, "pool_max": 17, "python_budget": 52,
        "configured_max_connections": 100
    })).unwrap()
}

#[tokio::test]
async fn availability_public_boundary_cases() {
    let config = config("postgres://fixture:fixture_pw@127.0.0.1:1/availability_test");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy(&config.database_url).unwrap();
    let state = AppState::new(config, runtime::db::Pools {
        read: pool.clone(), health: pool,
    }, policy()).unwrap();
    let _permit = state.reads.clone().acquire_many_owned(16).await.unwrap();
    let app = runtime::http::middleware(availability::routes().with_state(state));
    let response = public_fixture::request(app, "/api/availability").await;
    assert_eq!(response.status(), 503);
    assert_eq!(public_fixture::json_body(response).await["detail"]["code"], "READ_UNAVAILABLE");
}

async fn execute(connection: &mut sqlx::PgConnection, sql: &str) {
    connection.execute(sql).await.unwrap();
}

async fn insert_supplier(connection: &mut sqlx::PgConnection, org: &str, class: &str) {
    sqlx::query("INSERT INTO organizations VALUES ($1::uuid, $2, 'APPROVED')")
        .bind(org).bind(class).execute(&mut *connection).await.unwrap();
    sqlx::query("INSERT INTO users VALUES ($1::uuid, $1::uuid, 'APPROVED', 'APPROVED')")
        .bind(org).execute(connection).await.unwrap();
}

async fn stock(connection: &mut sqlx::PgConnection, org: &str, port: &str, amount: &str, price: &str) {
    sqlx::query("INSERT INTO inventory_items (supplier_id, owner_user_id, port_id, product_name, current_stock_mt, price_per_mt_usd) VALUES ($1::uuid, $1::uuid, $2, 'Bio Methanol', $3::text::numeric, $4::text::numeric)")
        .bind(org).bind(port).bind(amount).bind(price).execute(connection).await.unwrap();
}

async fn read(connection: &mut sqlx::PgConnection, filter: Option<&str>) -> Result<Value, ApiError> {
    select_availability(connection, &policy(), filter.map(str::to_owned)).await
        .map(|rows| serde_json::to_value(rows).unwrap())
}

#[tokio::test]
#[ignore = "requires an explicit disposable PostgreSQL URL with PostGIS"]
async fn availability_postgres_cases() {
    let url = std::env::var("AVAILABILITY_TEST_DATABASE_URL").expect("explicit disposable database URL");
    let parsed = url::Url::parse(&url).unwrap();
    assert!(matches!(parsed.host_str(), Some("127.0.0.1" | "::1")));
    assert!(parsed.path().ends_with("_test"));
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    let mut transaction = connection.begin().await.unwrap();
    let connection = &mut *transaction;
    execute(connection, "SET LOCAL statement_timeout = '5s'").await;
    execute(connection, "CREATE TEMP TABLE organizations (id uuid PRIMARY KEY, provenance text, verification_status text);
        CREATE TEMP TABLE users (id uuid PRIMARY KEY, organization_id uuid, status text, kyc_status text);
        CREATE TEMP TABLE products (id uuid PRIMARY KEY, name text, fuel_type text, fuel_grade text, is_active boolean);
        CREATE TEMP TABLE delivery_points (id uuid PRIMARY KEY, name text, region text, is_active boolean);
        CREATE TEMP TABLE ports (id text PRIMARY KEY, name text, location geography(POINT,4326), is_active boolean);
        CREATE TEMP TABLE inventory_items (supplier_id uuid, owner_user_id uuid, port_id text, product_name text, current_stock_mt numeric, price_per_mt_usd numeric, off_spec boolean DEFAULT false, specification_standard text, carbon_intensity_method text, certification_declared boolean DEFAULT false, certification_scheme text, msds_available boolean DEFAULT false, carbon_intensity_gco2_mj numeric, feedstock text, origin text);").await;
    for product in policy().products {
        sqlx::query("INSERT INTO products VALUES ($1,$2,$3,$4,true)")
            .bind(product.id).bind(product.name).bind(product.fuel_type).bind(product.fuel_grade)
            .execute(&mut *connection).await.unwrap();
    }
    for point in policy().points {
        sqlx::query("INSERT INTO delivery_points VALUES ($1,$2,$3,true)")
            .bind(point.id).bind(point.name).bind(point.region)
            .execute(&mut *connection).await.unwrap();
    }
    execute(connection, "INSERT INTO ports VALUES ('NLRTM','Rotterdam',ST_SetSRID(ST_MakePoint(0,0),4326)::geography,true)").await;
    let real = "00000000-0000-4000-8000-000000000001";
    let second = "00000000-0000-4000-8000-000000000002";
    let unknown = "00000000-0000-4000-8000-000000000003";
    let demo = "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4";
    let test = "00000000-0000-4000-8000-000000000004";
    let canary = "00000000-0000-4000-8000-000000000005";
    for (org, class) in [(real,"REAL"),(second,"REAL"),(unknown,"UNKNOWN"),(demo,"DEMO"),(test,"TEST"),(canary,"CANARY")] {
        insert_supplier(connection, org, class).await;
    }
    stock(connection, real, "NLRTM", "400", "100").await;
    stock(connection, real, "NLRTM", "200", "200").await;
    stock(connection, second, "NLRTM", "400", "200").await;
    stock(connection, unknown, "NLRTM", "99999", "99999").await;
    stock(connection, demo, "NLRTM", "99999", "99999").await;
    stock(connection, test, "NLRTM", "99999", "99999").await;
    stock(connection, canary, "NLRTM", "99999", "99999").await;
    let rows = read(connection, None).await.unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["total_stock_mt"], "1000");
    assert_eq!(rows[0]["supplier_count"], 2);
    assert_eq!(rows[0]["avg_price_per_mt"], "166.67");
    assert_eq!(rows[0]["availability_level"], "LIMITED");
    assert_eq!(rows[0]["unknown_count"], 1);
    assert_eq!(rows[0]["demo_status"], "REAL_ONLY");
    assert_eq!(rows[0]["lat"], 0.0);
    assert!(read(connection, Some("ethANol")).await.unwrap().as_array().unwrap().len() == 1);
    assert_eq!(read(connection, Some("unlisted")).await.unwrap(), json!([]));
    stock(connection, real, "NLRTM", "0.001", "200").await;
    assert_eq!(read(connection, None).await.unwrap()[0]["availability_level"], "AVAILABLE");
    execute(connection, "UPDATE users SET kyc_status = 'PENDING' WHERE id IN ('00000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000002')").await;
    let rows = read(connection, None).await.unwrap();
    assert_eq!(rows[0]["demo_status"], "DEMO_ONLY");
    assert_eq!(rows[0]["source_kind"], "DEMO_SEED");
    assert_eq!(rows[0]["unknown_count"], 3);
    execute(connection, "UPDATE users SET status = 'PENDING' WHERE id = 'c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4'").await;
    let rows = read(connection, None).await.unwrap();
    assert!(rows[0]["total_stock_mt"].is_null());
    assert!(rows[0]["avg_price_per_mt"].is_null());
    assert_eq!(rows[0]["supplier_count"], 0);
    assert_eq!(rows[0]["source_kind"], "UNKNOWN");
    execute(connection, "UPDATE ports SET location = NULL").await;
    assert_eq!(read(connection, None).await.unwrap(), json!([]));
    execute(connection, "UPDATE ports SET location = ST_SetSRID(ST_MakePoint(0,0),4326)::geography;
        UPDATE users SET status = 'APPROVED', kyc_status = 'APPROVED';
        UPDATE inventory_items SET current_stock_mt = 0, price_per_mt_usd = NULL").await;
    let rows = read(connection, None).await.unwrap();
    assert_eq!(rows[0]["availability_level"], "NONE");
    assert!(rows[0]["avg_price_per_mt"].is_null());
    execute(connection, "UPDATE products SET is_active = false WHERE name = 'Bio Methanol'").await;
    assert_eq!(read(connection, None).await.unwrap(), json!([]));
    execute(connection, "UPDATE products SET is_active = true;
        UPDATE delivery_points SET is_active = false WHERE name = 'Rotterdam'").await;
    assert_eq!(read(connection, None).await.unwrap(), json!([]));
    execute(connection, "UPDATE delivery_points SET is_active = true;
        UPDATE ports SET name = 'Unlisted'").await;
    assert_eq!(read(connection, None).await.unwrap(), json!([]));
    execute(connection, "UPDATE ports SET name = 'Rotterdam';
        INSERT INTO inventory_items (supplier_id,owner_user_id,port_id,product_name,current_stock_mt,price_per_mt_usd)
        VALUES ('00000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000001','NLRTM','B30',500,400)").await;
    assert_eq!(read(connection, None).await.unwrap().as_array().unwrap().len(), 1);
    execute(connection, "UPDATE inventory_items SET specification_standard = 'ISO 8217:2024 RF 380', carbon_intensity_method = 'declared', certification_declared = true, certification_scheme = 'ISCC EU', msds_available = true, carbon_intensity_gco2_mj = 30, feedstock = 'used cooking oil', origin = 'NL' WHERE product_name = 'B30'").await;
    assert_eq!(read(connection, None).await.unwrap().as_array().unwrap().len(), 2);
    execute(connection, "UPDATE inventory_items SET off_spec = true WHERE product_name = 'B30'").await;
    assert_eq!(read(connection, None).await.unwrap().as_array().unwrap().len(), 1);
    execute(connection, "INSERT INTO inventory_items (supplier_id,owner_user_id,port_id,product_name,current_stock_mt,price_per_mt_usd)
        VALUES ('00000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000001','NLRTM','Bio-LNG',500,400)").await;
    assert_eq!(read(connection, None).await.unwrap().as_array().unwrap().len(), 1);
    execute(connection, "UPDATE ports SET name = 'Dalian'").await;
    assert_eq!(read(connection, Some("Bio-LNG")).await.unwrap(), json!([]));
    execute(connection, "UPDATE ports SET name = 'Rotterdam';
        INSERT INTO ports SELECT 'p' || n, 'Rotterdam', ST_SetSRID(ST_MakePoint(0,0),4326)::geography, true FROM generate_series(1,511) n;
        INSERT INTO inventory_items (supplier_id,owner_user_id,port_id,product_name,current_stock_mt,price_per_mt_usd)
        SELECT '00000000-0000-4000-8000-000000000001'::uuid,'00000000-0000-4000-8000-000000000001'::uuid,id,'Bio Methanol',1234567890123456789012345678901234567890.12,1 FROM ports WHERE id <> 'NLRTM'").await;
    let rows = read(connection, None).await.unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 512);
    assert!(rows.as_array().unwrap().iter().any(|r| r["total_stock_mt"] == "1234567890123456789012345678901234567890.12"));
    execute(connection, "INSERT INTO ports VALUES ('last','Rotterdam',ST_SetSRID(ST_MakePoint(0,0),4326)::geography,true)").await;
    stock(connection, real, "last", "1", "1").await;
    let error = read(connection, None).await.unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.detail["code"], "READ_RESULT_LIMIT");
    transaction.rollback().await.unwrap();
}
