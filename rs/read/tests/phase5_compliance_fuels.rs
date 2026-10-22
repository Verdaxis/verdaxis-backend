//! Task 8a vectors for the public fuel-intensity read.
//!
//! The route is a constant table with no database and no caller identity, so
//! these checks pin the table, the unit and the caveat the client displays.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::get,
};
use serde_json::json;
use tower::ServiceExt;
use verdaxis_read::compliance::scoring::FUEL_INTENSITIES;
use verdaxis_read::routes::compliance_fuels::{fuels, fuels_response};

#[test]
fn compliance_fuels_cases() {
    let body = fuels_response();

    // Every known fuel is listed once with its exact intensity text.
    let fuels = body["fuels"].as_object().expect("a fuel map");
    assert_eq!(fuels.len(), FUEL_INTENSITIES.len());
    for (name, intensity) in FUEL_INTENSITIES {
        assert_eq!(
            fuels.get(name),
            Some(&json!(intensity.to_string())),
            "{name}"
        );
    }
    assert_eq!(fuels["VLSFO"], json!("91.16"));
    assert_eq!(fuels["LSMGO"], json!("91.16"));
    assert_eq!(fuels["LNG"], json!("69.00"));
    assert_eq!(fuels["E-Methanol"], json!("8.00"));
    assert_eq!(fuels["Hydrogen"], json!("10.00"));

    // The wire unit and the illustrative-source caveat the client shows.
    assert_eq!(body["unit"], json!("gCO2eq/MJ"));
    let source = body["source"].as_str().expect("a source note");
    assert!(source.contains("not verified"), "{source}");
    assert!(source.contains("Regulation (EU) 2023/1805"), "{source}");
    assert!(source.to_lowercase().contains("illustrative"), "{source}");
}

#[tokio::test]
async fn compliance_fuels_cases_are_public() {
    // No bearer token, no account and no query input: the mount is a public
    // GET that ignores unknown parameters.
    let app = verdaxis_read::runtime::http::middleware(
        Router::new().route("/api/compliance/fuels", get(fuels)),
    );
    for uri in [
        "/api/compliance/fuels",
        "/api/compliance/fuels?window=SPOT&unknown=1",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
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
        assert_eq!(value["unit"], json!("gCO2eq/MJ"));
        assert_eq!(
            value["fuels"].as_object().unwrap().len(),
            FUEL_INTENSITIES.len()
        );
    }
}
