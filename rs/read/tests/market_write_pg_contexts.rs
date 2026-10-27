#![cfg(feature = "contract-tests")]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, request::Builder},
};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    auth::VerificationSettings,
    market_support::{SupportSettings, SupportState, router},
};

struct Fixture {
    admin: PgConnection,
    pool: sqlx::PgPool,
    runtime_url: String,
    actor: Uuid,
    target: Uuid,
    organization: Uuid,
    other: Uuid,
    key: String,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_SUPPORT_ADMIN_DATABASE_URL")
            .expect("explicit disposable administrator URL");
        let runtime_url =
            std::env::var("RUST_SUPPORT_DATABASE_URL").expect("explicit disposable runtime URL");
        let admin_endpoint = url::Url::parse(&admin_url).unwrap();
        let runtime_endpoint = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_endpoint.host_str(), Some("127.0.0.1"));
        assert!(admin_endpoint.path().ends_with("_test"));
        assert_eq!(admin_endpoint.port(), runtime_endpoint.port());
        assert_eq!(admin_endpoint.path(), runtime_endpoint.path());
        assert_ne!(admin_endpoint.username(), runtime_endpoint.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_support.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_contexts.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let actor = Uuid::new_v4();
        let target = Uuid::new_v4();
        let organization = Uuid::new_v4();
        let other = Uuid::new_v4();
        sqlx::query("SELECT support_fixture_seed($1,$2,$3,$4)")
            .bind(actor)
            .bind(target)
            .bind(organization)
            .bind(other)
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&runtime_url)
            .await
            .unwrap();
        Self {
            admin,
            pool,
            runtime_url,
            actor,
            target,
            organization,
            other,
            key: Uuid::new_v4().to_string(),
        }
    }
    fn app(&self, enabled: bool, bootstrap: bool) -> Router {
        self.configured_app(enabled, bootstrap, true, 60)
    }
    fn configured_app(&self, enabled: bool, bootstrap: bool, gate: bool, ttl: i64) -> Router {
        let verification = Arc::new(VerificationSettings::new(
            self.key.clone(),
            None,
            "support-tests".into(),
            "support-tests".into(),
        ));
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let mut config: verdaxis_read::Config = serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).unwrap();
        config.kyc_execution_gate_enabled = gate;
        let bootstrap_ids = if bootstrap {
            self.actor.to_string()
        } else {
            String::new()
        };
        router(SupportState::new(
            self.pool.clone(),
            verification,
            SupportSettings::new(enabled, &bootstrap_ids)
                .with_context_ttl_minutes(ttl)
                .unwrap(),
            policy,
            config,
        ))
    }
    /// A request with a bearer token for `actor`, or with no credentials.
    fn builder(&self, actor: Option<Uuid>, method: &str, path: &str) -> Builder {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        let Some(actor) = actor else {
            return builder;
        };
        let now = Utc::now();
        let claims = json!({"sub":actor,"type":"access","iat":now.timestamp(),"exp":(now+Duration::hours(1)).timestamp(),"iss":"support-tests","aud":"support-tests"});
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(self.key.as_bytes()),
        )
        .unwrap();
        builder.header("authorization", format!("Bearer {token}"))
    }
    async fn send(&self, app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1048576)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn request(
        &self,
        app: &Router,
        actor: Uuid,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = self
            .builder(Some(actor), method, path)
            .header("x-request-id", "support-request")
            .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
            .unwrap();
        self.send(app, request).await
    }
    /// A request with an unparsed body, so validation order can be observed.
    async fn raw(
        &self,
        app: &Router,
        actor: Option<Uuid>,
        method: &str,
        path: &str,
        body: &str,
    ) -> (StatusCode, Value) {
        let request = self
            .builder(actor, method, path)
            .body(Body::from(body.to_owned()))
            .unwrap();
        self.send(app, request).await
    }
    fn grant(&self, capability: &str) -> Value {
        json!({"user_id":self.target,"capability":capability,"reason":"Support request"})
    }
    async fn grant_target(&self, app: &Router, capability: &str) {
        let (status, _) = self
            .request(
                app,
                self.actor,
                "POST",
                &format!("{ROOT}/capability-assignments"),
                Some(self.grant(capability)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    async fn user(&mut self, role: &str, status: &str, change_required: bool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("SELECT support_fixture_user($1,$2,$3,$4)")
            .bind(id)
            .bind(role)
            .bind(status)
            .bind(change_required)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }
    async fn organization(&mut self, label: &str, status: &str, provenance: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("SELECT support_fixture_organization($1,$2,$3,$4)")
            .bind(id)
            .bind(label)
            .bind(status)
            .bind(provenance)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }
    /// The newest audit row's client address and request ID for one action.
    async fn audit_context(
        &mut self,
        action: &str,
        resource: &Value,
    ) -> (Option<String>, Option<String>) {
        let row = sqlx::query("SELECT ip_address,request_id FROM audit_logs WHERE action=$1 AND resource_id=$2 ORDER BY timestamp DESC,id DESC LIMIT 1")
            .bind(action)
            .bind(resource.as_str().unwrap())
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        (row.get("ip_address"), row.get("request_id"))
    }
    async fn counts(&mut self) -> (i64, i64, i64) {
        let row = sqlx::query("SELECT (SELECT count(*) FROM audit_logs) AS audit,(SELECT count(*) FROM notifications) AS notifications,(SELECT count(*) FROM market_event_outbox) AS outbox").fetch_one(&mut self.admin).await.unwrap();
        (
            row.get("audit"),
            row.get("notifications"),
            row.get("outbox"),
        )
    }
}
const ROOT: &str = "/api/admin/market-support";

impl Fixture {
    async fn ready() -> (Self, Router) {
        let f = Self::new().await;
        let app = f.app(true, true);
        for capability in ["MARKET_SUPPORT_AUTHORIZATIONS", "MARKET_SUPPORT_LISTINGS"] {
            f.grant_target(&app, capability).await;
        }
        (f, app)
    }
    async fn start(&self, app: &Router, org: Uuid, replace: bool) -> (StatusCode, Value) {
        self.request(app, self.target, "POST", &format!("{ROOT}/contexts"), Some(json!({"organization_id":org,"support_reference":"Context case","confirm_replacement":replace}))).await
    }
    async fn seed_context(&mut self, actor: Uuid, org: Uuid, stale: bool, status: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("SELECT context_fixture_context($1,$2,$3,$4,$5)")
            .bind(id)
            .bind(actor)
            .bind(org)
            .bind(stale)
            .bind(status)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }
    async fn state(&mut self, id: Uuid) -> (String, i32, bool) {
        let row = sqlx::query("SELECT status,version,ended_at IS NOT NULL AS ended FROM market_support_contexts WHERE id=$1").bind(id).fetch_one(&mut self.admin).await.unwrap();
        (row.get("status"), row.get("version"), row.get("ended"))
    }
    async fn active_count(&mut self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM market_support_contexts WHERE actor_user_id=$1 AND status='ACTIVE'").bind(self.target).fetch_one(&mut self.admin).await.unwrap()
    }
    async fn capability_state(&mut self, capability: &str, state: &str) {
        sqlx::query("SELECT context_fixture_capability($1,$2,$3)")
            .bind(self.target)
            .bind(capability)
            .bind(state)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn context_audits(&mut self) -> Vec<Value> {
        sqlx::query_scalar("SELECT jsonb_build_object('action',action,'user_id',user_id,'resource_type',resource_type,'resource_id',resource_id,'changes',changes) FROM audit_logs WHERE action LIKE 'market_support.context_%' ORDER BY timestamp,id").fetch_all(&mut self.admin).await.unwrap()
    }
}

fn context_id(body: &Value) -> Uuid {
    body["id"].as_str().unwrap().parse().unwrap()
}
fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_create_reuses_the_same_organization_context() {
    let (mut f, app) = Fixture::ready().await;
    let before = f.counts().await;
    let (status, first) = f.start(&app, f.organization, false).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let id = context_id(&first);
    let (status, second) = f.start(&app, f.organization, false).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(first, second);
    assert_eq!(f.active_count().await, 1);
    assert_eq!(f.state(id).await, ("ACTIVE".into(), 1, false));
    let after = f.counts().await;
    assert_eq!(after, (before.0 + 1, before.1, before.2));
    let row = sqlx::query("SELECT accountable_user_id,expires_at-started_at AS unused,extract(epoch FROM expires_at-started_at)::bigint AS seconds FROM market_support_contexts WHERE id=$1").bind(id).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<Uuid, _>("accountable_user_id"), f.target);
    assert_eq!(row.get::<i64, _>("seconds"), 3600);
    let audits = f.context_audits().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["action"], "market_support.context_started");
    assert_eq!(audits[0]["user_id"], json!(f.target));
    assert_eq!(audits[0]["resource_type"], "market_support_context");
    assert_eq!(
        audits[0]["changes"]["organization_id"],
        json!(f.organization)
    );
    assert_eq!(audits[0]["changes"]["scope"], "ASSISTED_ORDER_ENTRY");
    assert_eq!(audits[0]["changes"]["support_reference"], "Context case");
    let expiry =
        chrono::DateTime::parse_from_rfc3339(audits[0]["changes"]["expires_at"].as_str().unwrap())
            .unwrap();
    assert_eq!(
        expiry,
        chrono::DateTime::parse_from_rfc3339(first["expires_at"].as_str().unwrap()).unwrap()
    );
    let (status, active) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/contexts/active"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active, first);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_replacement_requires_confirmation_and_writes_exited() {
    let (mut f, app) = Fixture::ready().await;
    let other = f.organization("Other approved", "APPROVED", "REAL").await;
    let (status, first) = f.start(&app, f.organization, false).await;
    assert_eq!(status, StatusCode::CREATED);
    let before = f.counts().await;
    let request = f
        .builder(Some(f.target), "POST", &format!("{ROOT}/contexts"))
        .body(Body::from(
            json!({"organization_id":other,"support_reference":"replace"}).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers()["x-verdaxis-market-support-context-invalid"],
        "true"
    );
    assert_eq!(
        response.headers()["x-verdaxis-market-support-context-code"],
        "MARKET_SUPPORT_CONTEXT_REPLACEMENT_REQUIRED"
    );
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1048576)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(code(&body), "MARKET_SUPPORT_CONTEXT_REPLACEMENT_REQUIRED");
    assert_eq!(f.counts().await, before);
    assert_eq!(
        f.state(context_id(&first)).await,
        ("ACTIVE".into(), 1, false)
    );
    let (status, next) = f.start(&app, other, true).await;
    assert_eq!(status, StatusCode::CREATED, "{next}");
    assert_ne!(first["id"], next["id"]);
    assert_eq!(next["organization_id"], json!(other));
    assert_eq!(
        f.state(context_id(&first)).await,
        ("EXITED".into(), 2, true)
    );
    assert_eq!(f.active_count().await, 1);
    assert_eq!(f.counts().await, (before.0 + 2, before.1, before.2));
    let audits = f.context_audits().await;
    assert_eq!(audits.len(), 3);
    let exited = audits
        .iter()
        .find(|a| a["action"] == "market_support.context_exited")
        .unwrap();
    assert_eq!(exited["resource_id"], first["id"]);
    assert_eq!(
        exited["changes"],
        json!({"status":"EXITED","replacement":true})
    );
    assert_eq!(exited["user_id"], json!(f.target));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn one_active_context_per_actor() {
    let (mut f, app) = Fixture::ready().await;
    let (status, _) = f.start(&app, f.organization, false).await;
    assert_eq!(status, StatusCode::CREATED);
    let index: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE indexname='uq_market_support_context_actor_active'",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert!(index.contains("UNIQUE"));
    assert!(index.contains("WHERE"));
    assert!(index.contains("ACTIVE"));
    let error = sqlx::query("SELECT context_fixture_context($1,$2,$3,false,'ACTIVE')")
        .bind(Uuid::new_v4())
        .bind(f.target)
        .bind(f.organization)
        .execute(&mut f.admin)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );
    assert_eq!(
        error.as_database_error().unwrap().constraint(),
        Some("uq_market_support_context_actor_active")
    );
    let other = f.organization("Other approved", "APPROVED", "REAL").await;
    assert_eq!(f.start(&app, other, true).await.0, StatusCode::CREATED);
    assert_eq!(f.active_count().await, 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn stale_context_expires_on_read_and_commits() {
    let (mut f, app) = Fixture::ready().await;
    let before = f.counts().await;
    let id = f
        .seed_context(f.target, f.organization, true, "ACTIVE")
        .await;
    let (status, body) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/contexts/active"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Value::Null);
    assert_eq!(f.state(id).await, ("EXPIRED".into(), 2, true));
    let id = f
        .seed_context(f.target, f.organization, true, "ACTIVE")
        .await;
    let path = format!("{ROOT}/contexts/{id}");
    let (status, body) = f.request(&app, f.target, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "EXPIRED");
    assert_eq!(body["version"], 2);
    assert_eq!(f.state(id).await, ("EXPIRED".into(), 2, true));
    let (_, again) = f.request(&app, f.target, "GET", &path, None).await;
    assert_eq!(again, body);
    assert_eq!(f.counts().await, before);
    let id = f
        .seed_context(f.target, f.organization, true, "ACTIVE")
        .await;
    let (status, body) = f.start(&app, f.organization, false).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(context_id(&body), id);
    assert_eq!(f.state(id).await, ("EXPIRED".into(), 2, true));
    assert_eq!(f.counts().await, (before.0 + 1, before.1, before.2));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn exit_is_versioned_and_audited() {
    let (mut f, app) = Fixture::ready().await;
    for stale in [false, true] {
        let id = f
            .seed_context(f.target, f.organization, stale, "ACTIVE")
            .await;
        let before = f.counts().await;
        let path = format!("{ROOT}/contexts/{id}/exit");
        let (status, body) = f.request(&app, f.target, "POST", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let state = if stale { "EXPIRED" } else { "EXITED" };
        assert_eq!(f.state(id).await, (state.into(), 2, true));
        assert_eq!(body["status"], state);
        assert_eq!(body["version"], 2);
        let after = f.counts().await;
        assert_eq!(after, (before.0 + 1, before.1, before.2));
        assert_eq!(
            f.request(&app, f.target, "POST", &path, None).await,
            (StatusCode::OK, body)
        );
        assert_eq!(f.counts().await, after);
        let audit = f
            .context_audits()
            .await
            .into_iter()
            .find(|a| a["resource_id"] == json!(id))
            .unwrap();
        assert_eq!(audit["action"], "market_support.context_exited");
        assert_eq!(audit["changes"], json!({"status":state}));
    }
    let id = f
        .seed_context(f.target, f.organization, false, "REVOKED")
        .await;
    let before = f.counts().await;
    assert_eq!(
        f.request(
            &app,
            f.target,
            "POST",
            &format!("{ROOT}/contexts/{id}/exit"),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.state(id).await, ("REVOKED".into(), 1, false));
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_status_enum_has_four_members() {
    let mut f = Fixture::new().await;
    let definition: String = sqlx::query_scalar("SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname='ck_market_support_context_status'").fetch_one(&mut f.admin).await.unwrap();
    for state in ["ACTIVE", "EXITED", "EXPIRED", "REVOKED"] {
        assert!(definition.contains(state));
    }
    assert!(!definition.contains("REPLACED"));
    let scope: String = sqlx::query_scalar("SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname='ck_market_support_context_scope'").fetch_one(&mut f.admin).await.unwrap();
    assert!(scope.contains("ASSISTED_ORDER_ENTRY"));
    let error = sqlx::query("SELECT context_fixture_context($1,$2,$3,false,'REPLACED')")
        .bind(Uuid::new_v4())
        .bind(f.target)
        .bind(f.organization)
        .execute(&mut f.admin)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_routes_check_account_feature_validation_and_both_capabilities() {
    let (mut f, _) = Fixture::ready().await;
    let pending = f.user("ADMIN", "PENDING", false).await;
    let forced = f.user("ADMIN", "APPROVED", true).await;
    let supplier = f.user("SUPPLIER", "APPROVED", false).await;
    let foreign = f
        .seed_context(f.actor, f.organization, false, "ACTIVE")
        .await;
    let paths = [
        (
            "GET",
            format!("{ROOT}/organizations/{}/entry", f.organization),
        ),
        ("POST", format!("{ROOT}/contexts")),
        ("GET", format!("{ROOT}/contexts/active")),
        ("GET", format!("{ROOT}/contexts/{foreign}")),
        ("POST", format!("{ROOT}/contexts/{foreign}/exit")),
        (
            "GET",
            format!("{ROOT}/organizations/{}/context", f.organization),
        ),
    ];
    let body = json!({"organization_id":f.organization,"support_reference":"case"}).to_string();
    for (method, path) in &paths {
        for enabled in [false, true] {
            let app = f.app(enabled, true);
            for (actor, expected) in [
                (None, StatusCode::UNAUTHORIZED),
                (Some(pending), StatusCode::FORBIDDEN),
                (Some(forced), StatusCode::FORBIDDEN),
                (
                    Some(supplier),
                    if enabled {
                        StatusCode::FORBIDDEN
                    } else {
                        StatusCode::NOT_FOUND
                    },
                ),
            ] {
                assert_eq!(
                    f.raw(&app, actor, method, path, &body).await.0,
                    expected,
                    "{method} {path}"
                );
            }
            if !enabled {
                let (status, body) = f.raw(&app, Some(f.target), method, path, &body).await;
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert_eq!(code(&body), "NOT_FOUND");
            }
        }
    }
    let app = f.app(true, true);
    for capability in ["MARKET_SUPPORT_LISTINGS", "MARKET_SUPPORT_AUTHORIZATIONS"] {
        for state in ["missing", "revoked", "expired"] {
            f.capability_state(capability, state).await;
            for (method, path) in &paths {
                let (status, body) = f.raw(&app, Some(f.target), method, path, &body).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{capability} {state} {path}");
                assert_eq!(code(&body), "MARKET_SUPPORT_CAPABILITY_REQUIRED");
            }
            for suffix in ["entry", "context"] {
                let (status, body) = f
                    .request(
                        &app,
                        f.target,
                        "GET",
                        &format!("{ROOT}/organizations/{}/{suffix}", Uuid::new_v4()),
                        None,
                    )
                    .await;
                assert_eq!(status, StatusCode::FORBIDDEN);
                assert_eq!(code(&body), "MARKET_SUPPORT_CAPABILITY_REQUIRED");
            }
            f.capability_state(capability, "current").await;
        }
    }
    let before = f.counts().await;
    for id in [foreign, Uuid::new_v4()] {
        for (method, suffix) in [("GET", ""), ("POST", "/exit")] {
            let (status, body) = f
                .request(
                    &app,
                    f.target,
                    method,
                    &format!("{ROOT}/contexts/{id}{suffix}"),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(code(&body), "NOT_FOUND");
        }
    }
    assert_eq!(f.state(foreign).await, ("ACTIVE".into(), 1, false));
    assert_eq!(f.counts().await, before);
    for (method, path) in [
        ("GET", format!("{ROOT}/contexts/not-a-uuid")),
        ("POST", format!("{ROOT}/contexts/not-a-uuid/exit")),
        ("GET", format!("{ROOT}/organizations/not-a-uuid/entry")),
    ] {
        assert_eq!(
            f.raw(&app, None, method, &path, "bad").await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            f.raw(&app, Some(supplier), method, &path, "bad").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.raw(&app, Some(f.target), method, &path, "bad").await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expired_capability_cannot_authorize_context_entry() {
    let (mut f, app) = Fixture::ready().await;
    f.capability_state("MARKET_SUPPORT_LISTINGS", "expired")
        .await;
    let before = f.counts().await;
    let (status, body) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations/{}/entry", f.organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "MARKET_SUPPORT_CAPABILITY_REQUIRED");
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_entry_and_composite_require_approved_real_organizations() {
    let (mut f, app) = Fixture::ready().await;
    let unknown = f.organization("Unknown", "APPROVED", "UNKNOWN").await;
    let pending = f.organization("Pending real", "PENDING", "REAL").await;
    let before = f.counts().await;
    for suffix in ["entry", "context"] {
        for (org, expected, error) in [
            (pending, StatusCode::CONFLICT, "ORGANIZATION_NOT_APPROVED"),
            (unknown, StatusCode::CONFLICT, "ORGANIZATION_NOT_REAL"),
            (
                Uuid::new_v4(),
                StatusCode::NOT_FOUND,
                "ORGANIZATION_NOT_FOUND",
            ),
        ] {
            let (status, body) = f
                .request(
                    &app,
                    f.target,
                    "GET",
                    &format!("{ROOT}/organizations/{org}/{suffix}"),
                    None,
                )
                .await;
            assert_eq!(status, expected);
            assert_eq!(code(&body), error);
            let (status, body) = f.start(&app, org, false).await;
            assert_eq!(status, expected);
            assert_eq!(code(&body), error);
        }
    }
    let (status, body) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations/{}/entry", f.organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"organization":{"id":f.organization,"name":"Support customer","domain":null,"type":"FUEL_SUPPLIER"},"eligible":true,"reason":null})
    );
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_start_validates_reference_and_uses_configured_ttl() {
    let (mut f, _) = Fixture::ready().await;
    let app = f.configured_app(true, true, true, 37);
    for reference in ["   ".to_owned(), "x".repeat(201)] {
        let (status, _) = f
            .request(
                &app,
                f.target,
                "POST",
                &format!("{ROOT}/contexts"),
                Some(json!({"organization_id":f.organization,"support_reference":reference})),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, body) = f
        .request(
            &app,
            f.target,
            "POST",
            &format!("{ROOT}/contexts"),
            Some(json!({"organization_id":f.organization,"support_reference":" ab "})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["support_reference"], "ab");
    let seconds: i64 = sqlx::query_scalar("SELECT extract(epoch FROM expires_at-started_at)::bigint FROM market_support_contexts WHERE id=$1").bind(context_id(&body)).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(seconds, 37 * 60);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_audits_carry_the_client_address_and_request_id() {
    let (mut f, app) = Fixture::ready().await;
    let mut request = f
        .builder(Some(f.target), "POST", &format!("{ROOT}/contexts"))
        .body(Body::from(
            json!({"organization_id":f.organization,"support_reference":"case"}).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "203.0.113.9:4321".parse::<SocketAddr>().unwrap(),
    ));
    let (status, body) = f.send(&app, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let (ip, request_id) = f
        .audit_context("market_support.context_started", &body["id"])
        .await;
    assert_eq!(ip.as_deref(), Some("203.0.113.9"));
    assert!(Uuid::parse_str(request_id.as_deref().unwrap()).is_ok());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn context_audit_and_projection_failures_roll_back_replacement() {
    let (mut f, app) = Fixture::ready().await;
    let (_, first) = f.start(&app, f.organization, false).await;
    let other = f.organization("Other approved", "APPROVED", "REAL").await;
    for failure in ["audit", "projection"] {
        sqlx::query("SELECT context_fixture_failure($1)")
            .bind(failure)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.counts().await;
        let (status, _) = f.start(&app, other, true).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failure}");
        assert_eq!(
            f.state(context_id(&first)).await,
            ("ACTIVE".into(), 1, false)
        );
        assert_eq!(f.active_count().await, 1);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM market_support_contexts")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(f.counts().await, before);
        sqlx::query("SELECT context_fixture_failure('none')")
            .execute(&mut f.admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn composite_is_bounded_to_one_hundred_each() {
    let (mut f, app) = Fixture::ready().await;
    sqlx::query("SELECT context_fixture_lists($1,$2,101)")
        .bind(f.target)
        .bind(f.organization)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT context_fixture_members($1,$2,101)")
        .bind(f.organization)
        .bind(f.other)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.counts().await;
    let (status, body) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations/{}/context", f.organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["authorizations"].as_array().unwrap().len(), 100);
    assert_eq!(body["listings"].as_array().unwrap().len(), 100);
    assert_eq!(body["eligible_principals"].as_array().unwrap().len(), 104);
    for field in ["authorizations", "listings"] {
        let values = body[field].as_array().unwrap();
        let times: Vec<&str> = values
            .iter()
            .map(|v| {
                if field == "listings" {
                    v["order"]["created_at"].as_str().unwrap()
                } else {
                    v["created_at"].as_str().unwrap()
                }
            })
            .collect();
        assert!(times.windows(2).all(|pair| pair[0] >= pair[1]));
    }
    let principals = body["eligible_principals"].as_array().unwrap();
    let emails: Vec<&str> = principals
        .iter()
        .map(|p| p["email"].as_str().unwrap())
        .collect();
    assert!(emails.windows(2).all(|p| p[0] < p[1]));
    let names: Vec<&str> = principals
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"First Last"));
    assert!(names.contains(&"empty@example.test"));
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn composite_uses_shared_execution_eligibility_with_gate_on_and_off() {
    let (mut f, _) = Fixture::ready().await;
    sqlx::query("SELECT context_fixture_members($1,$2,0)")
        .bind(f.organization)
        .bind(f.other)
        .execute(&mut f.admin)
        .await
        .unwrap();
    for gate in [true, false] {
        let app = f.configured_app(true, true, gate, 60);
        let (status, body) = f
            .request(
                &app,
                f.target,
                "GET",
                &format!("{ROOT}/organizations/{}/context", f.organization),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let emails: BTreeSet<&str> = body["eligible_principals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["email"].as_str().unwrap())
            .collect();
        let expected = if gate {
            vec![
                "approved@example.test",
                "empty@example.test",
                "grace@example.test",
            ]
        } else {
            vec![
                "approved@example.test",
                "empty@example.test",
                "expired@example.test",
                "grace@example.test",
                "pendingkyc@example.test",
            ]
        };
        assert_eq!(emails, expected.into_iter().collect(), "gate={gate}");
    }
}

/// Observe the actual lock wait before allowing the competing command to proceed.
async fn wait_for_blocked_transaction(observer: &mut PgConnection, blocker: i32) -> i32 {
    tokio::time::timeout(std::time::Duration::from_millis(300), async {
        loop {
            let waiting: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) ORDER BY pid LIMIT 1",
            )
            .bind(blocker)
            .fetch_optional(&mut *observer)
            .await
            .unwrap();
            if let Some(pid) = waiting {
                return pid;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("a competing transaction must be observed waiting before the barrier is released")
}

async fn race_response(app: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1048576)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_context_starts_leave_one_active_context() {
    let (mut f, app) = Fixture::ready().await;
    let other = f.organization("Other approved", "APPROVED", "REAL").await;
    let mut barrier = PgConnection::connect(&f.runtime_url).await.unwrap();
    let mut held = barrier.begin().await.unwrap();
    let barrier_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM staff_capability_assignments WHERE user_id=$1 FOR UPDATE")
        .bind(f.target)
        .fetch_all(&mut *held)
        .await
        .unwrap();
    let start_request = |organization| {
        f.builder(Some(f.target), "POST", &format!("{ROOT}/contexts"))
            .body(Body::from(json!({"organization_id":organization,"support_reference":"Context case","confirm_replacement":true}).to_string()))
            .unwrap()
    };
    let first_request = start_request(f.organization);
    let second_request = start_request(other);
    let first = tokio::spawn(race_response(app.clone(), first_request));
    // The first command holds the actor and waits for the capability barrier.
    let first_pid = wait_for_blocked_transaction(&mut f.admin, barrier_pid).await;
    let second = tokio::spawn(race_response(app.clone(), second_request));
    // The second command must wait for that first command's actor lock.
    let second_pid = wait_for_blocked_transaction(&mut f.admin, first_pid).await;
    assert_ne!(first_pid, second_pid);
    held.commit().await.unwrap();
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    for (status, body) in [&a, &b] {
        assert!(
            *status == StatusCode::CREATED
                || (*status == StatusCode::CONFLICT && code(body) == "MARKET_BUSY")
                || *status == StatusCode::SERVICE_UNAVAILABLE,
            "{status} {body}"
        );
    }
    assert!(a.0 == StatusCode::CREATED || b.0 == StatusCode::CREATED);
    assert_eq!(f.active_count().await, 1);
    let audits = f.context_audits().await;
    let starts = audits
        .iter()
        .filter(|a| a["action"] == "market_support.context_started")
        .count();
    assert_eq!(
        starts,
        usize::from(a.0 == StatusCode::CREATED) + usize::from(b.0 == StatusCode::CREATED)
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn exit_and_expiry_on_read_end_a_context_once() {
    let (mut f, app) = Fixture::ready().await;
    let id = f
        .seed_context(f.target, f.organization, true, "ACTIVE")
        .await;
    let exit = format!("{ROOT}/contexts/{id}/exit");
    let read = format!("{ROOT}/contexts/{id}");
    let mut barrier = PgConnection::connect(&f.runtime_url).await.unwrap();
    let mut held = barrier.begin().await.unwrap();
    let barrier_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM market_support_contexts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let exit_request = f.builder(Some(f.target), "POST", &exit)
        .body(Body::empty()).unwrap();
    let first = tokio::spawn(race_response(app.clone(), exit_request));
    // Exit holds the actor and capabilities before waiting for the context.
    let first_pid = wait_for_blocked_transaction(&mut f.admin, barrier_pid).await;
    let read_request = f.builder(Some(f.target), "GET", &read)
        .body(Body::empty()).unwrap();
    let second = tokio::spawn(race_response(app.clone(), read_request));
    let second_pid = wait_for_blocked_transaction(&mut f.admin, first_pid).await;
    assert_ne!(first_pid, second_pid);
    held.commit().await.unwrap();
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    assert_eq!(a.0, StatusCode::OK, "{:?}", a);
    assert_eq!(b.0, StatusCode::OK, "{:?}", b);
    assert_eq!(f.state(id).await, ("EXPIRED".into(), 2, true));
    let audits = f.context_audits().await;
    assert!(audits.len() <= 1);
    if let Some(audit) = audits.first() {
        assert_eq!(audit["action"], "market_support.context_exited");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn capability_revoke_and_context_start_serialize() {
    let (mut f, app) = Fixture::ready().await;
    let assignment: Uuid = sqlx::query_scalar("SELECT id FROM staff_capability_assignments WHERE user_id=$1 AND capability='MARKET_SUPPORT_LISTINGS'").bind(f.target).fetch_one(&mut f.admin).await.unwrap();
    let revoke_path = format!("{ROOT}/capability-assignments/{assignment}/revoke");
    // Hold the capability row: the command must already hold the actor user row.
    let mut holder = PgConnection::connect(&f.runtime_url).await.unwrap();
    let mut held = holder.begin().await.unwrap();
    sqlx::query("SELECT id FROM staff_capability_assignments WHERE id=$1 FOR UPDATE")
        .bind(assignment)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let request = f
        .builder(Some(f.target), "POST", &format!("{ROOT}/contexts"))
        .body(Body::from(
            json!({"organization_id":f.organization,"support_reference":"case"}).to_string(),
        ))
        .unwrap();
    let pending = tokio::spawn(app.clone().oneshot(request));
    let mut probe = PgConnection::connect(&f.runtime_url).await.unwrap();
    let mut observed = false;
    for _ in 0..100 {
        let mut tx = probe.begin().await.unwrap();
        let result = sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(f.target)
            .fetch_one(&mut *tx)
            .await;
        observed = result
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|e| e.code())
            .as_deref()
            == Some("55P03");
        tx.rollback().await.unwrap();
        if observed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(
        observed,
        "context start must hold its actor before capability acquisition"
    );
    let revoke_request = f
        .builder(Some(f.actor), "POST", &revoke_path)
        .body(Body::from(json!({"reason":"case revoke"}).to_string()))
        .unwrap();
    let revoked = tokio::spawn(app.clone().oneshot(revoke_request));
    held.commit().await.unwrap();
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1048576)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(revoked.await.unwrap().unwrap().status(), StatusCode::OK);
    assert_eq!(
        f.state(context_id(&body)).await,
        ("REVOKED".into(), 2, true)
    );
    assert!(
        f.context_audits()
            .await
            .iter()
            .any(|a| a["action"] == "market_support.context_revoked")
    );
    // The committed revoke is observed before another start is admitted.
    let (status, body) = f.start(&app, f.organization, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(code(&body), "MARKET_SUPPORT_CAPABILITY_REQUIRED");
}
