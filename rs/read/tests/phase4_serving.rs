use axum::{body::Body,http::Request};
use tower::ServiceExt;
use verdaxis_read::{AppState,Config};

#[tokio::test]
async fn production_composition_owns_every_business_operation() {
    let config:Config=serde_json::from_value(serde_json::json!({
        "environment":"test","release_sha":verdaxis_read::compiled_sha(),"migration_revision":"p3_20261020_auth_tokens",
        "database_url":"postgres://rust_app:synthetic@127.0.0.1:12345/verdaxis_serving_test","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":3,"read_admission_slots":2,"python_budget":52,"configured_max_connections":100,
        "phase2":{"jwt_secret":"synthetic-test-key-at-least-32-characters","jwt_issuer":"test","jwt_audience":"test","frontend_url":"https://app.example.test","email_from":"team@example.test","resend_api_key":"synthetic","auth_domain_datasets":"null"}
    })).unwrap();
    let pool=sqlx::postgres::PgPoolOptions::new().connect_lazy(&config.database_url).unwrap();
    let state=AppState::new(config,verdaxis_read::runtime::db::Pools {read:pool.clone(),health:pool},serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()).unwrap();
    let app=verdaxis_read::router(state);
    let manifest:serde_json::Value=serde_json::from_str(include_str!("../../../tests/rust_contract/phase4/routes.json")).unwrap();
    let mut count=0;
    for route in manifest["routes"].as_array().unwrap() {
        if route["path"]=="/api/ai/chat" {continue;}
        let mut path=route["path"].as_str().unwrap().to_owned();
        while let Some(start)=path.find('{') {let end=path[start..].find('}').unwrap()+start;path.replace_range(start..=end,"00000000-0000-0000-0000-000000000000");}
        let request=Request::builder().method(route["method"].as_str().unwrap()).uri(&path).body(Body::empty()).unwrap();
        let response=app.clone().oneshot(request).await.unwrap();
        assert!(!matches!(response.status().as_u16(),404|405),"{} {}",route["method"],path);
        count+=1;
    }
    assert_eq!(count,42);
}
