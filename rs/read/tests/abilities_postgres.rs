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

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL; run_abilities.sh"]
async fn migrated_party_state_matrix() {
    let db = market_database::MarketDatabase::new("RUST_ABILITIES").await;
    let mut admin = PgConnection::connect(&db.admin_url).await.unwrap();
    let org = Uuid::new_v4();
    let other = Uuid::new_v4();
    let user = Uuid::new_v4();
    for id in [org, other] {
        sqlx::query("INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES($1,'Abilities matrix','SHIPOWNER','APPROVED','REAL')")
            .bind(id).execute(&mut admin).await.unwrap();
    }
    sqlx::query("INSERT INTO users(id,email,password_hash,role,status,email_verified,organization_id,kyc_status) VALUES($1,'matrix@example.test','unused','BUYER','APPROVED',true,$2,'PENDING')")
        .bind(user).bind(org).execute(&mut admin).await.unwrap();
    let (app, read, health) = application(&db.runtime_url, true, true).await;
    let bearer = token(user);
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(reason(&body, "place_bid_order"), Some("KYC_REQUIRED"));
    sqlx::query("INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES($1,$2,$3,'PENDING',now(),now()+interval '1 hour','abilities-test')")
        .bind(Uuid::new_v4()).bind(user).bind(org).execute(&mut admin).await.unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(reason(&body, "place_bid_order"), None);
    sqlx::query("UPDATE users SET kyc_status='REJECTED' WHERE id=$1")
        .bind(user)
        .execute(&mut admin)
        .await
        .unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(reason(&body, "place_bid_order"), Some("KYC_REJECTED"));
    sqlx::query(
        "UPDATE kyc_execution_grace SET expires_at=now()-interval '1 second' WHERE user_id=$1",
    )
    .bind(user)
    .execute(&mut admin)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET kyc_status='APPROVED',kyc_organization_id=$2 WHERE id=$1")
        .bind(user)
        .bind(other)
        .execute(&mut admin)
        .await
        .unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(reason(&body, "place_bid_order"), Some("KYC_REQUIRED"));
    assert_eq!(
        reason(&body, "manage_inventory"),
        Some("KYC_ORGANIZATION_MISMATCH")
    );
    sqlx::query("UPDATE users SET kyc_organization_id=$2 WHERE id=$1")
        .bind(user)
        .bind(org)
        .execute(&mut admin)
        .await
        .unwrap();
    sqlx::query("UPDATE organizations SET verification_status='PENDING' WHERE id=$1")
        .bind(org)
        .execute(&mut admin)
        .await
        .unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(
        reason(&body, "place_bid_order"),
        Some("ORGANIZATION_NOT_APPROVED")
    );
    // The migrated provenance trigger accepts DEMO only for seed-owned IDs.
    let demo = Uuid::parse_str("7cc77115-0a9f-4ec4-8c74-05aa10050111").unwrap();
    sqlx::query("INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES($1,'Abilities demo','SHIPOWNER','APPROVED','DEMO')").bind(demo).execute(&mut admin).await.unwrap();
    sqlx::query("UPDATE users SET organization_id=$2,kyc_organization_id=$2 WHERE id=$1")
        .bind(user)
        .bind(demo)
        .execute(&mut admin)
        .await
        .unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(
        reason(&body, "hit_ask_order"),
        Some("ORGANIZATION_PROVENANCE_UNVERIFIED")
    );
    sqlx::query(
        "UPDATE users SET role='SUPPLIER',organization_id=$2,kyc_organization_id=$2 WHERE id=$1",
    )
    .bind(user)
    .bind(other)
    .execute(&mut admin)
    .await
    .unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(reason(&body, "place_ask_order"), None);
    assert_eq!(reason(&body, "publish_inventory"), None);
    sqlx::query(
        "UPDATE users SET role='ADMIN',kyc_status='PENDING',kyc_organization_id=NULL WHERE id=$1",
    )
    .bind(user)
    .execute(&mut admin)
    .await
    .unwrap();
    let (_, _, body) = get(&app, "/api/auth/me/abilities", Some(&bearer)).await;
    assert_eq!(
        reason(&body, "market_support_access"),
        Some("MARKET_SUPPORT_CAPABILITY_REQUIRED")
    );
    for action in [
        "place_bid_order",
        "place_ask_order",
        "hit_ask_order",
        "hit_bid_order",
        "confirm_trade",
        "publish_inventory",
    ] {
        assert_eq!(
            reason(&body, action),
            Some("ADMIN_CANNOT_EXECUTE"),
            "{action}"
        );
    }
    read.close().await;
    health.close().await;
}

async fn market_call(
    app: &Router,
    method: &str,
    path: &str,
    user: Uuid,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {}", token(user)))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn assert_kyc_route_parity(
    app: &Router,
    method: &str,
    path: &str,
    user: Uuid,
    action: &str,
    body: Value,
) {
    let (status, _, hints) = get(app, "/api/auth/me/abilities", Some(&token(user))).await;
    assert_eq!(status, StatusCode::OK, "{hints}");
    assert_eq!(reason(&hints, action), Some("KYC_REQUIRED"));
    let (status, refusal) = market_call(app, method, path, user, body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {refusal}");
    assert_eq!(
        refusal["detail"]["code"],
        reason(&hints, action).unwrap(),
        "{action}: {refusal}"
    );
}

// Real market-write fixtures and the served production composition are used,
// not a policy mock. Each family stops at admission before changing market rows.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL; run_abilities.sh"]
async fn place_and_publish_hints_match_real_execution_routes() {
    let db = market_database::MarketDatabase::new("RUST_ABILITIES").await;
    let mut admin = PgConnection::connect(&db.admin_url).await.unwrap();
    for sql in [
        include_str!("fixtures/market_write_matching.sql"),
        include_str!("fixtures/market_write_publish.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    sqlx::query("SELECT publish_fixture_seed(0,200)")
        .execute(&mut admin)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL")
        .execute(&mut admin)
        .await
        .unwrap();
    let point: Uuid = sqlx::query_scalar("SELECT id FROM delivery_points WHERE name='Rotterdam'")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    let (app, read, health) = application(&db.runtime_url, true, false).await;
    let buyer = Uuid::from_u128(0x1001);
    let seller = Uuid::from_u128(0x2001);
    assert_kyc_route_parity(&app,"POST","/api/orderbook",buyer,"place_bid_order",
        json!({"side":"BID","product_id":"c4cecebc-3d2d-5840-8021-57a9a11bc673","delivery_point_id":point,"quantity_mt":"200","price_per_mt_usd":"110"})).await;
    assert_kyc_route_parity(
        &app,
        "POST",
        "/api/inventory/00000000-0000-0000-0000-000000004000/publish",
        seller,
        "publish_inventory",
        json!({}),
    )
    .await;
    read.close().await;
    health.close().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL; run_abilities.sh"]
async fn hit_and_confirm_hints_match_real_execution_routes() {
    let db = market_database::MarketDatabase::new("RUST_ABILITIES").await;
    let mut admin = PgConnection::connect(&db.admin_url).await.unwrap();
    for sql in [
        include_str!("fixtures/phase9_trades.sql"),
        include_str!("fixtures/phase9_confirm.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
    }
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let product = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| {
            p["qualification_profile"] == "scalar"
                && p["execution_mode"] == "ORDERBOOK"
                && p.get("specification_standard").is_none()
        })
        .unwrap();
    sqlx::query("SELECT trades_fixture_seed($1,$2)")
        .bind(product["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .bind(
            product["lanes"][0]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
        )
        .execute(&mut admin)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('confirm.policy',$1,false)")
        .bind(policy["products"].to_string())
        .execute(&mut admin)
        .await
        .unwrap();
    let trade: Uuid = sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL")
        .execute(&mut admin)
        .await
        .unwrap();
    let (app, read, health) = application(&db.runtime_url, true, false).await;
    assert_kyc_route_parity(
        &app,
        "POST",
        "/api/trades/",
        Uuid::from_u128(0x1001),
        "hit_ask_order",
        json!({"order_id":"00000000-0000-0000-0000-000000000001","quantity_mt":"1"}),
    )
    .await;
    assert_kyc_route_parity(
        &app,
        "PUT",
        &format!("/api/trades/{trade}/confirm"),
        Uuid::from_u128(0x2001),
        "confirm_trade",
        json!({}),
    )
    .await;
    read.close().await;
    health.close().await;
}
