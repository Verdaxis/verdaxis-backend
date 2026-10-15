use serde_json::json;

#[test]
fn protocol_adapter_preserves_python_envelope_order() {
    let input = json!({"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"hello"}],"structuredContent":{"quantity":"1.00"},"isError":false}});
    let result = verdaxis_read::mcp::transport::python_envelope(input, "tools/call");
    assert_eq!(
        result.to_string(),
        r#"{"jsonrpc":"2.0","id":7,"result":{"content":[{"text":"hello","type":"text"}],"isError":false,"structuredContent":{"quantity":"1.00"}}}"#
    );
}

#[test]
fn voyage_decimal_context_matches_python_vectors() {
    use verdaxis_read::mcp::decimal::Decimal;
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../deploy/rust/voyage-decimal-vectors.json"
    ))
    .unwrap();
    for vector in vectors.as_array().unwrap() {
        let left = Decimal::parse(vector["left"].as_str().unwrap()).unwrap();
        let right = Decimal::parse(vector["right"].as_str().unwrap()).unwrap();
        let result = match vector["operation"].as_str().unwrap() {
            "add" => left.add(&right),
            "subtract" => left.subtract(&right),
            "multiply" => left.multiply(&right),
            "divide" => left.divide(&right).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(
            result.text(),
            vector["result"].as_str().unwrap(),
            "{vector}"
        );
        assert_eq!(
            result.quantize(2).unwrap(),
            vector["money"].as_str().unwrap(),
            "{vector}"
        );
    }
}

use verdaxis_read::mcp::transport::{host_allowed, reject_origin};

#[test]
fn transport_rejects_every_origin_and_uses_exact_hosts() {
    use axum::http::HeaderMap;
    let mut headers = HeaderMap::new();
    assert!(reject_origin(&headers).is_none());
    for origin in ["", "null", "https://evil.example"] {
        headers.insert("origin", origin.parse().unwrap());
        assert_eq!(reject_origin(&headers).unwrap().status(), 403);
    }
    for host in ["127.0.0.1:43210", "localhost:43210", "testserver"] {
        assert!(host_allowed("test", host));
        assert!(!host_allowed("staging", host));
    }
    for host in ["localhost", "evil.example", "localhost.evil.example:43210"] {
        assert!(!host_allowed("test", host));
    }
    assert!(host_allowed("staging", "api-staging.verdaxis.exchange"));
    assert!(!host_allowed(
        "staging",
        "api-staging.verdaxis.exchange:443"
    ));
    assert!(!host_allowed("production", "api-staging.verdaxis.exchange"));
}

use verdaxis_read::mcp::prices::project_slice;

#[test]
fn slice_keeps_both_provenances_and_numeric_scale_without_identifiers() {
    let rows = json!([
        {"side":"BID","price":"12.00","quantity":"2.000","provenance":"REAL","id":"private"},
        {"side":"ASK","price":"15.00","quantity":"3.000","provenance":"DEMO","organization_id":"private"},
        {"side":"ASK","price":"14.00","quantity":"4.000","provenance":"REAL"}
    ]);
    let value = project_slice(
        "fuel",
        "port",
        "SPOT",
        rows.as_array().unwrap(),
        None,
        "2026-10-15T12:00:00Z",
    )
    .unwrap();
    assert_eq!(value["best_bid_usd_per_mt"], "12.00");
    assert_eq!(value["best_ask_usd_per_mt"], "14.00");
    assert_eq!(value["ask_quantity_mt"], "7.000");
    assert_eq!(value["demo_status"], "MIXED");
    assert_eq!(value["ask_order_count"], 2);
    assert!(!value.to_string().contains("private"));
    assert!(!value.to_string().contains("organization_id"));
}

#[test]
fn empty_slice_retains_null_prices_and_zero_quantities() {
    let value = project_slice(
        "fuel",
        "port",
        "SPOT",
        &[],
        Some("12.00"),
        "2026-10-15T12:00:00Z",
    )
    .unwrap();
    assert!(value["best_bid_usd_per_mt"].is_null());
    assert_eq!(value["bid_quantity_mt"], "0");
    assert_eq!(value["demo_status"], "NOT_APPLICABLE");
    assert_eq!(value["benchmark_usd_per_mt"], "12.00");
    assert_eq!(
        value["note"],
        "This slice shows no open orders; the benchmark is an indicative reference, not an executable price."
    );
}

use verdaxis_read::{AppState, Config, MarketPolicy};
fn state() -> AppState {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        pool_max: 9,
        python_budget: 52,
        configured_max_connections: 100,
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .connect_lazy(&config.database_url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    let policy: MarketPolicy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    AppState::new(config, pools, policy).unwrap()
}

#[tokio::test]
async fn catalog_and_protocol_bypass_saturated_database_admission() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let state = state();
    let permit = state.reads.clone().acquire_many_owned(8).await.unwrap();
    let app = verdaxis_read::mcp::mcp_router().with_state(state.clone());
    for (method, params) in [
        (
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}),
        ),
        ("tools/list", json!({})),
        ("tools/call", json!({"name":"list_markets","arguments":{}})),
        (
            "tools/call",
            json!({"name":"get_market_slice","arguments":{"market_product":"fixture","delivery_point":"fixture","availability_window":"SPOT"}}),
        ),
        (
            "tools/call",
            json!({"name":"plan_voyage_fuel","arguments":{"request":{}}}),
        ),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "testserver")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if params["name"] == "get_market_slice" || params["name"] == "plan_voyage_fuel" {
            assert_eq!(value["result"]["isError"], true);
            assert!(
                String::from_utf8_lossy(&bytes)
                    .contains("Verdaxis market tools are busy; retry shortly.")
            );
        } else {
            assert!(value.get("error").is_none());
            assert_ne!(value["result"]["isError"], true);
        }
        assert_eq!(state.reads.available_permits(), 0);
    }
    drop(permit);
    assert_eq!(state.reads.available_permits(), 8);
}
