//! Feature-gated PostgreSQL evidence adapter. All mutation authority stays here.
use super::super::command::{Canonical, LockPlan};
use super::super::locks::SliceKey;
use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
};
use sqlx::postgres::PgPoolOptions;

#[derive(Clone)]
struct Adapter {
    pool: sqlx::PgPool,
    token: String,
}

pub async fn serve_contract() {
    assert_eq!(std::env::var("ENVIRONMENT").as_deref(), Ok("test"));
    let bind = std::env::var("PHASE2_BIND").unwrap();
    assert!(bind.starts_with("127.0.0.1:"));
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("PHASE2_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let adapter = Adapter {
        pool,
        token: std::env::var("DISPOSABLE_TEST_TOKEN").unwrap(),
    };
    let router = Router::new()
        .route("/.well-known/verdaxis-disposable-test", get(identity))
        .route("/__contract__/order-write", post(handle))
        .with_state(adapter);
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    axum::serve(listener, router).await.unwrap();
}

async fn identity(State(a): State<Adapter>) -> Json<Value> {
    Json(json!({"disposable":true,"token":a.token}))
}

async fn handle(
    State(a): State<Adapter>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> axum::response::Response {
    if headers
        .get("x-disposable-token")
        .and_then(|h| h.to_str().ok())
        != Some(a.token.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match probe(a, v).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

async fn probe(a: Adapter, v: Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let id: Uuid = v["id"].as_str().ok_or("id")?.parse()?;
    let actor: Uuid = "20000000-0000-4000-8000-000000000001".parse()?;
    let org: Uuid = "10000000-0000-4000-8000-000000000001".parse()?;
    let slice = SliceKey {
        product_id: id,
        delivery_point_id: None,
        availability_window: "SPOT".into(),
    };
    let key = super::super::locks::market_slice_lock_key(id, None, "SPOT");
    let mode = v["mode"].as_str().ok_or("mode")?.to_owned();
    let mut blocker = if mode == "timeout" || mode == "ordered" {
        let mut tx = a.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(key)
            .execute(&mut *tx)
            .await?;
        Some(tx)
    } else {
        None
    };
    let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = if mode == "ordered" {
        let holder = blocker.take().unwrap();
        let pool = a.pool.clone();
        let observed = observed.clone();
        Some(tokio::spawn(async move {
            let class = (key as u64 >> 32) as i64;
            let object = (key as u64 & 0xffff_ffff) as i64;
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND classid::bigint=$1 AND objid::bigint=$2 AND NOT granted)")
                        .bind(class).bind(object).fetch_one(&pool).await.unwrap();
                    if waiting { break; }
                    tokio::task::yield_now().await;
                }
            }).await;
            if result.is_ok() {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            holder.rollback().await.unwrap();
            result
        }))
    } else {
        None
    };
    let context = CommandContext {
        actor_user_id: actor,
        effective_organization_id: Some(org),
        support_context_id: None,
        idempotency_key: Some(id.to_string()),
    };
    let command = OrderCommand {
        operation: Operation::OrderAmend,
        resource_id: id,
        payload: Canonical::Text(id.to_string()),
        locks: LockPlan {
            slices: vec![slice],
            ..Default::default()
        },
        events: vec![OutboxEvent {
            event_type: "phase2_boundary".into(),
            aggregate_type: "boundary".into(),
            aggregate_id: id.to_string(),
            participant_org_ids: vec![org.to_string()],
            payload: json!({"id":id}),
        }],
        response_status: 200,
        response_body: json!({"id":id}),
    };
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = attempts.clone();
    let pool = a.pool.clone();
    let fixture = v.clone();
    let opener = move || -> BoxFuture<'static, Result<PgWriteSession, WriteError>> {
        let pool = pool.clone();
        let mut fixture = fixture.clone();
        fixture["attempt"] = json!(count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1);
        Box::pin(async move {
            let mut session = open_session(Arc::new(Semaphore::new(1)), pool).await?;
            session.fixture = Some(fixture);
            Ok(session)
        })
    };
    let result = run_write_unit(opener, context, command).await;
    if let Some(tx) = blocker {
        tx.rollback().await?;
    }
    if let Some(release) = release {
        release.await??;
    }
    let lock_observed = observed.load(std::sync::atomic::Ordering::SeqCst);
    let (status, replayed) = match result {
        Ok(result) => (
            if mode == "lost-response" { 503 } else { 200 },
            result.replayed,
        ),
        Err(error) => (error.status.as_u16(), false),
    };
    Ok(
        json!({"id":id,"status":status,"replayed":replayed,"attempts":attempts.load(std::sync::atomic::Ordering::SeqCst),"lock_observed":lock_observed}),
    )
}

pub(super) async fn insert_fixture_receipt(
    session: &mut PgWriteSession,
    r: &ReceiptRecord,
    fixture: &Value,
) -> Result<(), WriteError> {
    let connection = session.connection()?;
    // Verify the real transaction-local settings, not just the exported constants.
    let bounds: (String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'),current_setting('statement_timeout')",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|e| write_error(&e))?;
    if bounds != ("500ms".into(), "5s".into()) {
        return Err(WriteError::other());
    }
    if fixture["mode"] == "rollback" {
        // The outbox INSERT has already run. Cause a real server error before receipt insertion.
        sqlx::query("SELECT 1/0")
            .execute(&mut *connection)
            .await
            .map_err(|e| write_error(&e))?;
    }
    let mode = fixture["mode"].as_str().unwrap();
    if (mode == "retry-40001" && fixture["attempt"] == 1) || mode == "retry-40P01-bound" {
        let statement = if mode == "retry-40001" {
            "DO $$ BEGIN RAISE EXCEPTION 'scheduled serialization' USING ERRCODE='40001'; END $$"
        } else {
            "DO $$ BEGIN RAISE EXCEPTION 'scheduled deadlock' USING ERRCODE='40P01'; END $$"
        };
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(|e| write_error(&e))?;
    }
    let body = serde_json::to_string(&r.response_body).map_err(|_| WriteError::other())?;
    sqlx::query("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,support_context_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::jsonb,'2026-10-19T00:00:00Z')")
        .bind(r.resource_id).bind(r.actor_user_id).bind(r.effective_organization_id).bind(r.support_context_id)
        .bind(r.operation.as_str()).bind(&r.idempotency_key).bind(&r.request_hash).bind(r.operation.resource_type())
        .bind(r.resource_id).bind(r.response_status as i16).bind(body).execute(connection).await.map_err(|e|write_error(&e))?;
    Ok(())
}
