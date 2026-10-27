use serde_json::json;
use verdaxis_read::routes::prices::reference_csv;

#[test]
fn reference_csv_matches_python_quoting_and_crlf() {
    let rows = [
        json!({"date":"2026-10-15","product_name":"A, \"quoted\"\nname", "market_product":"FIXTURE", "availability_window":"SPOT", "fuel_type":"type", "delivery_point_name":null, "region":"region", "vwap_usd":"12.50", "total_volume_mt":"2.000", "trade_count":2}),
    ];
    assert_eq!(
        String::from_utf8(reference_csv(&rows).unwrap()).unwrap(),
        "date,product_name,market_product,availability_window,fuel_type,delivery_point_name,region,vwap_usd,volume_mt,trade_count\r\n2026-10-15,\"A, \"\"quoted\"\"\nname\",FIXTURE,SPOT,type,,region,12.50,2.000,2\r\n"
    );
}

#[test]
fn price_rounding_uses_half_even() {
    use verdaxis_read::routes::prices::rounded_price;
    for (input, expected) in [
        ("12.505", "12.50"),
        ("12.515", "12.52"),
        ("-12.505", "-12.50"),
        ("0", "0.00"),
        ("1.2", "1.20"),
    ] {
        assert_eq!(rounded_price(input).unwrap(), expected);
    }
}

#[test]
fn empty_reference_csv_has_header_only() {
    assert_eq!(reference_csv(&[]).unwrap(), b"date,product_name,market_product,availability_window,fuel_type,delivery_point_name,region,vwap_usd,volume_mt,trade_count\r\n");
}

#[test]
fn parameter_errors_follow_python_declaration_order() {
    use verdaxis_read::routes::prices::query::{Surface, validate};
    let error = validate(
        "delivery_point_id=bad&demo_only=bad&skip=-1&limit=0",
        Surface::Tape,
    )
    .unwrap_err();
    let names: Vec<_> = error
        .detail
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["loc"][1].as_str().unwrap())
        .collect();
    assert_eq!(names, ["delivery_point_id", "demo_only", "skip", "limit"]);
}

#[test]
fn uuid_character_errors_precede_short_length_errors() {
    use verdaxis_read::routes::prices::query::{Surface, validate};
    for (value, character, position) in [
        ("x", 'x', 1),
        ("zzzz", 'z', 1),
        ("urn:uuid:x", 'x', 10),
        ("{x}", 'x', 2),
    ] {
        let error = validate(&format!("product_id={value}"), Surface::Summary).unwrap_err();
        assert_eq!(
            error.detail[0]["ctx"]["error"],
            format!("invalid character: found `{character}` at {position}")
        );
    }
    let error = validate("product_id=bad", Surface::Summary).unwrap_err();
    assert_eq!(
        error.detail[0]["ctx"]["error"],
        "invalid length: expected length 32 for simple format, found 3"
    );
}

#[test]
fn numeric_date_uses_microsecond_precision_before_midnight_check() {
    use verdaxis_read::routes::prices::query::{Surface, validate};
    for value in [
        "86400.0000001",
        "-86400.0000001",
        "86400000.0000001",
        "0.0000001",
    ] {
        assert!(
            validate(&format!("date_from={value}"), Surface::Reference).is_ok(),
            "{value}"
        );
    }
    assert!(validate("date_from=86400.000001", Surface::Reference).is_err());
}

#[tokio::test]
async fn typed_validation_precedes_rate_limit_but_filter_validation_does_not() {
    use axum::{Router, body::Body, http::Request, routing::get};
    use tower::ServiceExt;
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(
        Router::new().route("/api/prices", get(|| async { "ok" })),
        "staging",
    );
    for _ in 0..65 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/prices?hours=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 422);
    }
    for _ in 0..60 {
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/prices")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            200
        );
    }
    for (query, expected) in [("hours=0", 422), ("market_product=invalid", 429), ("", 429)] {
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/prices?{query}"))
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            expected
        );
    }
}

#[tokio::test]
async fn unhandled_errors_bypass_response_headers_but_handled_errors_do_not() {
    use axum::{Router, body::Body, http::Request, routing::get};
    use tower::ServiceExt;
    use verdaxis_read::runtime::http::{middleware, unhandled_server_error};
    let app = middleware(
        Router::new()
            .route("/unhandled", get(|| async { unhandled_server_error() }))
            .route(
                "/handled",
                get(|| async { verdaxis_read::ApiError::internal() }),
            ),
        "staging",
    );
    for path in ["/unhandled", "/handled"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("origin", "https://staging.verdaxis.exchange")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 500);
        assert_eq!(
            response.headers().contains_key("x-request-id"),
            path == "/handled"
        );
        assert_eq!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            path == "/handled"
        );
    }
}
