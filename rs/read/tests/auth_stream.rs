pub use verdaxis_read::{ApiError, auth};
#[path = "../src/auth/stream.rs"]
mod stream;

use chrono::{TimeZone, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde_json::Value;
use uuid::Uuid;

const SECRET: &str = "synthetic-stream-key-at-least-32-characters";

fn issuer() -> stream::StreamTokens {
    stream::StreamTokens::new(
        auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api"),
        " Production ",
        auth::limits::SharedLimits::default(),
    )
}

fn actor(organization_id: Option<Uuid>) -> auth::CurrentActor {
    auth::CurrentActor {
        id: Uuid::new_v4(),
        organization_id,
        role: Some("BUYER".into()),
        email_verified: true,
    }
}

#[test]
fn stream_token_claims_and_ttl() {
    let actor = actor(Some(Uuid::new_v4()));
    let now = Utc.timestamp_opt(1_792_800_000, 123_456_000).unwrap();
    let response = issuer().issue(&actor, "client", now).unwrap();
    let body = serde_json::to_value(&response).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 1);
    let token = body["stream_token"].as_str().unwrap();
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&["verdaxis"]);
    validation.set_audience(&["verdaxis-api"]);
    validation.validate_exp = false;
    let claims = decode::<Value>(
        token,
        &DecodingKey::from_secret(SECRET.as_bytes()),
        &validation,
    )
    .unwrap()
    .claims;
    assert_eq!(claims["sub"], actor.id.to_string());
    assert_eq!(claims["org_id"], actor.organization_id.unwrap().to_string());
    assert_eq!(claims["environment"], "production");
    assert_eq!(claims["type"], "stream");
    assert_eq!(claims["iat"], now.timestamp());
    assert_eq!(claims["iat_us"], now.timestamp_micros());
    assert_eq!(claims["exp"], now.timestamp() + 60);
    assert_eq!(claims["iss"], "verdaxis");
    assert_eq!(claims["aud"], "verdaxis-api");
    assert!(claims.get("role").is_none());
    assert!(claims.get("jti").is_none());
}

#[test]
fn stream_token_requires_organization() {
    let error = issuer()
        .issue(&actor(None), "client", Utc::now())
        .err()
        .unwrap();
    assert_eq!(error.status.as_u16(), 403);
    assert_eq!(error.detail["code"], "AUTH_ORGANIZATION_REQUIRED");
}

#[test]
fn stream_token_limit_is_30_per_minute() {
    let issuer = issuer();
    let actor = actor(Some(Uuid::new_v4()));
    let now = Utc.timestamp_opt(1_792_800_000, 0).unwrap();
    for _ in 0..30 {
        issuer.issue(&actor, "client", now).unwrap();
    }
    let error = issuer.issue(&actor, "client", now).err().unwrap();
    assert_eq!(error.status.as_u16(), 429);
    assert_eq!(error.detail["code"], "RATE_LIMITED");
    assert_eq!(error.retry_after, Some(60));
    issuer.issue(&actor, "other-client", now).unwrap();
    issuer
        .issue(&actor, "client", now + chrono::Duration::seconds(60))
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn stream_token_uses_current_bearer_actor() {
    use std::sync::Arc;
    use verdaxis_read::{AppState, Config, MarketPolicy};

    let owner = sqlx::PgPool::connect(&std::env::var("AUTH_STORE_TEST_OWNER_URL").unwrap())
        .await
        .unwrap();
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var("AUTH_STORE_TEST_DATABASE_URL").unwrap(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 2,
        pool_max: 3,
        python_budget: 52,
        configured_max_connections: 100,
    };
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let state = AppState::new(
        config.clone(),
        pools,
        MarketPolicy {
            source_sha: config.release_sha.clone(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let id = Uuid::new_v4();
    let organization = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, type) VALUES ($1, $2, 'FUEL_BUYER')")
        .bind(organization)
        .bind(format!("Stream fixture {organization}"))
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id,email,password_hash,role,status,email_verified,must_change_password,organization_id) VALUES ($1,$2,'unused','BUYER','APPROVED',true,false,$3)")
        .bind(id).bind(format!("{id}@stream.invalid")).bind(organization).execute(&owner).await.unwrap();
    let now = Utc::now();
    let settings = auth::VerificationSettings::new(
        SECRET.into(),
        None,
        "verdaxis".into(),
        "verdaxis-api".into(),
    );
    let access_token =
        auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api").access(id, now, None);
    let tokens = Arc::new(issuer());
    for expected in [Some(organization), None] {
        sqlx::query("UPDATE users SET organization_id=$2 WHERE id=$1")
            .bind(id)
            .bind(expected)
            .execute(&owner)
            .await
            .unwrap();
        let tokens = tokens.clone();
        let access =
            auth::verify_authorization(Some(&format!("Bearer {access_token}")), &settings, now)
                .unwrap();
        let result = auth::run_authenticated(
            &state,
            access,
            auth::AuthenticatedRequest::new("GET", "/api/auth/stream-token", false),
            move |tx| Box::pin(async move { tokens.issue(tx.actor(), "client", now) }),
        )
        .await;
        if expected.is_some() {
            assert!(result.is_ok());
        } else {
            assert!(
                matches!(result, Err(auth::AuthenticatedError::Database(error)) if error.status.as_u16() == 403 && error.detail["code"] == "AUTH_ORGANIZATION_REQUIRED")
            );
        }
    }
    sqlx::query("UPDATE users SET must_change_password=true WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await
        .unwrap();
    let access =
        auth::verify_authorization(Some(&format!("Bearer {access_token}")), &settings, now)
            .unwrap();
    let result = auth::run_authenticated(
        &state,
        access,
        auth::AuthenticatedRequest::new("GET", "/api/auth/stream-token", false),
        |_| {
            Box::pin(async {
                panic!("account refusal must precede stream issuance");
                #[allow(unreachable_code)]
                Ok(())
            })
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(auth::AuthenticatedError::Authentication(_))
    ));
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(organization)
        .execute(&owner)
        .await
        .unwrap();
}
