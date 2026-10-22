//! Task 8a vectors for the public fuel-intensity read.
//!
//! These checks pin the catalog, unit and caveat the client displays.

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::json;
use tower::ServiceExt;
use verdaxis_read::routes::compliance_fuels::{PATH, fuels_response, routes};

#[test]
fn compliance_fuels_cases() {
    let body = fuels_response();
    let expected = json!({
        "VLSFO": "91.16", "LSMGO": "91.16", "LNG": "69.00",
        "Bio-LNG": "28.00", "Methanol": "31.00", "E-Methanol": "8.00",
        "Ammonia": "18.00", "Biofuel": "35.00", "Hydrogen": "10.00"
    });
    assert_eq!(body["fuels"], expected);
    let fuels = body["fuels"].as_object().expect("a fuel map");
    assert_eq!(
        fuels.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "VLSFO",
            "LSMGO",
            "LNG",
            "Bio-LNG",
            "Methanol",
            "E-Methanol",
            "Ammonia",
            "Biofuel",
            "Hydrogen"
        ]
    );
    assert_eq!(body["unit"], json!("gCO2eq/MJ"));
    let source = body["source"].as_str().expect("a source note");
    assert!(source.contains("not verified"), "{source}");
    assert!(source.contains("Regulation (EU) 2023/1805"), "{source}");
    assert!(source.to_lowercase().contains("illustrative"), "{source}");
}

#[tokio::test]
async fn compliance_fuels_cases_are_public() {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        compliance_eur_usd_rate: None,
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.read_admission_slots)
            .connect_lazy(&config.database_url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    let state = verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let app = verdaxis_read::runtime::http::middleware(routes().with_state(state));
    for (uri, bearer) in [
        (PATH.to_owned(), false),
        (format!("{PATH}?window=SPOT&unknown=1"), false),
        (PATH.to_owned(), true),
    ] {
        let mut request = Request::builder().uri(&uri);
        if bearer {
            request = request.header("Authorization", "Bearer x");
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{uri}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let bytes = to_bytes(response.into_body(), 16_384).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON body");
        assert_eq!(value, fuels_response());
    }
}
