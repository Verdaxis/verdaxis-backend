//! Schema-1 abilities through the real runtime role and migrated disposable database.
#![cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{AppState, Config, compiled_sha};

const SECRET: &str = "abilities-synthetic-secret-at-least-32-characters";
const ACTIONS: [&str; 23] = [
    "place_bid_order",
    "place_ask_order",
    "amend_order",
    "cancel_order",
    "hit_ask_order",
    "hit_bid_order",
    "confirm_trade",
    "deliver_trade",
    "pay_trade",
    "manage_inventory",
    "publish_inventory",
    "create_rfq",
    "quote_rfq",
    "view_admin_analytics",
    "view_audit_log",
    "view_user_activity",
    "view_feedback",
    "manage_subscriptions",
    "manage_accounts",
    "review_kyc",
    "manage_commissions",
    "market_support_access",
    "market_support_assisted_orders",
];

fn token(user: Uuid) -> String {
    let now = Utc::now().timestamp();
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({"sub":user,"type":"access","iat":now-1,"exp":now+600,
                "iss":"abilities-test","aud":"abilities-test"}),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

async fn application(url: &str, gate: bool, support: bool) -> (Router, sqlx::PgPool, sqlx::PgPool) {
    let config: Config = serde_json::from_value(json!({
        "environment":"test", "release_sha":compiled_sha(),
        "migration_revision":"ob_20261031_offboarding", "database_url":url,
        "kyc_execution_gate_enabled":gate, "db_session_time_zone":"UTC", "bind":"127.0.0.1:0",
        "pool_max":5, "read_admission_slots":4, "python_budget":52, "configured_max_connections":100,
        "phase2":{"jwt_secret":SECRET,"jwt_issuer":"abilities-test","jwt_audience":"abilities-test",
        "frontend_url":"https://app.example.test","email_from":"team@example.test",
        "resend_api_key":"synthetic","auth_domain_datasets":"null",
        "market_write":{"support_enabled":support,"support_consent_version":"test-v1","support_consent_reference":"https://example.test/consent"}}
    })).unwrap();
    let read = PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .unwrap();
    let state = AppState::new(
        config,
        verdaxis_read::runtime::db::Pools {
            read: read.clone(),
            health: health.clone(),
        },
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap(),
    )
    .unwrap();
    (verdaxis_read::router(state), read, health)
}

async fn get(
    app: &Router,
    path: &str,
    bearer: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder().uri(path);
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

fn reason<'a>(payload: &'a Value, action: &str) -> Option<&'a str> {
    payload["abilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == action)
        .unwrap()["reason_code"]
        .as_str()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL; run_abilities.sh"]
async fn authenticated_shape_policy_switch_and_password_boundary() {
    let database = market_database::MarketDatabase::new("RUST_ABILITIES").await;
    let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,name,type,verification_status,provenance,created_at) VALUES($1,'Abilities buyer','SHIP_OPERATOR','APPROVED','REAL',now())")
        .bind(org).execute(&mut admin).await.unwrap();
    sqlx::query("INSERT INTO users(id,email,password_hash,role,status,email_verified,organization_id,kyc_status) VALUES($1,'abilities@example.test','synthetic','BUYER','APPROVED',true,$2,'PENDING')")
        .bind(user).bind(org).execute(&mut admin).await.unwrap();
    let bearer = token(user);
    for gate in [false, true] {
        let (app, read, health) = application(&database.runtime_url, gate, false).await;
        assert_eq!(
            get(&app, "/api/auth/me/abilities", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, headers, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(body["schema_version"], 1);
        let rows = body["abilities"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row["action"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ACTIONS
        );
        for row in rows {
            assert_eq!(row.as_object().unwrap().len(), 4);
            let allowed = row["allowed"].as_bool().unwrap();
            assert_eq!(row["reason_code"].is_null(), allowed);
            assert_eq!(row["reason"].is_null(), allowed);
            if !allowed {
                assert!(!row["reason"].as_str().unwrap().is_empty());
            }
        }
        assert_eq!(
            reason(&body, "place_bid_order"),
            gate.then_some("KYC_REQUIRED")
        );
        assert_eq!(reason(&body, "cancel_order"), None);
        assert_eq!(reason(&body, "deliver_trade"), None);
        for action in ["create_rfq", "quote_rfq"] {
            assert_eq!(reason(&body, action), Some("ACTION_RETIRED"));
            let row = rows.iter().find(|row| row["action"] == action).unwrap();
            assert_eq!(row["reason"], "This feature has been retired.");
        }
        // Forced-password profile reads are allowed, but ability hints are not.
        sqlx::query("UPDATE users SET must_change_password=true WHERE id=$1")
            .bind(user)
            .execute(&mut admin)
            .await
            .unwrap();
        assert_eq!(
            get(&app, "/api/auth/me", Some(&bearer)).await.0,
            StatusCode::OK
        );
        let (status, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        sqlx::query("UPDATE users SET must_change_password=false WHERE id=$1")
            .bind(user)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET role='ADMIN' WHERE id=$1")
            .bind(user)
            .execute(&mut admin)
            .await
            .unwrap();
        let (status, _, staff) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
        assert_eq!(status, StatusCode::OK, "{staff}");
        assert_eq!(reason(&staff, "manage_accounts"), None);
        assert_eq!(
            reason(&staff, "market_support_access"),
            Some("MARKET_SUPPORT_DISABLED")
        );
        let (support_app, support_read, support_health) =
            application(&database.runtime_url, gate, true).await;
        for capability in ["MARKET_SUPPORT_LISTINGS", "MARKET_SUPPORT_AUTHORIZATIONS"] {
            sqlx::query("INSERT INTO staff_capability_assignments(id,user_id,capability,reason,granted_by_user_id,granted_at) VALUES($1,$2,$3,'Abilities fixture',$2,now()) ON CONFLICT(user_id,capability) DO UPDATE SET revoked_at=NULL,expires_at=NULL")
                .bind(Uuid::new_v4()).bind(user).bind(capability).execute(&mut admin).await.unwrap();
        }
        let (status, _, staff) = get(&support_app, "/api/auth/me/abilities", Some(&bearer)).await;
        assert_eq!(status, StatusCode::OK, "{staff}");
        assert_eq!(reason(&staff, "market_support_access"), None);
        assert_eq!(reason(&staff, "market_support_assisted_orders"), None);
        sqlx::query("UPDATE staff_capability_assignments SET expires_at=now()-interval '1 second' WHERE user_id=$1 AND capability='MARKET_SUPPORT_AUTHORIZATIONS'").bind(user).execute(&mut admin).await.unwrap();
        let (_, _, staff) = get(&support_app, "/api/auth/me/abilities", Some(&bearer)).await;
        assert_eq!(reason(&staff, "market_support_access"), None);
        assert_eq!(
            reason(&staff, "market_support_assisted_orders"),
            Some("MARKET_SUPPORT_CAPABILITY_REQUIRED")
        );
        sqlx::query("UPDATE staff_capability_assignments SET revoked_at=now() WHERE user_id=$1")
            .bind(user)
            .execute(&mut admin)
            .await
            .unwrap();
        let (_, _, staff) = get(&support_app, "/api/auth/me/abilities", Some(&bearer)).await;
        assert_eq!(
            reason(&staff, "market_support_access"),
            Some("MARKET_SUPPORT_CAPABILITY_REQUIRED")
        );
        support_read.close().await;
        support_health.close().await;
        sqlx::query("UPDATE users SET role='BUYER' WHERE id=$1")
            .bind(user)
            .execute(&mut admin)
            .await
            .unwrap();
        read.close().await;
        health.close().await;
    }
}
