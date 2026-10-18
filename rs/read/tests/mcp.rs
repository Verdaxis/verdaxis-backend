use serde_json::json;

fn shared_meaning(pair: serde_json::Value) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new(root.join("venv/bin/python"))
        .current_dir(root)
        .args(["-m", "scripts.rust_contract.compare_values"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(pair.to_string().as_bytes())
        .unwrap();
    child.wait().unwrap().success()
}

#[tokio::test]
async fn modern_success_and_header_precedence_match_python() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = verdaxis_read::mcp::mcp_router().with_state(state());
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../deploy/rust/mcp-http-vectors.json")).unwrap();
    let mut failures = Vec::new();
    for vector in vectors.as_array().unwrap() {
        let mut request = Request::builder().method("POST").uri("/mcp");
        if vector["headers"].get("host").is_none() {
            request = request.header("host", "testserver");
        }
        for (name, value) in vector["headers"].as_object().unwrap() {
            request = request.header(name, value.as_str().unwrap());
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(vector["body"].as_str().unwrap().to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 1000000).await.unwrap();
        let Ok(expected) =
            serde_json::from_str::<serde_json::Value>(vector["response"].as_str().unwrap())
        else {
            assert_eq!(status as u64, vector["status"].as_u64().unwrap());
            assert_eq!(
                bytes.as_ref(),
                vector["response"].as_str().unwrap().as_bytes()
            );
            continue;
        };
        let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let request: serde_json::Value =
            serde_json::from_str(vector["body"].as_str().unwrap()).unwrap_or(json!({}));
        let tool = request["params"]["name"].as_str();
        let matches = if actual["result"]["isError"] == true && tool.is_some() {
            shared_meaning(
                json!({"expected": expected["result"]["content"][0]["text"].as_str().unwrap().split_once(": ").unwrap().1,
                "actual":actual["result"]["content"][0]["text"], "route_id":format!("P1-tool-{}",tool.unwrap()),
                "diagnostic_context":{"tool":tool.unwrap(),"arguments":request["params"]["arguments"]}}),
            )
        } else {
            shared_meaning(
                json!({"expected":expected,"actual":actual,"raw_envelope":true,
                "route_id":tool.map_or("P1-mcp-protocol".to_owned(), |tool| format!("P1-tool-{tool}"))}),
            )
        };
        if status as u64 != vector["status"].as_u64().unwrap() || !matches {
            failures.push(format!(
                "{}: status {status}, {}",
                vector["name"],
                String::from_utf8_lossy(&bytes)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn root_arguments_match_python_method_validation() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = verdaxis_read::mcp::mcp_router().with_state(state());
    for arguments in [json!([]), json!(42), json!("bad"), json!(true), json!(null)] {
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"plan_voyage_fuel","arguments":arguments}});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "testserver")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 100000).await.unwrap();
        if arguments.is_null() {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["result"]["isError"], true);
            let text = value["result"]["content"][0]["text"].as_str().unwrap();
            let errors: serde_json::Value =
                serde_json::from_str(text.strip_prefix("Validation failed: ").unwrap()).unwrap();
            assert_eq!(
                errors,
                json!([{"type":"missing","loc":["request"],"msg":"Field required","input":{}}])
            );
        } else {
            assert_eq!(bytes.as_ref(), br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Invalid request parameters","data":""}}"#, "{arguments}");
        }
    }
}

#[tokio::test]
async fn protocol_errors_precede_tool_validation() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = verdaxis_read::mcp::mcp_router().with_state(state());
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../deploy/rust/mcp-protocol-vectors.json"
    ))
    .unwrap();
    for vector in vectors.as_array().unwrap() {
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "testserver")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (name, value) in vector["headers"].as_object().unwrap() {
            request = request.header(name, value.as_str().unwrap());
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(vector["body"].to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{}", vector["name"]);
        let bytes = to_bytes(response.into_body(), 100000).await.unwrap();
        let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            actual,
            json!({"jsonrpc":"2.0","id":vector["id"],"error":vector["error"]}),
            "{}",
            vector["name"]
        );
    }
}

#[tokio::test]
async fn stateless_transport_matches_python_terminal_responses() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = verdaxis_read::mcp::mcp_router().with_state(state());
    for (method, body, status, message) in [
        (
            "POST",
            "{",
            400,
            "Parse error: EOF while parsing an object at line 1 column 1",
        ),
        (
            "DELETE",
            "",
            405,
            "Method Not Allowed: Session termination not supported",
        ),
        (
            "POST",
            r#"{"jsonrpc":"2.0","id":3,"method":"nope","params":{}}"#,
            200,
            "Method not found",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/mcp")
                    .header("host", "testserver")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let bytes = to_bytes(response.into_body(), 100000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"]["message"], message);
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .header("host", "testserver")
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(
        response.headers()["cache-control"],
        "no-cache, no-transform"
    );
}

#[test]
fn protocol_adapter_preserves_protocol_envelope_order() {
    let input = json!({"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"hello"}],"structuredContent":{"quantity":"1.00"},"isError":false}});
    let result = verdaxis_read::mcp::transport::protocol_envelope(input, "tools/call");
    assert_eq!(
        result,
        json!({"jsonrpc":"2.0","id":7,"result":{"content":[{"text":"hello","type":"text"}],"isError":false,"structuredContent":{"quantity":"1.00"}}})
    );
}

#[test]
fn tool_schemas_use_python_model_field_order() {
    let input = json!({"id":1,"result":{"tools":[{"name":"list_markets"}]}});
    let result = verdaxis_read::mcp::transport::protocol_envelope(input, "tools/list");
    let tool = &result["result"]["tools"][0];
    assert_eq!(
        tool["inputSchema"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["properties", "title", "type"])
    );
    assert_eq!(
        tool["annotations"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["destructiveHint", "idempotentHint", "readOnlyHint"])
    );
}

#[test]
fn voyage_decimal_context_matches_python_vectors() {
    use verdaxis_read::market::amount::Amount as Decimal;
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
            result.compare(&Decimal::parse(vector["result"].as_str().unwrap()).unwrap()),
            std::cmp::Ordering::Equal,
            "{vector}"
        );
        assert_eq!(
            Decimal::parse(&result.quantize(2).unwrap())
                .unwrap()
                .compare(&Decimal::parse(vector["money"].as_str().unwrap()).unwrap()),
            std::cmp::Ordering::Equal,
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
    assert_eq!(
        verdaxis_read::market::amount::Amount::parse(value["ask_quantity_mt"].as_str().unwrap())
            .unwrap()
            .compare(&verdaxis_read::market::amount::Amount::parse("7").unwrap()),
        std::cmp::Ordering::Equal
    );
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
    state_with_pool().0
}

fn state_with_pool() -> (AppState, sqlx::PgPool) {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
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
    let policy: MarketPolicy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let pool = pools.read.clone();
    (AppState::new(config, pools, policy).unwrap(), pool)
}

#[tokio::test]
async fn catalog_and_protocol_bypass_saturated_database_admission() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let state = state();
    let permit = state
        .reads
        .clone()
        .acquire_many_owned(state.config.read_admission_slots)
        .await
        .unwrap();
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
            json!({"name":"plan_voyage_fuel","arguments":{"request":{"vessel":{"speed_knots":"14","sea_consumption_mt_per_day":"20","compatible_fuels":["VLSFO"]},"legs":[{"from_port":"Singapore","to_port":"Rotterdam"}],"start_date":"2026-10-15"}}}),
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
    assert_eq!(
        state.reads.available_permits(),
        state.config.read_admission_slots as usize
    );
}

#[tokio::test]
async fn invalid_matrix_inputs_precede_saturated_admission() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let state = state();
    let _permit = state
        .reads
        .clone()
        .acquire_many_owned(state.config.read_admission_slots)
        .await
        .unwrap();
    let app = verdaxis_read::mcp::mcp_router().with_state(state.clone());
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../deploy/rust/mcp-validation-vectors.json"
    ))
    .unwrap();
    for vector in vectors.as_array().unwrap() {
        let Some(expected) = vector["error"].as_str() else {
            continue;
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "testserver")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":vector["tool"],"arguments":vector["arguments"]}})
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 100000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            shared_meaning(
                json!({"expected":expected,"actual":value["result"]["content"][0]["text"],
            "route_id":format!("P1-tool-{}",vector["tool"].as_str().unwrap()),
            "diagnostic_context":{"tool":vector["tool"],"arguments":vector["arguments"]}})
            ),
            "{}",
            vector["name"]
        );
        assert_eq!(state.reads.available_permits(), 0);
    }
}

#[tokio::test]
async fn database_unavailability_is_sanitized_and_catalog_stays_available() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let (state, pool) = state_with_pool();
    pool.close().await;
    let app = verdaxis_read::mcp::mcp_router().with_state(state.clone());
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/rust_contract/cases.json")).unwrap();
    for id in ["M04-list-markets", "M05-slice-mixed", "M12-voyage-methanol"] {
        let case = cases["groups"]["mcp"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "testserver")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json")
                    .body(Body::from(case["body"].as_str().unwrap().to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 100000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if id == "M04-list-markets" {
            assert_eq!(value["result"]["isError"], false);
        } else {
            assert_eq!(value["result"]["isError"], true);
            assert!(
                String::from_utf8_lossy(&bytes).contains("Market data is temporarily unavailable.")
            );
            assert!(!String::from_utf8_lossy(&bytes).contains("fixture_pw"));
            assert!(!String::from_utf8_lossy(&bytes).contains("postgres"));
        }
        assert_eq!(
            state.reads.available_permits(),
            state.config.read_admission_slots as usize
        );
    }
}
