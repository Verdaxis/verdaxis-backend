//! Producer read contracts and the populated PostgreSQL matrix.

pub use verdaxis_read::{ApiError, AppState, routes, runtime};

// Exercise the lane module before the integration owner registers its export.
#[path = "../src/routes/producers.rs"]
mod producers;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use producers::{ProducerQuery, ProjectStatus};
use serde_json::{Value, json};
use tower::ServiceExt;

#[test]
fn producer_filter_cases() {
    let empty = ProducerQuery::parse("").unwrap();
    assert_eq!(empty.cod_year_min, None);
    assert_eq!(empty.cod_year_max, None);
    for (name, expected) in [
        ("ANNOUNCED", ProjectStatus::Announced),
        ("UNDER_CONSTRUCTION", ProjectStatus::UnderConstruction),
        ("OPERATIONAL", ProjectStatus::Operational),
        ("CANCELLED", ProjectStatus::Cancelled),
    ] {
        assert_eq!(
            ProducerQuery::parse(&format!("status={name}"))
                .unwrap()
                .status,
            Some(expected)
        );
    }
    let query = ProducerQuery::parse(
        "fuel_type=Green+Methanol&country=nether&cod_year_min=0&cod_year_max=2030",
    )
    .unwrap();
    assert_eq!(query.fuel_type.as_deref(), Some("Green Methanol"));
    assert_eq!(query.country.as_deref(), Some("nether"));
    assert_eq!(query.cod_year_min, Some(0));
    assert_eq!(query.cod_year_max, Some(2030));
    assert!(ProducerQuery::parse("cod_year_min=2030&cod_year_max=2030").is_ok());
    for raw in [
        "status=",
        "status=operational",
        "status=OTHER",
        "cod_year_min=",
        "cod_year_max=2.5",
        "cod_year_min=2147483648",
        "cod_year_min=2031&cod_year_max=2030",
    ] {
        let error = ProducerQuery::parse(raw).unwrap_err();
        assert_eq!(error.status, 422, "{raw}");
        assert_eq!(error.detail["code"], "INVALID_INPUT", "{raw}");
    }
}

#[test]
fn producer_projection_cases() {
    let row = producers::ProducerProject {
        id: uuid::Uuid::nil(),
        name: "Harbour project".into(),
        fuel_type: "Methanol".into(),
        capacity_kt_per_year: Some("99999999.99".into()),
        country: "NL".into(),
        region: None,
        lat: Some(1.25),
        lng: Some(103.8),
        cod_date: None,
        cod_year: None,
        status: "ANNOUNCED".into(),
        data_source: Some("manual".into()),
        gena_project_id: None,
        organization_id: None,
        feedstock: None,
        technology: None,
        carbon_intensity_gco2_mj: Some("0.00".into()),
        notes: None,
        created_at: "2026-10-23T00:00:00Z".parse().unwrap(),
    };
    let value = serde_json::to_value(&row).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 19);
    assert_eq!(value["capacity_kt_per_year"], "99999999.99");
    assert_eq!(value["carbon_intensity_gco2_mj"], "0.00");
    assert_eq!(value["lat"], 1.25);
    assert_eq!(value["lng"], 103.8);
    for key in ["cod_date", "cod_year", "region", "organization_id", "notes"] {
        assert!(value[key].is_null());
    }
    assert!(value.get("location").is_none());
}

fn state(url: &str) -> AppState {
    let config: verdaxis_read::Config = serde_json::from_value(json!({
        "kyc_execution_gate_enabled": true, "db_session_time_zone": "UTC",
        "environment": "test", "release_sha": verdaxis_read::compiled_sha(),
        "migration_revision": "p5_20261020_read_indexes", "database_url": url,
        "bind": "127.0.0.1:0", "read_admission_slots": 2, "pool_max": 3,
        "python_budget": 52, "configured_max_connections": 100
    }))
    .unwrap();
    let pools = runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_lazy(url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(url)
            .unwrap(),
    };
    AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap()
}

async fn get(state: &AppState, query: &str) -> (axum::http::StatusCode, Value) {
    let router = producers::routes().with_state(state.clone());
    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/producers{query}"))
                .header("authorization", "Bearer unused-public-header")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    if status == 503 {
        assert_eq!(response.headers()["retry-after"], "1");
    }
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn producer_filter_cases_http() {
    let state = state("postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test");
    for query in ["?status=OTHER", "?cod_year_min=2031&cod_year_max=2030"] {
        let (status, body) = get(&state, query).await;
        assert_eq!(status, 422);
        assert_eq!(body["detail"]["code"], "INVALID_INPUT");
    }
}

#[tokio::test]
#[ignore = "requires an explicitly provisioned disposable PostgreSQL database"]
async fn producer_result_limits() {
    let app_url = std::env::var("PRODUCER_TEST_APP_URL").expect("explicit disposable app URL");
    let writer_url =
        std::env::var("PRODUCER_TEST_WRITER_URL").expect("explicit disposable writer URL");
    for url in [&app_url, &writer_url] {
        let parsed = url::Url::parse(url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(parsed.path().ends_with("_test"));
        assert_ne!(parsed.port_or_known_default(), Some(5432));
    }
    let writer = sqlx::PgPool::connect(&writer_url).await.unwrap();
    sqlx::query("TRUNCATE producer_projects")
        .execute(&writer)
        .await
        .unwrap();
    sqlx::query("INSERT INTO producer_projects (id,name,fuel_type,country,cod_year,status,location,capacity_kt_per_year,carbon_intensity_gco2_mj) VALUES \
        ('00000000-0000-0000-0000-000000000001','Same','Green Methanol','Netherlands',2030,'OPERATIONAL',ST_SetSRID(ST_MakePoint(0,0),4326)::geography,99999999.99,0), \
        ('00000000-0000-0000-0000-000000000002','Same','Methanol','Netherlands',2030,'ANNOUNCED',NULL,NULL,NULL), \
        ('00000000-0000-0000-0000-000000000003','Later','Ammonia','Norway',NULL,'UNDER_CONSTRUCTION',NULL,NULL,NULL), \
        ('00000000-0000-0000-0000-000000000004','Early','Methanol','Netherlands',0,'CANCELLED',NULL,NULL,NULL)")
        .execute(&writer).await.unwrap();
    sqlx::query("INSERT INTO producer_projects (id,name,fuel_type,country,cod_year,status,location,cod_date,region,feedstock,technology,data_source,notes) VALUES \
        ('00000000-0000-0000-0000-000000000006','Alpha','Methanol','Netherlands',2030,'ANNOUNCED',ST_SetSRID(ST_MakePoint(103.8,1.25),4326)::geography,'2030-06-15','Europe','Biomass','Gasification','manual','Project metadata'), \
        ('00000000-0000-0000-0000-000000000005','Zulu','Methanol','Netherlands',2030,'ANNOUNCED',NULL,NULL,NULL,NULL,NULL,NULL,NULL)")
        .execute(&writer).await.unwrap();
    let state = state(&app_url);
    let (status, body) = get(&state, "").await;
    assert_eq!(status, 200);
    let items = body.as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Early", "Alpha", "Same", "Same", "Zulu", "Later"]
    );
    assert_eq!(items[1]["id"], "00000000-0000-0000-0000-000000000006");
    assert_eq!(items[1]["lat"], 1.25);
    assert_eq!(items[1]["lng"], 103.8);
    assert_eq!(items[1]["cod_date"], "2030-06-15");
    assert_eq!(items[1]["region"], "Europe");
    assert_eq!(items[1]["feedstock"], "Biomass");
    assert_eq!(items[1]["technology"], "Gasification");
    assert_eq!(items[1]["data_source"], "manual");
    assert_eq!(items[1]["notes"], "Project metadata");
    for item in &items[1..5] {
        assert_eq!(item["cod_year"], 2030);
    }
    assert_eq!(items[2]["id"], "00000000-0000-0000-0000-000000000001");
    assert_eq!(items[3]["id"], "00000000-0000-0000-0000-000000000002");
    assert_eq!(items[2]["lat"], 0.0);
    assert_eq!(items[2]["lng"], 0.0);
    assert_eq!(items[2]["capacity_kt_per_year"], "99999999.99");
    assert!(items[3]["lat"].is_null());
    assert!(items[3]["capacity_kt_per_year"].is_null());
    assert!(items[5]["cod_year"].is_null());
    for (query, count) in [
        ("?fuel_type=mEtHaNoL&country=ETHER", 5),
        ("?status=OPERATIONAL", 1),
        ("?status=ANNOUNCED", 3),
        ("?status=UNDER_CONSTRUCTION", 1),
        ("?status=CANCELLED", 1),
        ("?cod_year_max=0", 1),
        ("?cod_year_min=0&cod_year_max=0", 1),
        ("?cod_year_min=2030&cod_year_max=2030", 4),
        ("?cod_year_min=2031", 0),
    ] {
        let (status, body) = get(&state, query).await;
        assert_eq!(status, 200, "{query}");
        assert_eq!(body.as_array().unwrap().len(), count, "{query}");
    }
    sqlx::query("INSERT INTO producer_projects (name,fuel_type,country,cod_year,status) SELECT 'Bulk ' || n,'Methanol','NL',2028,'ANNOUNCED' FROM generate_series(1,4994) n")
        .execute(&writer).await.unwrap();
    let (status, body) = get(&state, "").await;
    assert_eq!(status, 200);
    assert_eq!(body.as_array().unwrap().len(), 5000);
    sqlx::query("INSERT INTO producer_projects (name,fuel_type,country,status) VALUES ('Final','Ammonia','NO','ANNOUNCED')")
        .execute(&writer).await.unwrap();
    let (status, body) = get(&state, "").await;
    assert_eq!(status, 503);
    assert_eq!(body["detail"]["code"], "READ_RESULT_LIMIT");
    assert!(body.get("items").is_none());
    let (status, body) = get(&state, "?status=OPERATIONAL").await;
    assert_eq!(status, 200);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM producer_projects")
            .fetch_one(&writer)
            .await
            .unwrap(),
        5001
    );
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_indexes WHERE indexname IN ('ix_p5_producers_cod_name_id','ix_p5_producers_status_cod_id')").fetch_one(&writer).await.unwrap(), 2);
    writer.close().await;
}
