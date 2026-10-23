pub use verdaxis_read::{ApiError, auth, runtime};
#[allow(dead_code)]
#[path = "../src/auth/refresh.rs"]
mod refresh;

use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use verdaxis_read::auth::{TokenIssuer, limits::SharedLimits};

const SECRET: &str = "synthetic-refresh-key-at-least-32-characters";
fn state() -> refresh::RefreshState {
    refresh::RefreshState::new(PgPoolOptions::new().connect_lazy("postgres://unused:unused@127.0.0.1:1/unused").unwrap(), "production", SECRET, None, "verdaxis", "verdaxis-api", SharedLimits::default())
}
async fn code(token: Option<&str>, device: Option<&str>, body: &str) -> (StatusCode, String) {
    let cookies = token.map(|t| format!("refresh_token={t}")).into_iter().chain(device.map(|d| format!("device_session={d}"))).collect::<Vec<_>>().join("; ");
    let response = refresh::router(state()).oneshot(Request::post("/api/auth/refresh").header("origin", "https://app.verdaxis.exchange").header("cookie", cookies).body(Body::from(body.to_owned())).unwrap()).await.unwrap();
    let status=response.status();
    let body: serde_json::Value=serde_json::from_slice(&to_bytes(response.into_body(),8192).await.unwrap()).unwrap();
    (status, body["detail"]["code"].as_str().unwrap().to_owned())
}
#[tokio::test]
async fn refresh_body_token_fallback_is_refused() {
    assert_eq!(code(None,None,"{\"refresh_token\":\"synthetic\"}").await,(StatusCode::UNAUTHORIZED,"REFRESH_TOKEN_MISSING".into()));
}
#[tokio::test]
async fn refresh_access_and_stream_tokens_are_refused() {
    let issuer=TokenIssuer::new(SECRET,"verdaxis","verdaxis-api");
    let id=uuid::Uuid::new_v4();let now=chrono::Utc::now();
    for token in [issuer.access(id,now,None),issuer.stream(id,id,"production",now)] {
        assert_eq!(code(Some(&token),None,"").await.1,"REFRESH_TOKEN_INVALID");
    }
}
#[tokio::test]
async fn refresh_subject_vectors_are_refused() {
    use jsonwebtoken::{encode,Header,EncodingKey};
    for subject in [serde_json::json!(1),serde_json::json!([]),serde_json::json!({}),serde_json::Value::Null,serde_json::json!(true)] {
        let claims=serde_json::json!({"sub":subject,"type":"refresh","family_id":uuid::Uuid::new_v4(),"jti":"synthetic","exp":chrono::Utc::now().timestamp()+60,"iss":"verdaxis","aud":"verdaxis-api"});
        let token=encode(&Header::default(),&claims,&EncodingKey::from_secret(SECRET.as_bytes())).unwrap();
        assert_eq!(code(Some(&token),None,"").await.1,"REFRESH_TOKEN_INVALID");
    }
}
#[tokio::test]
async fn refresh_requires_valid_device_and_reports_expiry() {
    let issuer=TokenIssuer::new(SECRET,"verdaxis","verdaxis-api");
    let id=uuid::Uuid::new_v4();
    let token=issuer.refresh(id,id,"synthetic",chrono::Utc::now(),None);
    assert_eq!(code(Some(&token),None,"").await.1,"REFRESH_DEVICE_REQUIRED");
    assert_eq!(code(Some(&token),Some("short"),"").await.1,"REFRESH_DEVICE_REQUIRED");
    let expired=issuer.refresh(id,id,"synthetic",chrono::Utc::now()-chrono::Duration::days(8),None);
    assert_eq!(code(Some(&expired),None,"").await.1,"REFRESH_TOKEN_EXPIRED");
}
