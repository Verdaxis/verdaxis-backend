//! Task 8 pure vectors for the simple public reads (R01-R05).
//!
//! The recorded corpus covers an empty port and news table, so these vectors
//! pin the parts of the wire contract the corpus cannot reach offline: the two
//! Python date formats, the port response's field order and nulls, and the
//! Python-compatible query validation envelope. No database is needed.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use verdaxis_read::Json;
use verdaxis_read::routes::news::{NewsRow, isoformat_utc, news_query, news_response};
use verdaxis_read::routes::ports::{PortRow, port_response, pydantic_datetime};

#[test]
fn news_padded_integer() {
    assert_eq!(news_query("limit=%2020%20").unwrap().limit, 20);
}

#[test]
fn news_integral_decimal() {
    assert_eq!(news_query("limit=20.0").unwrap().limit, 20);
}

#[test]
fn news_integer_separators() {
    assert_eq!(news_query("limit=1_0").unwrap().limit, 10);
}

#[test]
fn news_collects_errors_in_declaration_order() {
    let error = news_query("limit=0&min_relevance=0").unwrap_err();
    assert_eq!(
        error.detail,
        json!([
            {"type":"greater_than_equal","loc":["query","limit"],"msg":"Input should be greater than or equal to 1","input":"0","ctx":{"ge":1}},
            {"type":"greater_than_equal","loc":["query","min_relevance"],"msg":"Input should be greater than or equal to 1","input":"0","ctx":{"ge":1}}
        ])
    );
}

#[test]
fn news_large_integer_has_bound_error() {
    let error = news_query("limit=9223372036854775808").unwrap_err();
    assert_eq!(
        error.detail,
        json!([{
            "type":"less_than_equal","loc":["query","limit"],
            "msg":"Input should be less than or equal to 100",
            "input":"9223372036854775808","ctx":{"le":100}
        }])
    );
}

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid test instant")
        .with_timezone(&Utc)
}

#[test]
fn news_dates_use_python_isoformat() {
    assert_eq!(
        isoformat_utc(utc("2026-10-15T12:00:00Z")),
        "2026-10-15T12:00:00+00:00"
    );
    assert_eq!(
        isoformat_utc(utc("2026-10-15T12:00:00.123456Z")),
        "2026-10-15T12:00:00.123456+00:00"
    );
}

#[test]
fn port_dates_use_pydantic_rfc3339() {
    assert_eq!(
        pydantic_datetime(utc("2026-10-15T12:00:00Z")),
        "2026-10-15T12:00:00Z"
    );
    assert_eq!(
        pydantic_datetime(utc("2026-10-15T12:00:00.123456Z")),
        "2026-10-15T12:00:00.123456Z"
    );
}

#[test]
fn port_response_keeps_python_field_order_and_nulls() {
    let row = PortRow {
        id: "NLRTM".to_owned(),
        name: "Rotterdam".to_owned(),
        country: "NL".to_owned(),
        timezone: Some("Europe/Amsterdam".to_owned()),
        is_active: true,
        lng: Some(4.4777),
        lat: Some(51.92),
        intelligence_port_id: None,
        congestion_level: None,
        methanol_price_avg: None,
        biofuel_price_avg: None,
        captured_at: None,
    };
    let value = port_response(&row);
    let keys: Vec<&str> = value
        .as_object()
        .expect("port response is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "name",
            "country",
            "location",
            "timezone",
            "is_active",
            "lat",
            "lng",
            "intelligence"
        ]
    );
    // The geography column is never serialized, and a missing intelligence row
    // stays an explicit null instead of an object of nulls.
    assert_eq!(value["location"], Json::Null);
    assert_eq!(value["intelligence"], Json::Null);
    assert_eq!(value["lng"], json!(4.4777));
    assert_eq!(value["lat"], json!(51.92));
}

#[test]
fn port_response_serializes_intelligence_when_present() {
    let row = PortRow {
        id: "SGSIN".to_owned(),
        name: "Singapore".to_owned(),
        country: "SG".to_owned(),
        timezone: Some("Asia/Singapore".to_owned()),
        is_active: true,
        lng: Some(103.85),
        lat: Some(1.29),
        intelligence_port_id: Some("SGSIN".to_owned()),
        congestion_level: Some("Low".to_owned()),
        methanol_price_avg: Some(650.0),
        biofuel_price_avg: None,
        captured_at: Some(utc("2026-10-15T12:00:00Z")),
    };
    let intelligence = &port_response(&row)["intelligence"];
    assert_eq!(intelligence["congestion_level"], json!("Low"));
    assert_eq!(intelligence["methanol_price_avg"], json!(650.0));
    assert_eq!(intelligence["biofuel_price_avg"], Json::Null);
    assert_eq!(intelligence["captured_at"], json!("2026-10-15T12:00:00Z"));
}

#[test]
fn news_response_keeps_python_field_order_and_nullable_dates() {
    let row = NewsRow {
        id: "11111111-2222-4333-8444-555555555555".to_owned(),
        title: "Fixture headline".to_owned(),
        summary: None,
        source: "Fixture source".to_owned(),
        source_url: None,
        url: "https://example.invalid/fixture".to_owned(),
        category: "markets".to_owned(),
        relevance: 3,
        published_at: Some(utc("2026-10-15T12:00:00Z")),
        fetched_at: None,
    };
    let value = news_response(&row);
    let keys: Vec<&str> = value
        .as_object()
        .expect("news response is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "title",
            "summary",
            "source",
            "source_url",
            "url",
            "category",
            "relevance",
            "published_at",
            "fetched_at"
        ]
    );
    assert_eq!(value["summary"], Json::Null);
    assert_eq!(value["source_url"], Json::Null);
    assert_eq!(value["fetched_at"], Json::Null);
    assert_eq!(value["published_at"], json!("2026-10-15T12:00:00+00:00"));
    assert_eq!(value["relevance"], json!(3));
}

#[test]
fn news_query_defaults_match_python() {
    let query = news_query("").expect("defaults are valid");
    assert_eq!(query.limit, 20);
    assert_eq!(query.min_relevance, 1);
    assert_eq!(query.category, None);

    // FastAPI passes an empty category through as an empty string and the
    // route's falsy check turns it into "no filter"; the SQL keeps that branch.
    let query =
        news_query("category=&limit=100&min_relevance=5").expect("boundary values are valid");
    assert_eq!(query.category, Some(String::new()));
    assert_eq!(query.limit, 100);
    assert_eq!(query.min_relevance, 5);
}

#[test]
fn news_query_rejects_out_of_range_limit_like_python() {
    let error = news_query("limit=999").expect_err("limit above 100");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error.detail,
        json!([{
            "type": "less_than_equal",
            "loc": ["query", "limit"],
            "msg": "Input should be less than or equal to 100",
            "input": "999",
            "ctx": {"le": 100}
        }])
    );
}

#[test]
fn news_query_rejects_out_of_range_relevance_like_python() {
    let error = news_query("min_relevance=0").expect_err("below 1");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error.detail,
        json!([{
            "type": "greater_than_equal",
            "loc": ["query", "min_relevance"],
            "msg": "Input should be greater than or equal to 1",
            "input": "0",
            "ctx": {"ge": 1}
        }])
    );
}
