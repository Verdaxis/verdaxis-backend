use verdaxis_read::runtime::http::{RateBucket, admit};
use verdaxis_read::{busy, database_error, request_id, selected_read};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::get,
};
use tower::ServiceExt;

#[tokio::test]
async fn middleware_wraps_errors_and_preflight() {
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(Router::new().route("/fixture", get(|| async { busy() })));
    for origin in [
        "https://staging.verdaxis.exchange",
        "https://denied.invalid",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/fixture")
                    .header("origin", origin)
                    .header("x-request-id", "client-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["x-request-id"], "client-1");
        assert_eq!(response.headers()["retry-after"], "1");
        assert_eq!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            origin == "https://staging.verdaxis.exchange"
        );
        assert_eq!(
            response.headers()["access-control-allow-credentials"],
            "true"
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/fixture")
                .header("origin", "https://staging.verdaxis.exchange")
                .header("access-control-request-method", "GET")
                .header("access-control-request-headers", "X-Test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-headers"], "X-Test");
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "OK");
}

#[test]
fn forwarded_client_uses_final_hop_only_from_loopback() {
    use verdaxis_read::runtime::http::client_ip;
    assert_eq!(
        client_ip(
            Some("127.0.0.1".parse().unwrap()),
            Some("192.0.2.1, 192.0.2.2")
        ),
        "192.0.2.2"
    );
    assert_eq!(
        client_ip(Some("192.0.2.3".parse().unwrap()), Some("192.0.2.2")),
        "192.0.2.3"
    );
    assert_eq!(
        client_ip(Some("127.0.0.1".parse().unwrap()), Some("invalid")),
        "127.0.0.1"
    );
}

#[tokio::test]
async fn rate_bursts_share_one_client_route_bucket() {
    let app = verdaxis_read::runtime::http::middleware(
        Router::new().route("/api/prices/reference/export", get(|| async { "csv" })),
    );
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..25 {
        let app = app.clone();
        jobs.spawn(async move {
            app.oneshot(
                Request::builder()
                    .uri("/api/prices/reference/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        });
    }
    let mut accepted = 0;
    while let Some(response) = jobs.join_next().await {
        let response = response.unwrap();
        if response.status() == 200 {
            accepted += 1;
        } else {
            assert_eq!(response.status(), 429);
            assert!(!response.headers().contains_key("retry-after"));
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"detail":"Rate limit exceeded: 10 per 1 minute"})
            );
        }
    }
    assert_eq!(accepted, 10);
}

#[tokio::test]
async fn router_health_methods_and_empty_routes() {
    let config = verdaxis_read::Config {
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
    let app = verdaxis_read::router(state);
    for (method, path, status) in [
        ("GET", "/health/live", 200),
        ("HEAD", "/health/live", 405),
        ("POST", "/health/live", 405),
        ("GET", "/health/live/", 307),
        ("GET", "/api/ports", 404),
        ("POST", "/mcp", 404),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
        if status == 405 {
            assert_eq!(response.headers()["allow"], "GET");
        }
        if status == 307 {
            assert_eq!(
                response.headers()["location"],
                "http://localhost/health/live"
            );
        }
    }
}

#[test]
fn query_validation_is_python_shaped() {
    use verdaxis_read::runtime::http::{QueryFields, QueryKind};
    let fields = QueryFields::parse("limit=bad&extra=ignored&limit=0");
    let error = fields
        .validate(
            "limit",
            QueryKind::Integer {
                minimum: 1,
                maximum: 100,
            },
            true,
        )
        .unwrap_err();
    assert_eq!(error.status, 422);
    assert_eq!(
        error.detail,
        serde_json::json!([{"type":"greater_than_equal","loc":["query","limit"],"msg":"Input should be greater than or equal to 1","input":"0","ctx":{"ge":1}}])
    );
    let error = QueryFields::parse("")
        .validate("product_id", QueryKind::Uuid, true)
        .unwrap_err();
    assert_eq!(
        error.detail,
        serde_json::json!([{"type":"missing","loc":["query","product_id"],"msg":"Field required","input":null}])
    );
}

#[test]
fn request_ids_are_bounded() {
    assert_eq!(request_id(Some("client-1:a.b")), "client-1:a.b");
    for value in [
        "",
        "bad id",
        "bad\nheader",
        "abcdefghijklmnopqrstuvwxyz01234567890",
    ] {
        assert_ne!(request_id(Some(value)), value);
    }
    assert!(uuid::Uuid::parse_str(&request_id(None)).is_ok());
}

#[test]
fn selected_paths_match_python() {
    for path in [
        "/api/availability",
        "/api/orderbook",
        "/api/listings",
        "/api/ports/SGSIN",
        "/api/prices/reference/export",
    ] {
        assert!(selected_read("GET", path));
        assert!(!selected_read("POST", path));
        assert!(!selected_read("HEAD", path));
    }
    for path in [
        "/api/news",
        "/health/ready",
        "/api/orderbook/my",
        "/api/listings/my",
        "/api/ports/SGSIN/extra",
        "/api/ports/",
    ] {
        assert!(!selected_read("GET", path));
    }
}

#[test]
fn database_errors_keep_market_scope() {
    for code in ["55P03", "40P01", "40001", "57014"] {
        let error = database_error("/api/prices", "GET", Some(code));
        assert_eq!(error.status, 503);
        assert_eq!(error.retry_after, Some(1));
        assert_eq!(error.detail, "Market is temporarily busy; retry shortly.");
        assert_eq!(database_error("/api/ports", "GET", Some(code)).status, 500);
    }
    assert_eq!(
        database_error("/api/prices", "POST", Some("57014")).status,
        500
    );
    assert_eq!(
        database_error("/api/prices-extra", "GET", Some("55P03")).status,
        500
    );
    assert_eq!(
        busy().detail,
        "Public market data is temporarily busy; retry shortly."
    );
}

#[test]
fn exact_limit_and_reset() {
    for limit in [60, 30, 10] {
        let mut bucket = RateBucket {
            started_ms: 1_000,
            used: 0,
        };
        for _ in 0..limit {
            assert!(admit(&mut bucket, limit, 1_000));
        }
        assert!(!admit(&mut bucket, limit, 60_999));
        assert_eq!(bucket.started_ms, 1_000);
        assert!(admit(&mut bucket, limit, 61_000));
    }
}
