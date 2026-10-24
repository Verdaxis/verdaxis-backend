//! Phase 2 Task 8 — N01–N04 evidence.
//!
//! Offline sections run everywhere: they pin the query and path validation
//! envelopes, the response projection, and the PostgreSQL statements against
//! `tests/rust_contract/phase2/notifications.json`, which records the
//! Python-compiled statements and the source digests they came from.
//!
//! The database-backed section needs a disposable PostgreSQL with the migrated
//! schema and the app role, exactly like the Phase 2 runner's own cells. Supply
//! `NOTIFICATIONS_TEST_APP_URL` and `NOTIFICATIONS_TEST_ADMIN_URL`; without them
//! the section reports nothing and `REQUIRE_NOTIFICATIONS_TEST` turns that
//! absence into a failure.
//!
//! Task 8 owns `notification_scope_and_email_state`. It seeds two users in one
//! organization and one user in another, then checks that the list and the
//! count are recipient-bound, that only top-level `_email_` keys are redacted,
//! that the two writes include the email-only rows, and that no notification
//! write produces an outbox event.

use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

use verdaxis_read::auth::limits::SharedLimits;
use verdaxis_read::crud::notifications::{self, NotificationAuthority};
use verdaxis_read::{AppState, Config, MarketPolicy};

/// The pinned fixture: Python-compiled statements, response shape and digests.
const FIXTURE: &str = include_str!("../../../tests/rust_contract/phase2/notifications.json");

const ORG_PRIMARY: &str = "10000000-0000-4000-8000-000000000001";
const ORG_FOREIGN: &str = "10000000-0000-4000-8000-000000000002";
const USER_A: &str = "20000000-0000-4000-8000-00000000000a";
const USER_B: &str = "20000000-0000-4000-8000-00000000000b";
const USER_C: &str = "20000000-0000-4000-8000-00000000000c";

/// Token settings the test signs with. They never leave this process.
const TEST_SECRET: &str = "notifications-test-secret-at-least-32-bytes-long";
const TEST_ISSUER: &str = "verdaxis-test-api";
const TEST_AUDIENCE: &str = "verdaxis-test-web";

#[test]
fn notification_query_matches_the_fastapi_declarations() {
    let query = notifications::notification_query("").expect("defaults");
    assert_eq!((query.skip, query.limit), (0, 50));
    let query = notifications::notification_query("skip=7&limit=3").expect("explicit");
    assert_eq!((query.skip, query.limit), (7, 3));
    // FastAPI keeps the last value of a repeated scalar query key.
    let query = notifications::notification_query("skip=1&skip=4").expect("repeated");
    assert_eq!(query.skip, 4);
    // Unknown keys are ignored, exactly like undeclared FastAPI parameters.
    let query = notifications::notification_query("cursor=abc&skip=2").expect("unknown key");
    assert_eq!(query.skip, 2);

    for (raw, kind, name) in [
        ("skip=-1", "greater_than_equal", "skip"),
        ("skip=abc", "int_parsing", "skip"),
        ("limit=0", "greater_than_equal", "limit"),
        ("limit=101", "less_than_equal", "limit"),
        ("limit=", "int_parsing", "limit"),
    ] {
        let error = notifications::notification_query(raw).expect_err(raw);
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        let entries = error.detail.as_array().expect("detail array");
        assert_eq!(entries.len(), 1, "{raw}");
        assert_eq!(entries[0]["type"], kind, "{raw}");
        assert_eq!(entries[0]["loc"], json!(["query", name]), "{raw}");
    }

    // Both parameters are validated and reported together, in declaration order.
    let error = notifications::notification_query("skip=-1&limit=0").expect_err("both");
    let entries = error.detail.as_array().expect("detail array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["loc"], json!(["query", "skip"]));
    assert_eq!(entries[1]["loc"], json!(["query", "limit"]));

    // `skip` has no declared maximum, but the widest wire offset is still i64.
    assert_eq!(
        notifications::notification_query("skip=9223372036854775807")
            .expect("i64 max")
            .skip,
        i64::MAX
    );
    assert!(
        notifications::notification_query("skip=9223372036854775808").is_err(),
        "an offset above i64 is refused before the driver"
    );
}

#[test]
fn notification_offset_overflow_matches_approved_vectors() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    for vector in fixture["approved_differences"][0]["vectors"]
        .as_array()
        .unwrap()
    {
        let result = notifications::notification_query(vector["query"].as_str().unwrap());
        if vector.get("rust_status").is_some() {
            let error = result.expect_err("overflow rejected before database");
            assert_eq!(
                error.status.as_u16(),
                vector["rust_status"].as_u64().unwrap() as u16
            );
            assert_eq!(error.detail[0]["type"], vector["rust_type"]);
        } else {
            assert!(result.is_ok());
        }
    }
}

#[test]
fn notification_path_parameter_uses_the_path_location() {
    let parsed = notifications::parse_notification_id(USER_A).expect("valid uuid");
    assert_eq!(parsed, Uuid::parse_str(USER_A).unwrap());

    let error = notifications::parse_notification_id("not-a-uuid").expect_err("invalid");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error.detail,
        json!([{
            "type": "uuid_parsing",
            "loc": ["path", "notification_id"],
            "msg": "Input should be a valid UUID, invalid character: found `n` at 1",
            "input": "not-a-uuid",
            "ctx": {"error": "invalid character: found `n` at 1"},
        }])
    );
}

#[test]
fn notification_projection_matches_the_response_model() {
    // `data` is the stored `json` column text; key order and duplicates are
    // PostgreSQL's, so the response keeps the order of the stored document.
    let mut row = notifications::NotificationRow {
        id: Uuid::parse_str(USER_A).unwrap(),
        notification_type: "SYSTEM".into(),
        title: "Title".into(),
        message: "Message".into(),
        data: Some(
            r#"{"visible": 1, "_email_only": true, "_email_trace": "smtp", "nested": {"_email_key": 2}}"#
                .into(),
        ),
        is_read: false,
        created_at: DateTime::from_timestamp(1_792_281_600, 0).unwrap(),
    };
    let projected = notifications::notification_response(&row).unwrap();
    assert_eq!(
        projected,
        json!({
            "id": USER_A,
            "type": "SYSTEM",
            "title": "Title",
            "message": "Message",
            "data": {"visible": 1, "nested": {"_email_key": 2}},
            "is_read": false,
            "created_at": "2026-10-18T00:00:00Z",
        })
    );
    // Field order and the surviving top-level key order are wire evidence.
    let text = serde_json::to_string(&projected).unwrap();
    assert!(
        text.starts_with(
            "{\"id\":\"20000000-0000-4000-8000-00000000000a\",\"type\":\"SYSTEM\",\"title\":\"Title\",\"message\":\"Message\",\"data\":{\"visible\":1,\"nested\":{\"_email_key\":2}},\"is_read\":false,\"created_at\":\"2026-10-18T00:00:00Z\"}"
        ),
        "{text}"
    );

    // A Pydantic datetime keeps six fractional digits when they are non-zero.
    row.created_at = DateTime::from_timestamp(1_792_281_600, 123_456_000).unwrap();
    assert_eq!(
        notifications::notification_response(&row).unwrap()["created_at"],
        json!("2026-10-18T00:00:00.123456Z")
    );

    // SQL NULL and an empty JSON object stay distinct.
    row.data = None;
    assert_eq!(
        notifications::notification_response(&row).unwrap()["data"],
        Value::Null
    );
    row.data = Some("{}".into());
    assert_eq!(
        notifications::notification_response(&row).unwrap()["data"],
        json!({})
    );
    // Every top-level `_email_` key is removed even when the object is otherwise
    // empty, which is still an empty object and not a null.
    row.data = Some(r#"{"_email_only": false}"#.into());
    assert_eq!(
        notifications::notification_response(&row).unwrap()["data"],
        json!({})
    );

    // Pydantic rejects stored non-object data rather than exposing it as a DTO.
    for data in [r#""scalar""#, "[]", "3", "true"] {
        row.data = Some(data.into());
        assert!(
            notifications::notification_response(&row).is_err(),
            "{data}"
        );
    }
}

#[test]
fn notification_sql_matches_the_pinned_oracle_statements() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture");
    assert_eq!(fixture["task"], 8);
    let statements = fixture["statements"].as_object().expect("statements");
    for (id, rust) in [
        ("N01", notifications::LIST_SQL),
        ("N02", notifications::COUNT_SQL),
        ("N03-lookup", notifications::FIND_SQL),
        ("N03-update", notifications::MARK_SQL),
        ("N04", notifications::MARK_ALL_SQL),
    ] {
        let entry = statements
            .get(id)
            .unwrap_or_else(|| panic!("missing fixture statement {id}"));
        assert_eq!(
            entry["rust"].as_str().expect("rust text"),
            rust,
            "{id}: the module statement changed"
        );
        assert_eq!(
            normalise_statement(entry["python"].as_str().expect("python text")),
            normalise_statement(rust),
            "{id}: the Rust statement no longer matches the Python-compiled statement"
        );
    }
}

/// Compare two statements that differ only in placeholder spelling, table
/// qualification, SQLAlchemy's generated column labels, whitespace and the one
/// decode-only `data::text` cast the Rust statement needs.
fn normalise_statement(statement: &str) -> String {
    let uncommented: String = statement
        .lines()
        .map(|line| line.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    let mut unified = String::new();
    let mut characters = uncommented.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '$' if characters.peek().is_some_and(char::is_ascii_digit) => {
                while characters.peek().is_some_and(char::is_ascii_digit) {
                    characters.next();
                }
                unified.push('?');
            }
            '%' if characters.peek() == Some(&'(') => {
                // Consume `%(<name>)s` up to and including the closing `)s`;
                // the parameter name itself can contain an `s`.
                while let Some(next) = characters.next() {
                    if next == ')' && characters.peek() == Some(&'s') {
                        characters.next();
                        break;
                    }
                }
                unified.push('?');
            }
            other => unified.push(other),
        }
    }
    unified
        .to_lowercase()
        .replace("notifications.", "")
        // The Rust projection reads the `json` column as text because this
        // workspace's sqlx build has no `json` feature. The cast cannot change
        // a stored value, a key order or a null, so the comparison ignores it.
        .replace("::text", "")
        .replace(" as count_1", "")
        // SQLAlchemy renders the assignment `is_read=%(x)s` without spaces
        // while the port writes it readably; spacing around `=` is cosmetic and
        // both sides collapse it.
        .replace(" = ", "=")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn test_config(database_url: String) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url,
        db_session_time_zone: "Asia/Singapore".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 20,
        configured_max_connections: 100,
    }
}

fn policy() -> MarketPolicy {
    MarketPolicy {
        source_sha: verdaxis_read::compiled_sha().into(),
        products: vec![],
        points: vec![],
    }
}

/// A state whose read pool never connects, for checks that must fail before any
/// database work happens.
fn offline_state() -> AppState {
    let config =
        test_config("postgres://offline_app:offline_password@127.0.0.1:5432/offline_test".into());
    let read = PgPoolOptions::new()
        .max_connections(config.read_admission_slots)
        .connect_lazy(&config.database_url)
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&config.database_url)
        .unwrap();
    AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read, health },
        policy(),
    )
    .unwrap()
}

fn authority() -> NotificationAuthority {
    NotificationAuthority::new(
        verdaxis_read::auth::VerificationSettings::new(
            TEST_SECRET.into(),
            None,
            TEST_ISSUER.into(),
            TEST_AUDIENCE.into(),
        ),
        SharedLimits::default(),
    )
}

fn signed_token(subject: &str) -> String {
    let now = Utc::now();
    let claims = json!({
        "sub": subject,
        "type": "access",
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "jti": Uuid::new_v4().to_string(),
        "iat": now.timestamp(),
        "exp": (now + Duration::hours(1)).timestamp(),
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET.as_bytes()),
    )
    .unwrap()
}

fn authenticated(method: &str, path: &str, token: Option<&str>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(path);
    match token {
        Some(token) => builder
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value, HeaderMap) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("json body")
    };
    (status, value, headers)
}

async fn list_ids(app: &Router, token: &str, query: &str) -> Vec<String> {
    let path = if query.is_empty() {
        "/api/notifications".to_owned()
    } else {
        format!("/api/notifications?{query}")
    };
    let (status, body, _) = send(app, authenticated("GET", &path, Some(token))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .expect("array body")
        .iter()
        .map(|row| row["id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test]
async fn authentication_is_refused_before_any_database_work() {
    let app = verdaxis_read::runtime::http::router_with(
        notifications::routes(authority()),
        offline_state(),
    );
    for request in [
        authenticated("GET", "/api/notifications", None),
        authenticated("GET", "/api/notifications/unread-count", None),
        authenticated("PATCH", "/api/notifications/read-all", None),
    ] {
        let (status, body, headers) = send(&app, bearerless(request)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({"detail": "Not authenticated"}));
        assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
    }
    // A malformed bearer is refused before the pool would be acquired, which
    // this lazy pool could never serve.
    let (status, body, headers) = send(
        &app,
        authenticated("GET", "/api/notifications", Some("not-a-token")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, verdaxis_read::auth::AuthError::Credentials.body());
    assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
}

/// Rebuild a request without an `Authorization` header.
fn bearerless(request: Request<Body>) -> Request<Body> {
    let (parts, body) = request.into_parts();
    let mut builder = Request::builder().method(parts.method).uri(parts.uri);
    for (name, value) in parts.headers.iter() {
        if name != header::AUTHORIZATION {
            builder = builder.header(name, value);
        }
    }
    builder.body(body).unwrap()
}

#[tokio::test]
async fn notification_scope_and_email_state() {
    let (Some(app_url), Some(admin_url)) = (
        std::env::var("NOTIFICATIONS_TEST_APP_URL").ok(),
        std::env::var("NOTIFICATIONS_TEST_ADMIN_URL").ok(),
    ) else {
        assert!(
            std::env::var("REQUIRE_NOTIFICATIONS_TEST").is_err(),
            "NOTIFICATIONS_TEST_APP_URL and NOTIFICATIONS_TEST_ADMIN_URL are required"
        );
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url)
        .await
        .unwrap();
    let config = test_config(app_url);
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let state = AppState::new(config, pools, policy()).unwrap();
    let app = Arc::new(verdaxis_read::runtime::http::router_with(
        notifications::routes(authority()),
        state,
    ));

    // One organization with two users, plus a second organization, so a
    // recipient filter that leaked across users or organizations is visible.
    for (id, name, domain) in [
        (ORG_PRIMARY, "Primary", "primary.invalid"),
        (ORG_FOREIGN, "Foreign", "foreign.invalid"),
    ] {
        sqlx::query(
            "INSERT INTO organizations (id,name,domain,type) VALUES ($1::uuid,$2,$3,'FUEL_BUYER')",
        )
        .bind(id)
        .bind(name)
        .bind(domain)
        .execute(&admin)
        .await
        .unwrap();
    }
    for (id, email, organization) in [
        (USER_A, "a@primary.invalid", ORG_PRIMARY),
        (USER_B, "b@primary.invalid", ORG_PRIMARY),
        (USER_C, "c@foreign.invalid", ORG_FOREIGN),
    ] {
        sqlx::query("INSERT INTO users (id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES ($1::uuid,$2,'not-a-login','BUYER','APPROVED',$3::uuid,true,false)")
            .bind(id).bind(email).bind(organization).execute(&admin).await.unwrap();
    }

    // A: a null `data`, an empty object, every `_email_only` coercion the
    // predicate can see, private email keys, nested ordinary JSON, one already
    // read row, and a tied timestamp. B shares A's organization; C does not.
    let seeds: [(&str, &str, Option<&str>, bool, &str); 12] = [
        (
            "30000000-0000-4000-8000-000000000001",
            USER_A,
            None,
            false,
            "2026-01-01T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000002",
            USER_A,
            Some("{}"),
            false,
            "2026-01-02T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000003",
            USER_A,
            Some(r#"{"_email_only": true}"#),
            false,
            "2026-01-03T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000004",
            USER_A,
            Some(
                r#"{"_email_only": false, "visible": 1, "_email_trace": "smtp", "nested": {"_email_key": 2}}"#,
            ),
            false,
            "2026-01-04T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000005",
            USER_A,
            Some(r#"{"_email_only": "true"}"#),
            false,
            "2026-01-05T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000006",
            USER_A,
            Some(r#"{"_email_only": "false"}"#),
            false,
            "2026-01-05T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000007",
            USER_A,
            Some(r#"{"_email_only": 1}"#),
            false,
            "2026-01-06T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000008",
            USER_A,
            Some(r#"{"_email_only": 0}"#),
            false,
            "2026-01-06T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-000000000009",
            USER_A,
            Some(r#"{"_email_only": null}"#),
            false,
            "2026-01-08T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-00000000000a",
            USER_A,
            Some("{}"),
            true,
            "2026-01-08T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-0000000000b1",
            USER_B,
            Some("{}"),
            false,
            "2026-02-01T00:00:00Z",
        ),
        (
            "30000000-0000-4000-8000-0000000000c1",
            USER_C,
            Some("{}"),
            false,
            "2026-03-01T00:00:00Z",
        ),
    ];
    for (id, recipient, data, is_read, created_at) in seeds {
        sqlx::query(
            "INSERT INTO notifications (id, recipient_id, type, title, message, data, is_read, created_at) \
             VALUES ($1::uuid, $2::uuid, 'SYSTEM', 'Title', 'Message', $3::json, $4, $5::timestamptz)",
        )
        .bind(id)
        .bind(recipient)
        .bind(data)
        .bind(is_read)
        .bind(created_at)
        .execute(&admin)
        .await
        .unwrap();
    }

    let initial_rows: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(n)::text FROM notifications n ORDER BY id")
            .fetch_all(&admin)
            .await
            .unwrap();
    let initial_users: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(u)::text FROM users u ORDER BY id")
            .fetch_all(&admin)
            .await
            .unwrap();
    let outer_events: i64 = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
        .fetch_one(&admin)
        .await
        .unwrap();

    let token_a = signed_token(USER_A);
    let token_b = signed_token(USER_B);
    let token_c = signed_token(USER_C);

    // Scope: each recipient sees only their own rows, whichever organization.
    assert_eq!(
        list_ids(&app, &token_b, "").await,
        ["30000000-0000-4000-8000-0000000000b1"]
    );
    assert_eq!(
        list_ids(&app, &token_c, "").await,
        ["30000000-0000-4000-8000-0000000000c1"]
    );

    // The list excludes every truthy `_email_only` coercion and keeps the rest,
    // newest first, with the two tied rows as one unordered pair.
    let ids = list_ids(&app, &token_a, "").await;
    assert_eq!(ids.len(), 7, "{ids:?}");
    let mut tied = ids[0..2].to_vec();
    tied.sort();
    assert_eq!(
        tied,
        [
            "30000000-0000-4000-8000-000000000009",
            "30000000-0000-4000-8000-00000000000a",
        ]
    );
    assert_eq!(
        &ids[2..],
        [
            "30000000-0000-4000-8000-000000000008",
            "30000000-0000-4000-8000-000000000006",
            "30000000-0000-4000-8000-000000000004",
            "30000000-0000-4000-8000-000000000002",
            "30000000-0000-4000-8000-000000000001",
        ]
    );

    // Pagination stays inside the tie group and then walks the distinct keys.
    let ids = list_ids(&app, &token_a, "limit=2&skip=2").await;
    assert_eq!(
        ids,
        [
            "30000000-0000-4000-8000-000000000008",
            "30000000-0000-4000-8000-000000000006",
        ]
    );
    let ids = list_ids(&app, &token_a, "limit=100&skip=6").await;
    assert_eq!(ids, ["30000000-0000-4000-8000-000000000001"]);

    // Typed query validation runs after authentication and before the handler.
    let (status, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications?limit=0", Some(&token_a)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["detail"][0]["loc"], json!(["query", "limit"]));
    let (status, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications?skip=-1", Some(&token_a)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["detail"][0]["loc"], json!(["query", "skip"]));

    // Redaction is top-level only, and null stays distinct from an empty object.
    let (_, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications", Some(&token_a)),
    )
    .await;
    let rows = body.as_array().unwrap();
    let row = |id: &str| {
        rows.iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("missing row {id}"))
    };
    assert_eq!(
        row("30000000-0000-4000-8000-000000000004")["data"],
        json!({"visible": 1, "nested": {"_email_key": 2}})
    );
    assert_eq!(
        row("30000000-0000-4000-8000-000000000001")["data"],
        Value::Null
    );
    assert_eq!(
        row("30000000-0000-4000-8000-000000000002")["data"],
        json!({})
    );
    assert_eq!(
        row("30000000-0000-4000-8000-000000000004")["type"],
        "SYSTEM"
    );
    assert_eq!(
        row("30000000-0000-4000-8000-000000000004")["is_read"],
        false
    );

    // N02 counts the recipient's unread, non-email-only rows.
    let (status, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications/unread-count", Some(&token_a)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"count": 6}));
    let (status, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications/unread-count", Some(&token_b)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"count": 1}));

    // N03: the path parameter is validated after authentication.
    let (status, body, _) = send(
        &app,
        authenticated("PATCH", "/api/notifications/not-a-uuid/read", None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({"detail": "Not authenticated"}));
    let (status, body, _) = send(
        &app,
        authenticated(
            "PATCH",
            "/api/notifications/not-a-uuid/read",
            Some("garbage"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, verdaxis_read::auth::AuthError::Credentials.body());
    let (status, body, _) = send(
        &app,
        authenticated(
            "PATCH",
            "/api/notifications/not-a-uuid/read",
            Some(&token_a),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["detail"][0]["type"], "uuid_parsing");
    assert_eq!(body["detail"][0]["loc"], json!(["path", "notification_id"]));

    // A foreign row and an absent row are the same 404, and neither is written.
    for id in [
        "30000000-0000-4000-8000-0000000000b1",
        "30000000-0000-4000-8000-0000000000c1",
        "30000000-0000-4000-8000-0000000000ff",
    ] {
        let (status, body, _) = send(
            &app,
            authenticated(
                "PATCH",
                &format!("/api/notifications/{id}/read"),
                Some(&token_a),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
        assert_eq!(body, json!({"detail": "Notification not found"}), "{id}");
    }
    let foreign_read: bool =
        sqlx::query_scalar("SELECT is_read FROM notifications WHERE id=$1::uuid")
            .bind("30000000-0000-4000-8000-0000000000b1")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(!foreign_read, "a foreign row must not be marked read");

    // N03 includes an email-only row that the list hides.
    let (status, body, _) = send(
        &app,
        authenticated(
            "PATCH",
            "/api/notifications/30000000-0000-4000-8000-000000000003/read",
            Some(&token_a),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "success"}));
    let marked: bool = sqlx::query_scalar("SELECT is_read FROM notifications WHERE id=$1::uuid")
        .bind("30000000-0000-4000-8000-000000000003")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert!(marked);
    let (_, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications/unread-count", Some(&token_a)),
    )
    .await;
    assert_eq!(
        body,
        json!({"count": 6}),
        "an email-only row is not counted"
    );

    // Repeating the write is accepted and changes nothing else.
    let (status, body, _) = send(
        &app,
        authenticated(
            "PATCH",
            "/api/notifications/30000000-0000-4000-8000-000000000001/read",
            Some(&token_a),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "success"}));
    let (status, _, _) = send(
        &app,
        authenticated(
            "PATCH",
            "/api/notifications/30000000-0000-4000-8000-000000000001/read",
            Some(&token_a),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications/unread-count", Some(&token_a)),
    )
    .await;
    assert_eq!(body, json!({"count": 5}), "count follows the update");

    // N04 includes the email-only rows and leaves other recipients alone.
    let (status, body, _) = send(
        &app,
        authenticated("PATCH", "/api/notifications/read-all", Some(&token_a)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "success"}));
    let (_, body, _) = send(
        &app,
        authenticated("GET", "/api/notifications/unread-count", Some(&token_a)),
    )
    .await;
    assert_eq!(body, json!({"count": 0}));
    let unread_for_a: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE recipient_id=$1::uuid AND is_read=false",
    )
    .bind(USER_A)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(unread_for_a, 0, "read-all includes email-only rows");
    let unread_others: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE recipient_id<>$1::uuid AND is_read=false",
    )
    .bind(USER_A)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(unread_others, 2, "another recipient's rows are untouched");

    let (status, body, _) = send(
        &app,
        authenticated("PATCH", "/api/notifications/read-all", Some(&token_a)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "success"}));
    let final_rows: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(n)::text FROM notifications n ORDER BY id")
            .fetch_all(&admin)
            .await
            .unwrap();
    for (before, after) in initial_rows.iter().zip(&final_rows) {
        let mut expected: Value = serde_json::from_str(before).unwrap();
        if expected["recipient_id"] == USER_A {
            expected["is_read"] = json!(true);
        }
        assert_eq!(expected, serde_json::from_str::<Value>(after).unwrap());
    }
    assert_eq!(initial_rows.len(), final_rows.len());
    let final_users: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(u)::text FROM users u ORDER BY id")
            .fetch_all(&admin)
            .await
            .unwrap();
    assert_eq!(
        initial_users, final_users,
        "authentication must not change users"
    );
    let watchlist_events: i64 = sqlx::query_scalar("SELECT count(*) FROM watchlist_events")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(watchlist_events, 0);

    // No notification route produces an event.
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(events, outer_events, "no outbox event was generated");

    admin.close().await;
}
