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
const SUPPORT_CONTEXT: &str = "x-verdaxis-market-support-context";

fn bearer(actor: Uuid) -> String {
    let now = chrono::Utc::now().timestamp();
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({"sub":actor,"type":"access","iss":"inventory-test","aud":"inventory-test","iat":now,"exp":now+300}),
        &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap();
    format!("Bearer {token}")
}

/// Send one request to the component router and decode its JSON body.
async fn send(
    app: Router,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Body,
) -> (u16, Value) {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, value.as_str());
    }
    let response = app.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

/// Send one JSON request as the actor.
async fn authorized(
    app: Router,
    actor: Uuid,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let headers = [
        ("authorization", bearer(actor)),
        ("content-type", "application/json".to_owned()),
    ];
    send(app, method, path, &headers, Body::from(body.to_string())).await
}

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
        authorized(self.app.clone(), self.actor, method, path, body).await
    }
    async fn set_actor(&mut self, column: &str, value: &str) {
        sqlx::query("SELECT inventory_fixture_actor($1,$2,$3)")
            .bind(self.actor)
            .bind(column)
            .bind(value)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn create(&self) -> Uuid {
        let (status, body) = self.request("POST", "/api/inventory", self.body()).await;
        assert_eq!(status, 200, "{body}");
        body["id"].as_str().unwrap().parse().unwrap()
    }
    async fn counts(&mut self) -> (i64, i64, i64) {
        let row=sqlx::query("SELECT (SELECT count(*) FROM market_event_outbox WHERE participant_org_ids::jsonb @> jsonb_build_array($1::text)) AS events,(SELECT count(*) FROM audit_logs WHERE user_id=$2) AS audits,(SELECT count(*) FROM notifications WHERE recipient_id=$2) AS notifications").bind(self.organization).bind(self.actor).fetch_one(&mut self.admin).await.unwrap();
        (
            row.get("events"),
            row.get("audits"),
            row.get("notifications"),
        )
    }
    async fn reference(&mut self, id: Uuid, status: &str, trade: Option<&str>) {
        sqlx::query("SELECT inventory_fixture_reference($1,$2,$3,$4,$5,$6,$7,$8)")
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
            .bind(self.product["code"].as_str().unwrap())
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
        .bind(Fixture::new().await.organization)
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
    assert_eq!(f.counts().await, (0, 0, 0));
    // A port outside the canonical delivery points can hold a private record;
    // only publication refuses it.
    sqlx::query("UPDATE ports SET name='Noncanonical fixture' WHERE id=$1")
        .bind(&f.port)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, created) = f.request("POST", "/api/inventory", f.body()).await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["port_id"], json!(f.port));
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM market_event_outbox WHERE aggregate_id=$1 AND event_type='inventory_created'",
    )
    .bind(created["id"].as_str().unwrap())
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert!(
        payload.get("_public_market_invalidation").is_none(),
        "{payload}"
    );
    assert_eq!(f.counts().await, (1, 0, 0));
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
    // Market admission requires an organization, so every write is refused.
    let path = format!("/api/inventory/{first}");
    for (method, url, payload) in [
        ("POST", "/api/inventory", f.body()),
        ("PATCH", path.as_str(), json!({})),
        ("DELETE", path.as_str(), Value::Null),
    ] {
        let (status, refusal) = f.request(method, url, payload).await;
        assert_eq!(status, 403, "{method}: {refusal}");
        assert_eq!(refusal["detail"]["code"], "EXECUTION_NOT_AVAILABLE");
    }
    assert_eq!(f.counts().await, (2, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_update_tracks_changes_without_additional_effects() {
    let mut f = Fixture::new().await;
    let id = f.create().await;
    for patch in [
        json!({}),
        json!({"current_stock_mt":"0.00"}),
        json!({"current_stock_mt":"9.25","price_per_mt_usd":null}),
    ] {
        let (status, body) = f
            .request("PATCH", &format!("/api/inventory/{id}"), patch.clone())
            .await;
        assert_eq!(status, 200, "{body}");
        let payload:Value=sqlx::query_scalar("SELECT payload FROM market_event_outbox WHERE aggregate_id=$1 AND event_type='inventory_updated' ORDER BY created_at DESC LIMIT 1").bind(id.to_string()).fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(
            payload.get("_public_market_invalidation").is_some(),
            patch.get("price_per_mt_usd").is_some()
        );
    }
    assert_eq!(f.counts().await, (4, 0, 0));
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

/// Wait until another session waits for a row lock in a statement on `table`.
async fn wait_for_lock(admin: &mut PgConnection, table: &str) {
    for _ in 0..200 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE $1)",
        )
        .bind(format!("%{table}%"))
        .fetch_one(&mut *admin)
        .await
        .unwrap();
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("independent {table} waiter was not observed");
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
        wait_for_lock(&mut f.admin, "inventory_items").await;
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

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_foreign_and_missing_records_share_the_refusal() {
    let mut owner = Fixture::new().await;
    let foreign = Fixture::new().await;
    let id = foreign.create().await;
    for method in ["PATCH", "DELETE"] {
        let mut refusals = Vec::new();
        for target in [id, Uuid::new_v4()] {
            let result = owner
                .request(method, &format!("/api/inventory/{target}"), json!({}))
                .await;
            assert_eq!(result.0, 404, "{}", result.1);
            refusals.push(result.1);
        }
        assert_eq!(refusals[0], refusals[1], "{method}");
    }
    assert_eq!(
        owner.request("GET", "/api/inventory", Value::Null).await.1,
        json!([])
    );
    assert_eq!(owner.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_declaration_merge_preserves_private_terms_and_derived_fields() {
    let f = Fixture::new().await;
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let fame = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["qualification_profile"] == "fame_v1")
        .unwrap();
    let mut body = f.body();
    body["product_name"] = fame["name"].clone();
    body["fuel_type"] = fame["fuel_type"].clone();
    body["fame_terms"] = json!({"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"PENDING","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","sustainability_scheme":"ISCC_EU","certificate_reference":"fixture-reference","certificate_holder":"Fixture supplier","certificate_valid_until":"2099-06-30","evidence_status":"DECLARED","document_references":[],"evidence_due":"BEFORE_LOADING","production_origin":"NL","ci_gco2e_mj":"10.25","ci_methodology":"Fixture lifecycle","ci_boundary":"WTW","ci_basis":"ACTUAL","lhv_mj_kg":"37.25"});
    let (status, created) = f.request("POST", "/api/inventory", body.clone()).await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["carbon_intensity_gco2_mj"], "10.25");
    assert_eq!(created["energy_density_mj_kg"], "37.25");
    assert_eq!(created["certification_scheme"], "ISCC_EU");
    assert_eq!(
        created["fame_terms"]["certificate_reference"],
        "fixture-reference"
    );
    let path = format!("/api/inventory/{}", created["id"].as_str().unwrap());
    let (status, patched) = f
        .request("PATCH", &path, json!({"current_stock_mt":"6.25"}))
        .await;
    assert_eq!(status, 200, "{patched}");
    assert_eq!(patched["fame_terms"], created["fame_terms"]);
    assert_eq!(created["fame_terms"]["astm_grade"], Value::Null);
    assert!(created["fame_terms"].get("astm_grade").is_some());
    assert_eq!(created["fame_terms"]["quality_evidence"], Value::Null);
    assert!(created["fame_terms"].get("quality_evidence").is_some());
    assert_eq!(
        f.request("PATCH", &path, json!({"fame_terms":null}))
            .await
            .0,
        422
    );
    let lng = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["qualification_profile"] == "lng_v1")
        .unwrap();
    body["product_name"] = lng["name"].clone();
    body["fuel_type"] = lng["fuel_type"].clone();
    body["fame_terms"] = Value::Null;
    let vector: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/vectors/terms/private.json"
    ))
    .unwrap();
    body["lng_terms"] = vector["row"]["lng_terms"].clone();
    body["carbon_intensity_gco2_mj"] = json!("99.25");
    body["carbon_intensity_method"] = json!("Fixture input");
    let (status, created) = f.request("POST", "/api/inventory", body).await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["carbon_intensity_gco2_mj"], Value::Null);
    assert_eq!(created["carbon_intensity_method"], Value::Null);
    assert_eq!(
        created["lng_terms"]["scheme_certificate_number"],
        "ISCC-EU-0001"
    );
}

async fn stored_lng_terms(admin: &mut PgConnection, record: &Value) -> Value {
    sqlx::query_scalar("SELECT lng_terms FROM inventory_items WHERE id=$1")
        .bind(record["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(admin)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn bio_lng_inventory_create_keeps_the_typed_declaration() {
    let mut f = Fixture::new().await;
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let lng = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["qualification_profile"] == "lng_v1")
        .unwrap();
    let vector: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/vectors/terms/private.json"
    ))
    .unwrap();
    let declaration = vector["row"]["lng_terms"].clone();
    let mut body = f.body();
    body["product_name"] = lng["name"].clone();
    body["fuel_type"] = lng["fuel_type"].clone();
    body["lng_terms"] = declaration.clone();
    let (status, created) = f.request("POST", "/api/inventory", body.clone()).await;
    assert_eq!(status, 200, "{created}");
    // The display fields follow the declaration; the WTT value never fills
    // the lifecycle carbon intensity.
    for (field, expected) in [
        ("certification_declared", json!(true)),
        ("certification_scheme", json!("ISCC EU")),
        ("specification_standard", json!("ISO 23306:2020")),
        ("feedstock", json!("Manure")),
        ("origin", json!("NL")),
        ("carbon_intensity_gco2_mj", Value::Null),
        ("carbon_intensity_method", Value::Null),
        ("is_certified", json!(false)),
    ] {
        assert_eq!(created[field], expected, "{field}");
    }
    assert!(created.get("certifications").is_none(), "{created}");
    assert_eq!(stored_lng_terms(&mut f.admin, &created).await, declaration);
    // Omitted optional declarations are stored as explicit nulls, the same
    // complete shape the typed Python model writes and reads.
    let mut minimal = declaration.clone();
    for key in ["feedstock_detail", "min_methane_number"] {
        minimal.as_object_mut().unwrap().remove(key);
    }
    body["lng_terms"] = minimal;
    let (status, created) = f.request("POST", "/api/inventory", body).await;
    assert_eq!(status, 200, "{created}");
    let mut complete = declaration;
    complete["feedstock_detail"] = Value::Null;
    complete["min_methane_number"] = Value::Null;
    assert_eq!(stored_lng_terms(&mut f.admin, &created).await, complete);
    assert_eq!(f.counts().await, (2, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_missing_records_carry_their_own_codes() {
    let mut f = Fixture::new().await;
    let mut body = f.body();
    body["port_id"] = json!("missing-port");
    let (status, refusal) = f.request("POST", "/api/inventory", body).await;
    assert_eq!(status, 404, "{refusal}");
    assert_eq!(refusal["detail"]["code"], "PORT_NOT_FOUND");
    for method in ["PATCH", "DELETE"] {
        let (status, refusal) = f
            .request(
                method,
                &format!("/api/inventory/{}", Uuid::new_v4()),
                json!({}),
            )
            .await;
        assert_eq!(status, 404, "{method}: {refusal}");
        assert_eq!(refusal["detail"]["code"], "INVENTORY_NOT_FOUND", "{method}");
    }
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_requests_after_a_concurrent_delete_are_not_found() {
    for method in ["DELETE", "PATCH"] {
        let mut f = Fixture::new().await;
        let id = f.create().await;
        let url = std::env::var("RUST_INVENTORY_ADMIN_DATABASE_URL").unwrap();
        let mut holder = PgConnection::connect(&url).await.unwrap();
        let mut tx = holder.begin().await.unwrap();
        sqlx::query("DELETE FROM inventory_items WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap();
        let app = f.app.clone();
        let actor = f.actor;
        let path = format!("/api/inventory/{id}");
        let task = tokio::spawn(async move {
            authorized(app, actor, method, &path, json!({"current_stock_mt":"5"})).await
        });
        wait_for_lock(&mut f.admin, "inventory_items").await;
        tx.commit().await.unwrap();
        let (status, body) = task.await.unwrap();
        assert_eq!(status, 404, "{method}: {body}");
        assert_eq!(f.counts().await, (1, 0, 0));
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_reads_lock_only_the_user_row() {
    let mut f = Fixture::new().await;
    f.create().await;
    let url = std::env::var("RUST_INVENTORY_ADMIN_DATABASE_URL").unwrap();
    let mut holder = PgConnection::connect(&url).await.unwrap();
    // An organization writer does not delay either read.
    let mut tx = holder.begin().await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
        .bind(f.organization)
        .execute(&mut *tx)
        .await
        .unwrap();
    for path in ["/api/inventory", "/api/listings/my"] {
        let (status, body) = f.request("GET", path, Value::Null).await;
        assert_eq!(status, 200, "{path}: {body}");
    }
    tx.rollback().await.unwrap();
    // A writer of the actor's own user row does: the read waits for it.
    let mut tx = holder.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(f.actor)
        .execute(&mut *tx)
        .await
        .unwrap();
    let read = authorized(f.app.clone(), f.actor, "GET", "/api/inventory", Value::Null);
    let task = tokio::spawn(read);
    wait_for_lock(&mut f.admin, "users").await;
    tx.rollback().await.unwrap();
    let (status, body) = task.await.unwrap();
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_authentication_precedes_input_validation() {
    let mut f = Fixture::new().await;
    let json_type = [("content-type", "application/json".to_owned())];
    for (method, path, body) in [
        ("POST", "/api/inventory", "{}"),
        ("PATCH", "/api/inventory/not-an-identifier", "{}"),
        ("DELETE", "/api/inventory/not-an-identifier", ""),
        ("GET", "/api/inventory?limit=0", ""),
        ("GET", "/api/listings/my?skip=-1", ""),
    ] {
        let (status, detail) =
            send(f.app.clone(), method, path, &json_type, Body::from(body)).await;
        assert_eq!(status, 401, "{method} {path}: {detail}");
    }
    // JSON syntax is read before the route authenticates.
    let (status, detail) = send(
        f.app.clone(),
        "POST",
        "/api/inventory",
        &json_type,
        Body::from("{"),
    )
    .await;
    assert_eq!(status, 422, "{detail}");
    // An authenticated actor who is not admitted is refused before the input
    // is read.
    f.set_actor("email_verified", "false").await;
    for (method, path, body) in [
        ("POST", "/api/inventory", json!({})),
        ("PATCH", "/api/inventory/not-an-identifier", json!({})),
        ("DELETE", "/api/inventory/not-an-identifier", Value::Null),
    ] {
        let (status, detail) = f.request(method, path, body).await;
        assert_eq!(status, 403, "{method} {path}: {detail}");
    }
    f.set_actor("email_verified", "true").await;
    // An admitted actor's input is checked before the supplier role.
    f.set_actor("role", "BUYER").await;
    assert_eq!(f.request("POST", "/api/inventory", json!({})).await.0, 422);
    f.set_actor("role", "SUPPLIER").await;
    assert_eq!(
        f.request("GET", "/api/inventory?limit=0", Value::Null)
            .await
            .0,
        422
    );
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_reads_ignore_the_support_context_header() {
    let f = Fixture::new().await;
    f.create().await;
    let headers = [
        ("authorization", bearer(f.actor)),
        (SUPPORT_CONTEXT, Uuid::new_v4().to_string()),
    ];
    for path in ["/api/inventory", "/api/listings/my"] {
        let (status, body) = send(f.app.clone(), "GET", path, &headers, Body::empty()).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert!(body.is_array(), "{path}: {body}");
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned disposable PostgreSQL"]
async fn inventory_edit_accepts_terminal_only_references() {
    let mut f = Fixture::new().await;
    for (status, trade) in [
        ("CANCELLED", None),
        ("EXPIRED", None),
        ("FILLED", Some("DECLINED")),
    ] {
        let id = f.create().await;
        f.reference(id, status, trade).await;
        let (code, body) = f
            .request(
                "PATCH",
                &format!("/api/inventory/{id}"),
                json!({"current_stock_mt":"3"}),
            )
            .await;
        assert_eq!(code, 200, "{status} {trade:?}: {body}");
    }
    assert_eq!(f.counts().await, (6, 0, 0));
}
