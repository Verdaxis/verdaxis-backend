#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{VerificationSettings, limits::SharedLimits};
use verdaxis_read::{
    Config, MarketPolicy,
    commission_routes::{RouteContext, routes},
};

struct Fixture {
    admin: PgConnection,
    pool: sqlx::PgPool,
    app: Router,
    actors: [Uuid; 3],
    settings: Arc<VerificationSettings>,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_EFFECTS_ADMIN_DATABASE_URL")
            .expect("explicit migrated disposable URL");
        let runtime_url = std::env::var("RUST_EFFECTS_DATABASE_URL").unwrap();
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert!(a.path().ends_with("_test"));
        assert_eq!(
            (a.host_str(), a.port(), a.path()),
            (r.host_str(), r.port(), r.path())
        );
        assert_ne!(a.username(), r.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert_eq!(revision, "p89_20261022_trade_freeze");
        sqlx::raw_sql(include_str!("fixtures/phase8_commission.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let actors = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        sqlx::query("SELECT commission_fixture_reset($1,$2,$3)")
            .bind(actors[0])
            .bind(actors[1])
            .bind(actors[2])
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&runtime_url)
            .await
            .unwrap();
        let settings = Arc::new(VerificationSettings::new(
            "commission-test-signing-key-at-least-32-characters".into(),
            None,
            "commission-test".into(),
            "commission-test".into(),
        ));
        let config: Config = serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_trade_freeze","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).unwrap();
        let policy = serde_json::from_str::<MarketPolicy>(include_str!(
            "../../../deploy/rust/market-policy.json"
        ))
        .unwrap();
        let app = routes(Arc::new(RouteContext {
            pool: pool.clone(),
            permits: Arc::new(tokio::sync::Semaphore::new(8)),
            settings: settings.clone(),
            limits: SharedLimits::default(),
            policy: Arc::new(policy),
            config: Arc::new(config),
        }));
        Self {
            admin,
            pool,
            app,
            actors,
            settings,
        }
    }
    async fn seed(&mut self, count: i32) {
        sqlx::query("SELECT commission_fixture_rows($1)")
            .bind(count)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    fn token(&self, actor: usize) -> String {
        let now = chrono::Utc::now().timestamp();
        let _ = &self.settings;
        jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":self.actors[actor],"type":"access","iat":now,"exp":now+900,"iss":"commission-test","aud":"commission-test"}),&jsonwebtoken::EncodingKey::from_secret(b"commission-test-signing-key-at-least-32-characters")).unwrap()
    }
    async fn request(&self, actor: usize, method: &str, path: &str, body: Value) -> (u16, Value) {
        request(self.app.clone(), self.token(actor), method, path, body).await
    }
    async fn id(&mut self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM commissions ORDER BY id LIMIT 1")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn counts(&mut self) -> (i64, i64, i64) {
        let row=sqlx::query("SELECT (SELECT count(*) FROM audit_logs) a,(SELECT count(*) FROM notifications) n,(SELECT count(*) FROM market_event_outbox) o").fetch_one(&mut self.admin).await.unwrap();
        (row.get("a"), row.get("n"), row.get("o"))
    }
}
async fn request(
    app: Router,
    token: String,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("x-request-id", "11111111-2222-4333-8444-555555555555")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_is_serialized_and_audited() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("/api/orders/admin/commissions/{id}");
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM commissions WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = f.app.clone();
    let token = f.token(0);
    let p = path.clone();
    let first =
        tokio::spawn(
            async move { request(app, token, "PUT", &p, json!({"status":"INVOICED"})).await },
        );
    let mut observed = false;
    for _ in 0..400 {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE '%commissions%' AND pid<>pg_backend_pid())").fetch_one(&mut f.admin).await.unwrap();
        if waiting {
            observed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        observed,
        "first independent administrator must wait for the row"
    );
    let app = f.app.clone();
    let token = f.token(1);
    let p = path.clone();
    let second =
        tokio::spawn(async move { request(app, token, "PUT", &p, json!({"status":"PAID"})).await });
    let mut both_waiting = false;
    for _ in 0..400 {
        let waiting:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE '%commissions%' AND pid<>pg_backend_pid()").fetch_one(&mut f.admin).await.unwrap();
        if waiting >= 2 {
            both_waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        both_waiting,
        "both administrator connections must reach the commission row"
    );
    blocker.commit().await.unwrap();
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    assert_eq!(a.0, 200);
    assert_eq!(a.1["status"], "INVOICED");
    assert_eq!(b.0, 200);
    assert_eq!(b.1["status"], "PAID");
    let changes: Vec<Value> =
        sqlx::query_scalar("SELECT changes FROM audit_logs ORDER BY timestamp,id")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(
        changes,
        vec![
            json!({"status":{"from":"PENDING","to":"INVOICED"}}),
            json!({"status":{"from":"INVOICED","to":"PAID"}})
        ]
    );
    assert_eq!(f.counts().await, (2, 0, 0));
    let contexts: Vec<Option<String>> = sqlx::query_scalar("SELECT request_id FROM audit_logs")
        .fetch_all(&mut f.admin)
        .await
        .unwrap();
    assert!(
        contexts
            .iter()
            .all(|value| value.as_deref() == Some("11111111-2222-4333-8444-555555555555"))
    );
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT action,resource_type,resource_id FROM audit_logs")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert!(rows.iter().all(|r| r
        == &(
            "commission.updated".into(),
            "commission".into(),
            id.to_string()
        )));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_list_is_unbounded_and_preserves_the_array_shape() {
    let mut f = Fixture::new().await;
    assert_eq!(
        f.request(0, "GET", "/api/orders/admin/commissions", Value::Null)
            .await,
        (200, json!([]))
    );
    f.seed(1007).await;
    let (status, body) = f
        .request(
            0,
            "GET",
            "/api/orders/admin/commissions?limit=1&skip=1000",
            Value::Null,
        )
        .await;
    assert_eq!(status, 200);
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1007);
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM commissions ORDER BY created_at DESC,id DESC")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    for row in rows {
        assert_eq!(row.as_object().unwrap().len(), 8);
        assert!(row["amount_usd"].is_string());
        assert!(row["invoice_number"].is_null());
        assert!(row.get("notes").is_none());
    }
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_views_do_not_derive_from_trades() {
    let mut f = Fixture::new().await;
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let product = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK")
        .unwrap();
    sqlx::query("SELECT commission_fixture_trade($1,$2,$3)")
        .bind(product["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .bind(
            product["lanes"][0]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
        )
        .bind(product["code"].as_str().unwrap())
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.request(0, "GET", "/api/orders/admin/commissions", Value::Null)
            .await,
        (200, json!([]))
    );
    let (_, body) = f
        .request(
            0,
            "GET",
            "/api/orders/admin/commissions/summary",
            Value::Null,
        )
        .await;
    assert_eq!(body["paid_count"], 0);
    assert_eq!(
        body["total_paid_usd"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::ZERO
    );
    f.seed(1).await;
    assert_eq!(
        f.request(0, "GET", "/api/orders/admin/commissions", Value::Null)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn summary_totals_are_exact() {
    let mut f = Fixture::new().await;
    f.seed(1007).await;
    sqlx::query("SELECT commission_fixture_amounts()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, body) = f
        .request(
            0,
            "GET",
            "/api/orders/admin/commissions/summary",
            Value::Null,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body.as_object().unwrap().len(), 6);
    for (name, status) in [
        ("pending", "PENDING"),
        ("invoiced", "INVOICED"),
        ("paid", "PAID"),
    ] {
        let row=sqlx::query("SELECT count(*) n,coalesce(sum(amount_usd),0)::text total FROM commissions WHERE status=$1").bind(status).fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(body[format!("{name}_count")], row.get::<i64, _>("n"));
        assert_eq!(
            body[format!("total_{name}_usd")]
                .as_str()
                .unwrap()
                .parse::<rust_decimal::Decimal>()
                .unwrap(),
            row.get::<String, _>("total")
                .parse::<rust_decimal::Decimal>()
                .unwrap()
        );
    }
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_has_no_query_after_commit() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let source = include_str!("../src/order_write/transaction.rs");
    let commit = source
        .rfind("// No response query or failure can turn this known commit into a rerun.")
        .unwrap();
    let tail = &source[commit
        ..source[commit..]
            .find("#[cfg(feature = \"contract-tests\")]")
            .map(|n| commit + n)
            .unwrap_or(source.len())];
    assert!(!tail.contains("sqlx::query"));
    assert!(
        source[..commit]
            .rfind("command.prepare_response(&value)")
            .unwrap()
            < commit
    );
    let (status,body)=f.request(0,"PUT",&format!("/api/orders/admin/commissions/{id}"),json!({"status":"PAID","invoice_number":"A-1","invoice_date":"2026-10-25","payment_date":"2026-10-26","notes":"retained","amount_usd":"1"})).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "PAID");
    assert_eq!(body["invoice_number"], "A-1");
    assert!(body.get("notes").is_none());
    assert_ne!(body["amount_usd"], "1");
    let stored: String = sqlx::query_scalar("SELECT status FROM commissions WHERE id=$1")
        .bind(id)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(stored, body["status"].as_str().unwrap());
    assert_eq!(f.counts().await, (1, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_noop_and_reverse_status_keep_one_audit() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let path = format!("/api/orders/admin/commissions/{}", f.id().await);
    for body in [
        json!({"status":"PAID"}),
        json!({"status":"PENDING"}),
        json!({"status":"PENDING"}),
        json!({"notes":"changed"}),
        json!({}),
    ] {
        assert_eq!(f.request(0, "PUT", &path, body).await.0, 200);
    }
    let changes: Vec<Value> =
        sqlx::query_scalar("SELECT changes FROM audit_logs ORDER BY timestamp,id")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(
        changes,
        vec![
            json!({"status":{"from":"PENDING","to":"PAID"}}),
            json!({"status":{"from":"PAID","to":"PENDING"}}),
            json!({}),
            json!({}),
            json!({})
        ]
    );
    assert_eq!(f.counts().await, (5, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_admin_authority_precedes_resource_access() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    for path in [
        "/api/orders/admin/commissions".into(),
        "/api/orders/admin/commissions/summary".into(),
        format!("/api/orders/admin/commissions/{id}"),
        format!("/api/orders/admin/commissions/{}", Uuid::new_v4()),
    ] {
        let method = if path.ends_with(&id.to_string())
            || Uuid::parse_str(path.rsplit('/').next().unwrap()).is_ok()
        {
            "PUT"
        } else {
            "GET"
        };
        assert_eq!(
            f.request(2, method, &path, json!({"status":"PAID"}))
                .await
                .0,
            403
        );
    }
    assert_eq!(f.counts().await, (0, 0, 0));
    assert_eq!(
        f.request(
            0,
            "PUT",
            &format!("/api/orders/admin/commissions/{}", Uuid::new_v4()),
            json!({})
        )
        .await
        .0,
        404
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_update_and_audit_rollback_together() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    sqlx::query("SELECT commission_fixture_refuse_audit(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.request(
            0,
            "PUT",
            &format!("/api/orders/admin/commissions/{id}"),
            json!({"status":"PAID"})
        )
        .await
        .0,
        500
    );
    let status: String = sqlx::query_scalar("SELECT status FROM commissions WHERE id=$1")
        .bind(id)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(status, "PENDING");
    assert_eq!(f.counts().await, (0, 0, 0));
    sqlx::query("SELECT commission_fixture_refuse_audit(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.request(
            0,
            "PUT",
            &format!("/api/orders/admin/commissions/{id}"),
            json!({"status":"PAID"})
        )
        .await
        .0,
        200
    );
    assert_eq!(f.counts().await, (1, 0, 0));
}
