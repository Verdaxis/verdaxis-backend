//! Preferences routes against a migrated PostgreSQL schema.
//!
//! The `dashboard` namespace exists only in Rust, so no Python twin can compare
//! it. These cases check the stored value, the read-back and the refusals
//! through the real HTTP router and the shared authenticated transaction.
//!
//! The section needs a disposable PostgreSQL with the migrated schema and the
//! app role. Supply `PREFERENCES_TEST_APP_URL` and `PREFERENCES_TEST_ADMIN_URL`;
//! without them it reports nothing, and `REQUIRE_PREFERENCES_TEST` turns that
//! absence into a failure. `tests/run_preferences.py` sets everything up.

use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

use verdaxis_read::auth::VerificationSettings;
use verdaxis_read::auth::limits::SharedLimits;
use verdaxis_read::crud::preferences;
use verdaxis_read::{AppState, Config, MarketPolicy};

const ORG: &str = "10000000-0000-4000-8000-0000000000d1";
const USER_A: &str = "20000000-0000-4000-8000-0000000000d1";
const USER_B: &str = "20000000-0000-4000-8000-0000000000d2";

const TEST_SECRET: &str = "preferences-test-secret-at-least-32-bytes-long";
const TEST_ISSUER: &str = "verdaxis-test-api";
const TEST_AUDIENCE: &str = "verdaxis-test-web";

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
        phase2: Default::default(),
    }
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

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let payload = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(value).unwrap())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(payload).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, value)
}

fn layout() -> Value {
    json!({"version": 1, "widgets": [
        {"id": "quick_actions", "type": "quick_actions", "x": 0, "y": 0, "w": 12, "h": 2},
        {"id": "order_book", "type": "order_book", "x": 0, "y": 2, "w": 6, "h": 8},
        {"id": "trade_tape", "type": "trade_tape", "x": 6, "y": 2, "w": 6, "h": 8},
    ]})
}

#[tokio::test]
async fn dashboard_layout_is_stored_read_back_and_isolated_per_user() {
    let (Some(app_url), Some(admin_url)) = (
        std::env::var("PREFERENCES_TEST_APP_URL").ok(),
        std::env::var("PREFERENCES_TEST_ADMIN_URL").ok(),
    ) else {
        assert!(
            std::env::var("REQUIRE_PREFERENCES_TEST").is_err(),
            "PREFERENCES_TEST_APP_URL and PREFERENCES_TEST_ADMIN_URL are required"
        );
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    let config = test_config(app_url);
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let state = AppState::new(
        config,
        pools,
        MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let app = preferences::router(
        state,
        Arc::new(VerificationSettings::new(
            TEST_SECRET.into(),
            None,
            TEST_ISSUER.into(),
            TEST_AUDIENCE.into(),
        )),
        SharedLimits::default(),
    );

    sqlx::query(
        "INSERT INTO organizations (id,name,domain,type) VALUES ($1::uuid,'Layouts','layouts.invalid','FUEL_BUYER')",
    )
    .bind(ORG)
    .execute(&admin)
    .await
    .unwrap();
    for (id, email) in [(USER_A, "a@layouts.invalid"), (USER_B, "b@layouts.invalid")] {
        sqlx::query("INSERT INTO users (id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES ($1::uuid,$2,'not-a-login','BUYER','APPROVED',$3::uuid,true,false)")
            .bind(id).bind(email).bind(ORG).execute(&admin).await.unwrap();
    }
    let token_a = signed_token(USER_A);
    let token_b = signed_token(USER_B);

    // Nothing stored yet: the read has no dashboard key.
    let (status, all) = send(&app, "GET", "/api/users/me/preferences", &token_a, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(all.get("dashboard").is_none(), "{all}");

    // A valid layout is accepted and answered with the stored value.
    let (status, saved) = send(
        &app,
        "PUT",
        "/api/users/me/preferences/dashboard",
        &token_a,
        Some(&layout()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["value"], layout());
    assert!(saved["updated_at"].is_string());

    // The read returns exactly the stored value.
    let (status, all) = send(&app, "GET", "/api/users/me/preferences", &token_a, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all["dashboard"], layout());

    // A second write replaces the first; there is still one row per user.
    let replacement = json!({"version": 1, "widgets": []});
    let (status, _) = send(
        &app,
        "PUT",
        "/api/users/me/preferences/dashboard",
        &token_a,
        Some(&replacement),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, all) = send(&app, "GET", "/api/users/me/preferences", &token_a, None).await;
    assert_eq!(all["dashboard"], replacement);
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_preferences WHERE user_id=$1::uuid AND namespace='dashboard'",
    )
    .bind(USER_A)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(rows, 1);

    // Another user does not see it.
    let (_, other) = send(&app, "GET", "/api/users/me/preferences", &token_b, None).await;
    assert!(other.get("dashboard").is_none(), "{other}");

    // A refused layout reports a diagnostic and leaves the stored value alone.
    let mut invalid = layout();
    invalid["widgets"][0]["w"] = json!(13);
    let (status, error) = send(
        &app,
        "PUT",
        "/api/users/me/preferences/dashboard",
        &token_a,
        Some(&invalid),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error["detail"][0]["type"], "less_than_equal");
    assert_eq!(error["detail"][0]["loc"], json!(["widgets", 0, "w"]));
    let (_, all) = send(&app, "GET", "/api/users/me/preferences", &token_a, None).await;
    assert_eq!(all["dashboard"], replacement);

    // An unknown namespace is still a 404 and stores nothing.
    let (status, error) = send(
        &app,
        "PUT",
        "/api/users/me/preferences/home",
        &token_a,
        Some(&layout()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error, json!({"detail": "Unknown preference namespace"}));
    let namespaces: Vec<String> = sqlx::query_scalar(
        "SELECT namespace FROM user_preferences WHERE user_id=$1::uuid ORDER BY namespace",
    )
    .bind(USER_A)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(namespaces, ["dashboard"]);
}
