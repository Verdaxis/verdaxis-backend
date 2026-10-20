//! Disposable-only HTTP write probe. Not linked into the deployed binary.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row};

#[derive(Clone)]
struct Probe {
    database: String,
    token: String,
}

async fn identity(State(probe): State<Probe>) -> Json<Value> {
    Json(json!({"disposable": true, "token": probe.token}))
}

async fn write(
    State(probe): State<Probe>,
    headers: HeaderMap,
    Json(action): Json<Value>,
) -> axum::response::Response {
    if headers
        .get("x-disposable-token")
        .and_then(|value| value.to_str().ok())
        != Some(&probe.token)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match execute(&probe, &action).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe transaction failed",
        )
            .into_response(),
    }
}

async fn execute(
    probe: &Probe,
    action: &Value,
) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
    let ids = action["ids"].as_array().ok_or("ids")?;
    if ids.len() != 2 {
        return Err("two ids required".into());
    }
    let now = action["now"].as_str().ok_or("now")?;
    chrono::DateTime::parse_from_rfc3339(now)?;
    let code = action["referral_code"].as_str().ok_or("code")?;
    let fault = action["fault"].as_str().ok_or("fault")?;
    let mut connection = PgConnection::connect(&probe.database).await?;
    let mut transaction = connection.begin().await?;
    let row = sqlx::query("SELECT rolsuper, pg_get_userbyid(datdba)=current_user AS owner FROM pg_roles CROSS JOIN pg_database WHERE rolname=current_user AND datname=current_database()")
        .fetch_one(&mut *transaction).await?;
    if row.get::<bool, _>("rolsuper") || row.get::<bool, _>("owner") {
        return Err("non-owner role required".into());
    }
    sqlx::query("SELECT set_config('verdaxis.test_action_clock',$1,true)")
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    let database_now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *transaction)
        .await?;
    let actor = uuid::Uuid::parse_str("20000000-0000-4000-8000-000000000001")?;
    let mut outcome = "committed";
    for (index, value) in ids.iter().enumerate() {
        let id = uuid::Uuid::parse_str(value.as_str().ok_or("id")?)?;
        sqlx::query("INSERT INTO feedback_entries(id,user_id,message,page,created_at) VALUES($1,$2,$3,NULL,current_setting('verdaxis.test_action_clock')::timestamptz)")
            .bind(id).bind(actor).bind(code).execute(&mut *transaction).await?;
        if index == 0 && fault == "between-writes" {
            outcome = "rolled-back";
            break;
        }
    }
    if outcome != "rolled-back" && std::env::var("PHASE2_EMIT_EVENTS").as_deref() == Ok("true") {
        sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at) VALUES($1,'phase2_probe','feedback',$2,$3::json,$4::json,current_setting('verdaxis.test_action_clock')::timestamptz)")
            .bind(uuid::Uuid::parse_str(ids[0].as_str().ok_or("id")?)?)
            .bind(ids[0].as_str().ok_or("id")?)
            .bind("[\"10000000-0000-4000-8000-000000000001\"]")
            .bind("{\"version\":1}").execute(&mut *transaction).await?;
    }
    if outcome == "rolled-back" || fault == "before-commit" {
        transaction.rollback().await?;
        outcome = "rolled-back";
    } else {
        transaction.commit().await?;
        if fault == "after-commit" {
            outcome = "ambiguous";
        }
    }
    let mut response = (
        if outcome == "ambiguous" {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::OK
        },
        Json(json!({"outcome":outcome,"ids":ids,"now":now,"referral_code":code})),
    )
        .into_response();
    response
        .headers_mut()
        .insert("x-database-now", database_now.to_rfc3339().parse()?);
    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("test only".into());
    }
    let bind: std::net::SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err("loopback ephemeral bind required".into());
    }
    let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
    if token.len() < 32 {
        return Err("identity token required".into());
    }
    let probe = Probe {
        database: std::env::var("PHASE2_DATABASE_URL")?,
        token,
    };
    let app = Router::new()
        .route("/.well-known/verdaxis-disposable-test", get(identity))
        .route("/__contract__/phase2/write", post(write))
        .with_state(probe);
    axum::serve(tokio::net::TcpListener::bind(bind).await?, app).await?;
    Ok(())
}
