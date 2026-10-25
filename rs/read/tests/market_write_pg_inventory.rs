#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{AppState, Config, MarketPolicy, auth::VerificationSettings};

const KEY: &str = "inventory-fixture-signing-key-at-least-32-characters";
struct Fixture {
    admin: PgConnection,
    app: Router,
    actor: Uuid,
    organization: Uuid,
    port: String,
    product: Value,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_INVENTORY_ADMIN_DATABASE_URL")
            .expect("explicit disposable administrator URL");
        let runtime_url = std::env::var("RUST_INVENTORY_DATABASE_URL").unwrap();
        for value in [&admin_url, &runtime_url] {
            let url = url::Url::parse(value).unwrap();
            assert_eq!(url.host_str(), Some("127.0.0.1"));
            assert!(url.path().ends_with("_test"));
        }
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_inventory.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let raw: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let policy: MarketPolicy = serde_json::from_value(raw.clone()).unwrap();
        let product = raw["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["qualification_profile"] == "scalar"
                    && p["specification_standard"].is_null()
                    && p["execution_mode"] == "ORDERBOOK"
            })
            .unwrap()
            .clone();
        let actor = Uuid::new_v4();
        let organization = Uuid::new_v4();
        let port = format!("i-{}", &actor.simple().to_string()[..12]);
        sqlx::query("SELECT inventory_fixture_seed($1,$2,$3,$4)")
            .bind(organization)
            .bind(actor)
            .bind(&port)
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
        let config = Config {
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "p89_20261022_watchlist_producer".into(),
            database_url: runtime_url,
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 4,
            pool_max: 5,
            python_budget: 52,
            configured_max_connections: 100,
            kyc_execution_gate_enabled: true,
        };
        let pools = verdaxis_read::connect(&config).await.unwrap();
        let state = AppState::new(config, pools, policy).unwrap();
        let app = verdaxis_read::routes::market_write::inventory::router(
            state,
            VerificationSettings::new(
                KEY.into(),
                None,
                "inventory-test".into(),
                "inventory-test".into(),
            ),
        );
        Self {
            admin,
            app,
            actor,
            organization,
            port,
            product,
        }
    }
    fn body(&self) -> Value {
        json!({"port_id":self.port,"product_name":self.product["name"],"fuel_type":self.product["fuel_type"],"current_stock_mt":"0","incoming_stock_mt":"12.34","price_per_mt_usd":"123.45"})
    }
    async fn request(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let now = chrono::Utc::now().timestamp();
        let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":self.actor,"type":"access","iss":"inventory-test","aud":"inventory-test","iat":now,"exp":now+300}),&jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            },
        )
    }
    async fn create(&self) -> Uuid {
        let (status, body) = self.request("POST", "/api/inventory", self.body()).await;
        assert_eq!(status, 200, "{body}");
        body["id"].as_str().unwrap().parse().unwrap()
    }
    async fn counts(&mut self) -> (i64, i64, i64) {
        let row=sqlx::query("SELECT (SELECT count(*) FROM market_event_outbox WHERE participant_org_ids::jsonb @> jsonb_build_array($1::text)) AS events,(SELECT count(*) FROM audit_logs WHERE user_id=$2) AS audits,(SELECT count(*) FROM notifications WHERE user_id=$2) AS notifications").bind(self.organization).bind(self.actor).fetch_one(&mut self.admin).await.unwrap();
        (
            row.get("events"),
            row.get("audits"),
            row.get("notifications"),
        )
    }
    async fn reference(&mut self, id: Uuid, status: &str, trade: Option<&str>) {
        sqlx::query("SELECT inventory_fixture_reference($1,$2,$3,$4,$5,$6,$7)")
            .bind(id)
            .bind(self.organization)
            .bind(self.actor)
            .bind(
                self.product["id"]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap(),
            )
            .bind(
                self.product["lanes"][0]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap(),
            )
            .bind(status)
            .bind(trade)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_create_accepts_zero_stock() {
    let mut f = Fixture::new().await;
    let id = f.create().await;
    let row=sqlx::query("SELECT current_stock_mt::text AS current,reserved_stock_mt::text AS reserved,owner_user_id FROM inventory_items WHERE id=$1").bind(id).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(
        row.get::<String, _>("current")
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::ZERO
    );
    assert_eq!(
        row.get::<String, _>("reserved")
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::ZERO
    );
    assert_eq!(row.get::<Uuid, _>("owner_user_id"), f.actor);
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_create_returns_200() {
    let mut f = Fixture::new().await;
    let (status, body) = f.request("POST", "/api/inventory", f.body()).await;
    assert_eq!(status, 200, "{body}");
    for field in [
        "current_stock_mt",
        "incoming_stock_mt",
        "reserved_stock_mt",
        "price_per_mt_usd",
    ] {
        assert!(body[field].is_string(), "{field}");
    }
    assert_eq!(body["supplier_id"], f.organization.to_string());
    assert_eq!(body["fame_terms"], Value::Null);
    assert_eq!(body["off_spec_notes"], Value::Null);
    assert_eq!(f.counts().await, (1, 0, 0));
    let event: Value =
        sqlx::query_scalar("SELECT payload FROM market_event_outbox WHERE aggregate_id=$1")
            .bind(body["id"].as_str().unwrap())
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(event["id"], body["id"]);
    assert_eq!(event["_public_market_invalidation"], true);
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_crud_refusal_matrix() {
    let mut f = Fixture::new().await;
    let id = f.create().await;
    let path = format!("/api/inventory/{id}");
    for (column, value) in [
        ("role", "BUYER"),
        ("status", "PENDING"),
        ("email_verified", "false"),
        ("must_change_password", "true"),
        ("kyc_status", "REJECTED"),
    ] {
        sqlx::query("SELECT inventory_fixture_actor($1,$2,$3)")
            .bind(f.actor)
            .bind(column)
            .bind(value)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.counts().await;
        for (method, url, payload) in [
            ("POST", "/api/inventory", f.body()),
            ("PATCH", path.as_str(), json!({"current_stock_mt":"1"})),
            ("DELETE", path.as_str(), Value::Null),
        ] {
            let (status, body) = f.request(method, url, payload).await;
            assert_eq!(status, 403, "{column} {method}: {body}");
        }
        assert_eq!(f.counts().await, before);
        sqlx::query("SELECT inventory_fixture_actor($1,$2,$3)")
            .bind(f.actor)
            .bind(column)
            .bind(match column {
                "role" => "SUPPLIER",
                "status" => "APPROVED",
                "email_verified" => "true",
                "must_change_password" => "false",
                _ => "PENDING",
            })
            .execute(&mut f.admin)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE organizations SET verification_status='PENDING' WHERE id=$1")
        .bind(f.organization)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request("PATCH", &path, json!({})).await.0, 403);
    sqlx::query("UPDATE organizations SET verification_status='APPROVED' WHERE id=$1")
        .bind(f.organization)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET password_changed_at=now()+interval '1 minute' WHERE id=$1")
        .bind(f.actor)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request("PATCH", &path, json!({})).await.0, 401);
    sqlx::query("UPDATE users SET password_changed_at=NULL,authentication_revoked_at=now()+interval '1 minute' WHERE id=$1").bind(f.actor).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request("DELETE", &path, Value::Null).await.0, 401);
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_indicative_admission_does_not_require_execution_kyc() {
    let mut f = Fixture::new().await;
    for kyc in ["PENDING", "APPROVED"] {
        sqlx::query("UPDATE users SET kyc_status=$2,kyc_organization_id=CASE WHEN $2='APPROVED' THEN organization_id ELSE NULL END WHERE id=$1").bind(f.actor).bind(kyc).execute(&mut f.admin).await.unwrap();
        f.create().await;
    }
    sqlx::query("UPDATE users SET kyc_organization_id=$2 WHERE id=$1")
        .bind(f.actor)
        .bind(Uuid::new_v4())
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request("POST", "/api/inventory", f.body()).await.0, 403);
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_edit_refuses_reserved_or_referenced() {
    let mut f = Fixture::new().await;
    let id = f.create().await;
    sqlx::query("UPDATE inventory_items SET reserved_stock_mt=1 WHERE id=$1")
        .bind(id)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.request(
            "PATCH",
            &format!("/api/inventory/{id}"),
            json!({"current_stock_mt":"3"})
        )
        .await
        .0,
        409
    );
    for (status, trade) in [
        ("OPEN", None),
        ("PARTIALLY_FILLED", None),
        ("FILLED", Some("PENDING_CONFIRMATION")),
        ("FILLED", Some("CONFIRMED")),
        ("FILLED", Some("DELIVERED")),
    ] {
        let id = f.create().await;
        f.reference(id, status, trade).await;
        let before = f.counts().await;
        assert_eq!(
            f.request(
                "PATCH",
                &format!("/api/inventory/{id}"),
                json!({"current_stock_mt":"3"})
            )
            .await
            .0,
            409,
            "{status} {trade:?}"
        );
        assert_eq!(f.counts().await, before);
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn delete_refuses_any_reference() {
    let mut f = Fixture::new().await;
    for status in ["CANCELLED", "EXPIRED", "FILLED", "OPEN"] {
        let id = f.create().await;
        f.reference(id, status, None).await;
        assert_eq!(
            f.request("DELETE", &format!("/api/inventory/{id}"), Value::Null)
                .await
                .0,
            409
        );
    }
    let id = f.create().await;
    let (status, body) = f
        .request("DELETE", &format!("/api/inventory/{id}"), Value::Null)
        .await;
    assert_eq!((status, body), (204, Value::Null));
    assert_eq!(
        f.request("DELETE", &format!("/api/inventory/{id}"), Value::Null)
            .await
            .0,
        404
    );
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn port_authority_is_enforced() {
    let mut f = Fixture::new().await;
    let mut body = f.body();
    body["port_id"] = json!("missing-port");
    assert_eq!(f.request("POST", "/api/inventory", body).await.0, 404);
    sqlx::query("UPDATE ports SET name='Noncanonical fixture' WHERE id=$1")
        .bind(&f.port)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request("POST", "/api/inventory", f.body()).await.0, 404);
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn listings_my_is_a_compatibility_dict() {
    let mut f = Fixture::new().await;
    let id = f.create().await;
    f.reference(id, "CANCELLED", None).await;
    let (status, body) = f.request("GET", "/api/listings/my", Value::Null).await;
    assert_eq!(status, 200, "{body}");
    let item = &body[0];
    for field in [
        "product_name",
        "market_product",
        "fuel_type",
        "fuel_grade",
        "region",
        "quantity_mt",
        "price_per_mt_usd",
        "match_count",
        "observed_at",
        "source_kind",
        "scope",
        "demo_status",
        "is_demo_listing",
        "unknown_count",
        "delivery_point_id",
    ] {
        assert!(item.get(field).is_some(), "{field}: {item}");
    }
    assert_eq!(item["status"], "CANCELLED");
    assert_eq!(item["match_count"], 0);
    assert!(item["quantity_mt"].is_string());
    assert!(item.get("remaining_quantity_mt").is_none());
    sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
        .bind(f.actor)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.request("GET", "/api/listings/my", Value::Null).await,
        (200, json!([]))
    );
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_tenant_scope_and_pagination() {
    let mut f = Fixture::new().await;
    let first = f.create().await;
    let second = f.create().await;
    let (status, items) = f
        .request("GET", "/api/inventory?limit=1", Value::Null)
        .await;
    assert_eq!(status, 200);
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["id"], second.to_string());
    assert_eq!(
        f.request("GET", "/api/inventory?skip=1&limit=1", Value::Null)
            .await
            .1[0]["id"],
        first.to_string()
    );
    sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
        .bind(f.actor)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.request("GET", "/api/inventory", Value::Null).await,
        (200, json!([]))
    );
    assert_eq!(
        f.request("PATCH", &format!("/api/inventory/{first}"), json!({}))
            .await
            .0,
        400
    );
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_update_tracks_changes_without_additional_effects() {
    let mut f = Fixture::new().await;
    let id = f.create().await;
    for patch in [
        json!({}),
        json!({"current_stock_mt":"9.25","price_per_mt_usd":null}),
    ] {
        let (status, body) = f
            .request("PATCH", &format!("/api/inventory/{id}"), patch.clone())
            .await;
        assert_eq!(status, 200, "{body}");
        let payload:Value=sqlx::query_scalar("SELECT payload FROM market_event_outbox WHERE aggregate_id=$1 AND event_type='inventory_updated' ORDER BY created_at DESC LIMIT 1").bind(id.to_string()).fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(
            payload.get("_public_market_invalidation").is_some(),
            patch.as_object().unwrap().len() > 0
        );
    }
    assert_eq!(f.counts().await, (3, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_failure_rolls_back_the_record_and_outbox() {
    let mut f = Fixture::new().await;
    sqlx::query("SELECT inventory_fixture_outbox_failure(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request("POST", "/api/inventory", f.body()).await.0, 500);
    sqlx::query("SELECT inventory_fixture_outbox_failure(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_items WHERE supplier_id=$1")
            .bind(f.organization)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(f.counts().await, (0, 0, 0));
}

async fn wait_for_inventory_lock(admin: &mut PgConnection) {
    for _ in 0..200 {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE '%inventory_items%')").fetch_one(&mut *admin).await.unwrap();
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("independent inventory waiter was not observed");
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_edit_and_delete_recheck_after_an_independent_lock_holder() {
    for method in ["PATCH", "DELETE"] {
        let mut f = Fixture::new().await;
        let id = f.create().await;
        let url = std::env::var("RUST_INVENTORY_ADMIN_DATABASE_URL").unwrap();
        let mut holder = PgConnection::connect(&url).await.unwrap();
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut holder)
            .await
            .unwrap();
        let observer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_ne!(holder_pid, observer_pid);
        let mut tx = holder.begin().await.unwrap();
        sqlx::query("SELECT id FROM inventory_items WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap();
        let app = f.app.clone();
        let actor = f.actor;
        let task = tokio::spawn(async move {
            let now = chrono::Utc::now().timestamp();
            let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":actor,"type":"access","iss":"inventory-test","aud":"inventory-test","iat":now,"exp":now+300}),&jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
            app.oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/inventory/{id}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"current_stock_mt":"5"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        });
        wait_for_inventory_lock(&mut f.admin).await;
        sqlx::query("UPDATE inventory_items SET reserved_stock_mt=1 WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(task.await.unwrap(), 409);
        assert_eq!(f.counts().await, (1, 0, 0));
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_admission_rechecks_an_independent_account_change() {
    let mut f = Fixture::new().await;
    let mut holder =
        PgConnection::connect(&std::env::var("RUST_INVENTORY_ADMIN_DATABASE_URL").unwrap())
            .await
            .unwrap();
    let mut tx = holder.begin().await.unwrap();
    sqlx::query("UPDATE users SET email_verified=false WHERE id=$1")
        .bind(f.actor)
        .execute(&mut *tx)
        .await
        .unwrap();
    let request = f.request("POST", "/api/inventory", f.body());
    let release = async {
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        tx.commit().await.unwrap();
    };
    let (response, ()) = tokio::join!(request, release);
    assert_eq!(response.0, 403, "{}", response.1);
    assert_eq!(f.counts().await, (0, 0, 0));
}
