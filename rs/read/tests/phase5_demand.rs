//! Demand read contracts and populated PostgreSQL cases.

pub use verdaxis_read::*;
#[path = "../src/routes/demand.rs"]
mod demand;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use chrono::NaiveDate;
use serde_json::Value;
use tower::ServiceExt;

fn state() -> AppState {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "p5_20261020_read_indexes".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/demand_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    };
    let pools = runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect_lazy(&config.database_url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    AppState::new(config, pools, policy()).unwrap()
}

fn policy() -> MarketPolicy {
    let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
    let document: Value = serde_json::from_slice(bytes).unwrap();
    market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn demand_query_cases() {
    assert_eq!(demand::DemandQuery::parse(None).unwrap().limit, 256);
    assert_eq!(
        demand::DemandQuery::parse(Some("limit=1")).unwrap().limit,
        1
    );
    assert_eq!(
        demand::DemandQuery::parse(Some("limit=512")).unwrap().limit,
        512
    );
    let query =
        demand::DemandQuery::parse(Some("fuel_type=green+fuel&region=Asia%20Pacific")).unwrap();
    assert_eq!(query.fuel_type.as_deref(), Some("green fuel"));
    assert_eq!(query.region.as_deref(), Some("Asia Pacific"));
    let app = demand::routes().with_state(state());
    for limit in ["0", "513", "-1", "one", "1.5"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/demand?limit={limit}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 422, "{limit}");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["detail"]["code"], "INVALID_INPUT");
    }
}

#[test]
fn demand_urgency_cases() {
    for (today, window, expected) in [
        ("2026-10-23", "SPOT", "HIGH"),
        ("2026-10-02", "2026-11", "HIGH"),
        ("2026-10-01", "2026-11", "MEDIUM"),
        ("2026-10-03", "2027-01", "MEDIUM"),
        ("2026-10-02", "2027-01", "LOW"),
        ("2026-10-23", "2026-Q4", "HIGH"),
        ("2026-10-23", "2027-CAL", "MEDIUM"),
    ] {
        assert_eq!(
            demand::urgency(window, today.parse::<NaiveDate>().unwrap()).unwrap(),
            expected
        );
    }
}

// These cases require an explicit numeric-loopback disposable database. They
// create only connection-local temporary tables and never use an ambient URL.
#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use serde_json::json;
    use sqlx::{Connection, PgConnection};
    use uuid::Uuid;

    async fn fixture() -> (PgConnection, AppState, VisibilityClock) {
        let url = std::env::var("RUST_DEMAND_TEST_DATABASE_URL")
            .expect("explicit disposable demand URL required");
        let mut config = (*state().config).clone();
        config.database_url = url.clone();
        validate_config(&config).expect("numeric-loopback _test database required");
        let mut connection = PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE products (id uuid, name text, fuel_type text, fuel_grade text, is_active boolean);
            CREATE TEMP TABLE delivery_points (id uuid, name text, region text, is_active boolean);
            CREATE TEMP TABLE organizations (id uuid, verification_status text, provenance text);
            CREATE TEMP TABLE users (id uuid, organization_id uuid, role text, status text,
                email_verified boolean, must_change_password boolean, kyc_status text, kyc_organization_id uuid);
            CREATE TEMP TABLE kyc_execution_grace (user_id uuid, organization_id uuid, expires_at timestamptz);
            CREATE TEMP TABLE demand_bids (id uuid, product_id uuid, delivery_point_id uuid,
                side text DEFAULT 'BID', status text DEFAULT 'OPEN', provenance text DEFAULT 'REAL',
                availability_window text DEFAULT 'SPOT', remaining_quantity_mt numeric DEFAULT 10,
                price_per_mt_usd numeric DEFAULT 500, created_at timestamptz DEFAULT '2026-10-20Z',
                expires_at timestamptz, owner_user_id uuid, organization_id uuid,
                created_by_actor_user_id uuid, creation_method text DEFAULT 'DIRECT',
                fame_terms jsonb, lng_terms jsonb, off_spec boolean DEFAULT false,
                msds_available boolean DEFAULT false, certification_declared boolean DEFAULT false,
                certification_scheme text, specification_standard text, carbon_intensity_method text,
                carbon_intensity_gco2_mj numeric, feedstock text, origin text);
            CREATE TEMP VIEW orderbook_orders AS SELECT * FROM demand_bids;
        "#).execute(&mut connection).await.unwrap();
        let state = state();
        for product in &state.policy.products {
            // Use policy JSON rather than maintaining a second catalog in fixtures.
            let value = serde_json::to_value(product).unwrap();
            sqlx::query("INSERT INTO products VALUES ($1,$2,$3,$4,true)")
                .bind(value["id"].as_str().unwrap().parse::<Uuid>().unwrap())
                .bind(value["name"].as_str().unwrap())
                .bind(value["fuel_type"].as_str().unwrap())
                .bind(value["fuel_grade"].as_str().unwrap())
                .execute(&mut connection)
                .await
                .unwrap();
        }
        for point in &state.policy.points {
            let value = serde_json::to_value(point).unwrap();
            sqlx::query("INSERT INTO delivery_points VALUES ($1,$2,$3,true)")
                .bind(value["id"].as_str().unwrap().parse::<Uuid>().unwrap())
                .bind(value["name"].as_str().unwrap())
                .bind(value["region"].as_str().unwrap())
                .execute(&mut connection)
                .await
                .unwrap();
        }
        let clock = VisibilityClock::at("2026-10-23T12:00:00Z".parse().unwrap());
        (connection, state, clock)
    }

    fn lane(state: &AppState) -> (Uuid, Uuid) {
        let policy = serde_json::to_value(state.policy.as_ref()).unwrap();
        let product = policy["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK")
            .unwrap();
        (
            product["id"].as_str().unwrap().parse().unwrap(),
            product["lanes"][0].as_str().unwrap().parse().unwrap(),
        )
    }

    async fn insert(
        connection: &mut PgConnection,
        state: &AppState,
        window: &str,
        provenance: &str,
        volume: &str,
    ) {
        let (product, point) = lane(state);
        sqlx::query("INSERT INTO demand_bids (id,product_id,delivery_point_id,availability_window,provenance,remaining_quantity_mt,expires_at) VALUES ($1,$2,$3,$4,$5,$6::text::numeric,'2030-01-01Z')")
            .bind(Uuid::new_v4()).bind(product).bind(point).bind(window).bind(provenance).bind(volume)
            .execute(connection).await.unwrap();
    }

    async fn read(
        connection: &mut PgConnection,
        state: &AppState,
        clock: VisibilityClock,
        query: Option<&str>,
    ) -> Result<Value, ApiError> {
        let query = demand::DemandQuery::parse(query).unwrap();
        demand::load(connection, state, &clock, &query)
            .await
            .map(|signals| serde_json::to_value(signals).unwrap())
    }

    #[tokio::test]
    async fn demand_order_cases() {
        let (mut connection, state, clock) = fixture().await;
        insert(&mut connection, &state, "2027-12", "REAL", "999999").await;
        insert(&mut connection, &state, "SPOT", "REAL", "1").await;
        insert(&mut connection, &state, "2026-11", "REAL", "2").await;
        insert(&mut connection, &state, "SPOT", "UNKNOWN", "9999999").await;
        let result = read(&mut connection, &state, clock, Some("limit=1"))
            .await
            .unwrap();
        assert_eq!(result[0]["availability_window_code"], "2026-11");
        assert_eq!(result[0]["volume_mt"], "2");
        // Full-key ties do not depend on insertion order or created time.
        sqlx::query("TRUNCATE demand_bids")
            .execute(&mut connection)
            .await
            .unwrap();
        for window in ["2026-12", "2026-11", "SPOT"] {
            insert(&mut connection, &state, window, "REAL", "10").await;
        }
        let result = read(&mut connection, &state, clock, None).await.unwrap();
        assert_eq!(
            result
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["availability_window_code"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["2026-11", "SPOT", "2026-12"]
        );
        assert_eq!(
            result,
            read(&mut connection, &state, clock, None).await.unwrap()
        );
    }

    #[tokio::test]
    async fn demand_evidence_cases() {
        let (mut connection, state, clock) = fixture().await;
        insert(
            &mut connection,
            &state,
            "SPOT",
            "REAL",
            "123456789012345678901234567890.123456789",
        )
        .await;
        insert(&mut connection, &state, "SPOT", "REAL", "0.000000001").await;
        insert(&mut connection, &state, "SPOT", "DEMO", "8").await;
        insert(&mut connection, &state, "SPOT", "UNKNOWN", "9000").await;
        for (column, value) in [
            ("expires_at", "'2026-10-23T12:00:00Z'"),
            ("status", "'CANCELLED'"),
            ("side", "'ASK'"),
        ] {
            insert(&mut connection, &state, "2026-11", "REAL", "7").await;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE demand_bids SET {column}={value} WHERE availability_window='2026-11'"
            )))
            .execute(&mut connection)
            .await
            .unwrap();
        }
        let result = read(&mut connection, &state, clock, None).await.unwrap();
        let rows = result.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0]["volume_mt"],
            "123456789012345678901234567890.123456790"
        );
        assert_eq!(rows[0]["bid_count"], 2);
        assert_eq!(rows[0]["source_kind"], "LIVE_ORDER");
        assert_eq!(rows[0]["demo_status"], "REAL_ONLY");
        assert_eq!(rows[1]["source_kind"], "DEMO_SEED");
        assert_eq!(rows[1]["demo_status"], "DEMO_ONLY");
        assert_eq!(rows[2]["volume_mt"], Value::Null);
        assert_eq!(rows[2]["max_price_per_mt"], Value::Null);
        assert_eq!(rows[2]["bid_count"], 0);
        assert_eq!(rows[2]["unknown_count"], 1);
        let keys = rows[0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "fuel_type",
                "region",
                "market_product_code",
                "delivery_point_id",
                "delivery_point_name",
                "availability_window_code",
                "volume_mt",
                "max_price_per_mt",
                "urgency",
                "bid_count",
                "earliest_delivery",
                "created_at",
                "source_kind",
                "scope",
                "demo_status",
                "unknown_count"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        let owner = Uuid::new_v4();
        let org = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations VALUES ($1,'APPROVED','REAL')")
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users VALUES ($1,$2,'BUYER','APPROVED',true,false,'APPROVED',$2)")
            .bind(owner)
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("UPDATE demand_bids SET owner_user_id=$1, organization_id=$2")
            .bind(owner)
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None)
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            3
        );
        sqlx::query("UPDATE users SET status='REJECTED'")
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
    }

    #[tokio::test]
    async fn demand_status_expiry_cases() {
        let (mut connection, state, clock) = fixture().await;
        for provenance in ["REAL", "DEMO", "UNKNOWN"] {
            insert(&mut connection, &state, "SPOT", provenance, "7").await;
        }
        sqlx::query("UPDATE demand_bids SET expires_at=NULL, status='PARTIALLY_FILLED'")
            .execute(&mut connection)
            .await
            .unwrap();
        let result = read(&mut connection, &state, clock, None).await.unwrap();
        assert_eq!(result.as_array().unwrap().len(), 2);
        assert_eq!(result[0]["volume_mt"], "7");
        assert_eq!(result[0]["bid_count"], 1);
        assert_eq!(result[0]["source_kind"], "LIVE_ORDER");
        assert_eq!(result[1]["source_kind"], "UNKNOWN");
        assert_eq!(result[1]["unknown_count"], 1);
        assert_eq!(result[1]["volume_mt"], Value::Null);
    }

    #[tokio::test]
    async fn demand_catalog_cases() {
        let (mut connection, state, clock) = fixture().await;
        insert(&mut connection, &state, "SPOT", "REAL", "11").await;
        for (change, restore) in [
            (
                "UPDATE products SET is_active=false",
                "UPDATE products SET is_active=true",
            ),
            (
                "UPDATE products SET name=name || ' changed'",
                "UPDATE products SET name=left(name,length(name)-8)",
            ),
            (
                "UPDATE products SET fuel_grade=fuel_grade || ' changed'",
                "UPDATE products SET fuel_grade=left(fuel_grade,length(fuel_grade)-8)",
            ),
            (
                "UPDATE delivery_points SET is_active=false",
                "UPDATE delivery_points SET is_active=true",
            ),
            (
                "UPDATE delivery_points SET region=region || ' changed'",
                "UPDATE delivery_points SET region=left(region,length(region)-8)",
            ),
            (
                "UPDATE delivery_points SET name=name || ' changed'",
                "UPDATE delivery_points SET name=left(name,length(name)-8)",
            ),
        ] {
            sqlx::query(change).execute(&mut connection).await.unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap(),
                json!([]),
                "{change}"
            );
            sqlx::query(restore).execute(&mut connection).await.unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap()[0]["volume_mt"],
                "11",
                "{restore}"
            );
        }
        sqlx::query(
            "UPDATE demand_bids SET product_id=(SELECT id FROM products WHERE name='Bio-LNG')",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
    }

    #[tokio::test]
    async fn demand_owner_cases() {
        let (mut connection, state, clock) = fixture().await;
        let owner = Uuid::new_v4();
        let org = Uuid::new_v4();
        let other = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations VALUES ($1,'APPROVED','REAL'),($2,'APPROVED','REAL')",
        )
        .bind(org)
        .bind(other)
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users VALUES ($1,$2,'BUYER','APPROVED',true,false,'APPROVED',$2)")
            .bind(owner)
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        insert(&mut connection, &state, "SPOT", "REAL", "13").await;
        sqlx::query("UPDATE demand_bids SET owner_user_id=$1,organization_id=$2")
            .bind(owner)
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        for (change, restore) in [
            (
                "UPDATE organizations SET verification_status='PENDING'",
                "UPDATE organizations SET verification_status='APPROVED'",
            ),
            (
                "UPDATE users SET email_verified=false",
                "UPDATE users SET email_verified=true",
            ),
            (
                "UPDATE users SET must_change_password=true",
                "UPDATE users SET must_change_password=false",
            ),
            (
                "UPDATE users SET status='PENDING'",
                "UPDATE users SET status='APPROVED'",
            ),
            (
                "UPDATE users SET kyc_status='REJECTED'",
                "UPDATE users SET kyc_status='APPROVED'",
            ),
            (
                "UPDATE users SET kyc_organization_id=NULL",
                "UPDATE users SET kyc_organization_id=organization_id",
            ),
        ] {
            sqlx::query(change).execute(&mut connection).await.unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap(),
                json!([]),
                "{change}"
            );
            sqlx::query(restore).execute(&mut connection).await.unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap()[0]["volume_mt"],
                "13",
                "{restore}"
            );
        }
        sqlx::query("UPDATE demand_bids SET organization_id=$1")
            .bind(other)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
        sqlx::query("UPDATE demand_bids SET organization_id=$1")
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL")
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
        sqlx::query("INSERT INTO kyc_execution_grace VALUES ($1,$2,'2026-10-24Z')")
            .bind(owner)
            .bind(org)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap()[0]["volume_mt"],
            "13"
        );
        sqlx::query("UPDATE kyc_execution_grace SET organization_id=$1")
            .bind(other)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
        sqlx::query(
            "UPDATE kyc_execution_grace SET organization_id=$1,expires_at='2026-10-23T12:00:00Z'",
        )
        .bind(org)
        .execute(&mut connection)
        .await
        .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
        let mut advisory = state.clone();
        std::sync::Arc::make_mut(&mut advisory.config).kyc_execution_gate_enabled = false;
        assert_eq!(
            read(&mut connection, &advisory, clock, None).await.unwrap()[0]["volume_mt"],
            "13"
        );
        sqlx::query("UPDATE users SET kyc_organization_id=$1")
            .bind(other)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &advisory, clock, None).await.unwrap(),
            json!([])
        );
        sqlx::query("UPDATE users SET kyc_organization_id=NULL,kyc_status='REJECTED'")
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &advisory, clock, None).await.unwrap(),
            json!([])
        );
        sqlx::query("UPDATE kyc_execution_grace SET expires_at='2026-10-24Z'")
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            json!([])
        );
    }

    #[tokio::test]
    async fn demand_declaration_cases() {
        let (mut connection, state, clock) = fixture().await;
        for (product, point, terms_column, expected) in [
            ("UCOME B100", "Singapore", "fame_terms", "UCOME_B100"),
            ("Bio-LNG", "Rotterdam", "lng_terms", "BIO_LNG"),
        ] {
            sqlx::query("TRUNCATE demand_bids")
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::query("INSERT INTO demand_bids (id,product_id,delivery_point_id) SELECT $1,p.id,dp.id FROM products p CROSS JOIN delivery_points dp WHERE p.name=$2 AND dp.name=$3")
                .bind(Uuid::new_v4()).bind(product).bind(point).execute(&mut connection).await.unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap(),
                json!([])
            );
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE demand_bids SET {terms_column}='{{\"side\":\"BID\"}}'"
            )))
            .execute(&mut connection)
            .await
            .unwrap();
            let result = read(&mut connection, &state, clock, None).await.unwrap();
            assert_eq!(result.as_array().unwrap().len(), 1);
            assert_eq!(result[0]["market_product_code"], expected);
            assert_eq!(result[0]["volume_mt"], "10");
            sqlx::query("UPDATE demand_bids SET off_spec=true")
                .execute(&mut connection)
                .await
                .unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap(),
                json!([])
            );
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE demand_bids SET off_spec=false,{terms_column}='{{\"side\":\"ASK\"}}'"
            )))
            .execute(&mut connection)
            .await
            .unwrap();
            assert_eq!(
                read(&mut connection, &state, clock, None).await.unwrap(),
                json!([])
            );
        }
    }

    #[tokio::test]
    async fn demand_aggregate_filter_cases() {
        let (mut connection, state, clock) = fixture().await;
        insert(&mut connection, &state, "SPOT", "REAL", "17").await;
        sqlx::query(
            "UPDATE demand_bids SET price_per_mt_usd=700,created_at='2026-10-21T10:00:00Z'",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        insert(&mut connection, &state, "SPOT", "REAL", "19").await;
        sqlx::query("UPDATE demand_bids SET price_per_mt_usd=600,created_at='2026-10-22T11:00:00Z' WHERE remaining_quantity_mt=19")
            .execute(&mut connection).await.unwrap();
        let expected = json!([{
            "fuel_type":"B30", "region":"Asia", "market_product_code":"B30",
            "delivery_point_id":"78281fb3-e726-5396-802e-34b01fda21e8",
            "delivery_point_name":"Dalian", "availability_window_code":"SPOT",
            "volume_mt":"36", "max_price_per_mt":"700", "urgency":"HIGH", "bid_count":2,
            "earliest_delivery":"Spot", "created_at":"2026-10-22T11:00:00Z",
            "source_kind":"LIVE_ORDER", "scope":"DELIVERY_POINT", "demo_status":"REAL_ONLY", "unknown_count":0
        }]);
        assert_eq!(
            read(&mut connection, &state, clock, None).await.unwrap(),
            expected
        );
        for query in [
            "fuel_type=iof",
            "fuel_type=b3",
            "region=SI",
            "fuel_type=IoF&region=si",
        ] {
            assert_eq!(
                read(&mut connection, &state, clock, Some(query))
                    .await
                    .unwrap(),
                expected,
                "{query}"
            );
        }
        for query in [
            "fuel_type=ethanol",
            "region=europe",
            "fuel_type=b3&region=europe",
        ] {
            assert_eq!(
                read(&mut connection, &state, clock, Some(query))
                    .await
                    .unwrap(),
                json!([]),
                "{query}"
            );
        }
    }

    #[tokio::test]
    async fn demand_sql_urgency_cases() {
        let (mut connection, state, _) = fixture().await;
        insert(&mut connection, &state, "SPOT", "REAL", "1").await;
        insert(&mut connection, &state, "2026-11", "REAL", "20").await;
        insert(&mut connection, &state, "2027-01", "REAL", "30").await;
        insert(&mut connection, &state, "2026-12", "REAL", "10").await;
        for (now, expected) in [
            (
                "2026-10-02T12:00:00Z",
                json!([
                    ["2026-11", "HIGH"],
                    ["SPOT", "HIGH"],
                    ["2026-12", "MEDIUM"],
                    ["2027-01", "LOW"]
                ]),
            ),
            (
                "2026-10-01T12:00:00Z",
                json!([
                    ["SPOT", "HIGH"],
                    ["2026-11", "MEDIUM"],
                    ["2026-12", "MEDIUM"],
                    ["2027-01", "LOW"]
                ]),
            ),
            (
                "2026-10-03T12:00:00Z",
                json!([
                    ["2026-11", "HIGH"],
                    ["SPOT", "HIGH"],
                    ["2027-01", "MEDIUM"],
                    ["2026-12", "MEDIUM"]
                ]),
            ),
        ] {
            let clock = VisibilityClock::at(now.parse().unwrap());
            let result = read(&mut connection, &state, clock, None).await.unwrap();
            let actual: Vec<_> = result
                .as_array()
                .unwrap()
                .iter()
                .map(|row| json!([row["availability_window_code"], row["urgency"]]))
                .collect();
            assert_eq!(json!(actual), expected, "{now}");
        }
    }

    #[tokio::test]
    async fn demand_full_key_cases() {
        let (mut connection, state, clock) = fixture().await;
        sqlx::query("INSERT INTO demand_bids (id,product_id,delivery_point_id,provenance,expires_at) SELECT md5(p.name || dp.name || v.provenance)::uuid,p.id,dp.id,v.provenance,'2030-01-01Z' FROM products p CROSS JOIN delivery_points dp CROSS JOIN (VALUES ('REAL'),('DEMO')) v(provenance) WHERE p.name IN ('B30','B100') AND dp.name IN ('Busan','Dalian')")
            .execute(&mut connection).await.unwrap();
        let result = read(&mut connection, &state, clock, None).await.unwrap();
        let actual: Vec<_> = result
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                json!([
                    row["market_product_code"],
                    row["delivery_point_name"],
                    row["source_kind"],
                    row["volume_mt"]
                ])
            })
            .collect();
        assert_eq!(
            json!(actual),
            json!([
                ["B100", "Busan", "DEMO_SEED", "10"],
                ["B100", "Busan", "LIVE_ORDER", "10"],
                ["B100", "Dalian", "DEMO_SEED", "10"],
                ["B100", "Dalian", "LIVE_ORDER", "10"],
                ["B30", "Busan", "DEMO_SEED", "10"],
                ["B30", "Busan", "LIVE_ORDER", "10"],
                ["B30", "Dalian", "DEMO_SEED", "10"],
                ["B30", "Dalian", "LIVE_ORDER", "10"]
            ])
        );
    }

    #[tokio::test]
    async fn demand_result_limits() {
        let (mut connection, state, clock) = fixture().await;
        let (product, point) = lane(&state);
        sqlx::query(r#"INSERT INTO demand_bids (id,product_id,delivery_point_id,availability_window)
            SELECT md5(n::text)::uuid,$1,$2,to_char(date '2030-01-01' + (n || ' months')::interval,'YYYY-MM')
            FROM generate_series(0,32767) AS n"#).bind(product).bind(point).execute(&mut connection).await.unwrap();
        assert_eq!(
            read(&mut connection, &state, clock, None)
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            256
        );
        assert_eq!(
            read(&mut connection, &state, clock, Some("limit=512"))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            512
        );
        assert_eq!(
            read(&mut connection, &state, clock, Some("limit=1"))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        insert(&mut connection, &state, "SPOT", "REAL", "20").await;
        let error = read(&mut connection, &state, clock, None)
            .await
            .unwrap_err();
        assert_eq!(error.status, 503);
        assert_eq!(error.detail["code"], "READ_RESULT_LIMIT");
        assert_eq!(error.retry_after, Some(1));
    }
}
