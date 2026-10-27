#![cfg(feature = "contract-tests")]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
        let verification = Arc::new(VerificationSettings::new(
            self.key.clone(),
            None,
            "support-tests".into(),
            "support-tests".into(),
        ));
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let config = serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).unwrap();
        let bootstrap_ids = if bootstrap {
            self.actor.to_string()
        } else {
            String::new()
        };
        router(SupportState::new(
            self.pool.clone(),
            verification,
            SupportSettings::new(enabled, &bootstrap_ids),
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

fn page_ids(page: &Value) -> Vec<Value> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].clone())
        .collect()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn retired_routes_return_410_or_404() {
    let mut f = Fixture::new().await;
    let supplier = f.user("SUPPLIER", "APPROVED", false).await;
    let before = f.counts().await;
    for enabled in [false, true] {
        let app = f.app(enabled, true);
        for suffix in [
            format!("/organizations/{}/authorizations", f.organization),
            format!("/organizations/{}/listings", f.organization),
            format!(
                "/organizations/{}/listings/{}/cancel",
                f.organization,
                Uuid::new_v4()
            ),
            format!(
                "/organizations/{}/authorizations/{}/revoke",
                f.organization,
                Uuid::new_v4()
            ),
        ] {
            let path = format!("{ROOT}{suffix}");
            let retired = Some(json!({"retired":"not parsed"}));
            assert_eq!(
                f.raw(&app, None, "POST", &path, "{}").await.0,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                f.request(&app, supplier, "POST", &path, retired.clone())
                    .await
                    .0,
                if enabled {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::NOT_FOUND
                },
                "{path}"
            );
            let (status, body) = f.request(&app, f.actor, "POST", &path, retired).await;
            assert_eq!(
                status,
                if enabled {
                    StatusCode::GONE
                } else {
                    StatusCode::NOT_FOUND
                }
            );
            if enabled {
                assert_eq!(
                    body["detail"]["code"],
                    "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED"
                );
            }
        }
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn account_and_feature_checks_precede_request_validation() {
    let mut f = Fixture::new().await;
    let supplier = f.user("SUPPLIER", "APPROVED", false).await;
    let pending = f.user("ADMIN", "PENDING", false).await;
    let password_change = f.user("ADMIN", "APPROVED", true).await;
    let enabled = f.app(true, false);
    let disabled = f.app(false, false);
    let before = f.counts().await;
    for (method, path, body) in [
        (
            "POST",
            format!("{ROOT}/capability-assignments"),
            "{not json",
        ),
        (
            "POST",
            format!("{ROOT}/capability-assignments/not-a-uuid/revoke"),
            "{}",
        ),
        ("GET", format!("{ROOT}/organizations?limit=0"), ""),
        (
            "GET",
            format!("{ROOT}/organizations/not-a-uuid/listings"),
            "",
        ),
        (
            "GET",
            format!(
                "{ROOT}/organizations/{}/authorizations?skip=-1",
                f.organization
            ),
            "",
        ),
    ] {
        assert_eq!(
            f.raw(&enabled, None, method, &path, body).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
        assert_eq!(
            f.raw(&enabled, Some(supplier), method, &path, body).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
        assert_eq!(
            f.raw(&disabled, Some(supplier), method, &path, body)
                .await
                .0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        for account in [pending, password_change] {
            assert_eq!(
                f.raw(&disabled, Some(account), method, &path, body).await.0,
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        let (status, refusal) = f.raw(&disabled, Some(f.actor), method, &path, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(refusal["detail"]["code"], "NOT_FOUND", "{path}");
        let (status, refusal) = f.raw(&enabled, Some(f.actor), method, &path, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}");
        assert_eq!(refusal["detail"]["code"], "VALIDATION_FAILED", "{path}");
    }
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn audit_rows_carry_the_client_address_and_request_id() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    let grant_path = format!("{ROOT}/capability-assignments");
    // No X-Request-ID: one is generated. A loopback proxy's last hop is the client.
    let request = f
        .builder(Some(f.actor), "POST", &grant_path)
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))))
        .header("x-forwarded-for", "198.51.100.4, 203.0.113.9")
        .body(Body::from(f.grant("MARKET_SUPPORT_LISTINGS").to_string()))
        .unwrap();
    let (status, assignment) = f.send(&app, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let (ip, request_id) = f
        .audit_context("market_support.capability_granted", &assignment["id"])
        .await;
    assert_eq!(ip.as_deref(), Some("203.0.113.9"));
    assert!(
        request_id
            .as_deref()
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
    );
    // An X-Request-ID longer than 36 characters is replaced, not refused.
    // A peer that is not loopback is the client; its forwarded header is ignored.
    let supplied = format!("req-{}", Uuid::new_v4());
    let revoke_path = format!(
        "{ROOT}/capability-assignments/{}/revoke",
        assignment["id"].as_str().unwrap()
    );
    let request = f
        .builder(Some(f.actor), "POST", &revoke_path)
        .extension(ConnectInfo(SocketAddr::from(([192, 0, 2, 10], 40000))))
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-request-id", &supplied)
        .body(Body::from(json!({"reason":"Request complete"}).to_string()))
        .unwrap();
    assert_eq!(f.send(&app, request).await.0, StatusCode::OK);
    let (ip, request_id) = f
        .audit_context("market_support.capability_revoked", &assignment["id"])
        .await;
    assert_eq!(ip.as_deref(), Some("192.0.2.10"));
    let request_id = request_id.expect("a request ID is recorded");
    assert_ne!(request_id, supplied);
    assert!(Uuid::parse_str(&request_id).is_ok());
    // The serving middleware's correlation ID is reused for the audit row.
    let request = f
        .builder(Some(f.actor), "POST", &grant_path)
        .extension(String::from("middleware-id"))
        .header("x-request-id", "header-id")
        .body(Body::from(
            f.grant("MARKET_SUPPORT_AUTHORIZATIONS").to_string(),
        ))
        .unwrap();
    let (status, assignment) = f.send(&app, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let (ip, request_id) = f
        .audit_context("market_support.capability_granted", &assignment["id"])
        .await;
    assert_eq!(ip.as_deref(), Some("unknown"));
    assert_eq!(request_id.as_deref(), Some("middleware-id"));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn support_capability_lifecycle() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    let listings = format!("{ROOT}/organizations/{}/listings", f.organization);
    let (status, catalogue) = f
        .request(&app, f.target, "GET", &format!("{ROOT}/capabilities"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalogue, json!([]));
    for organization in [f.organization, Uuid::new_v4()] {
        let (status, body) = f
            .request(
                &app,
                f.target,
                "GET",
                &format!("{ROOT}/organizations/{organization}/listings"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["detail"]["code"], "MARKET_SUPPORT_CAPABILITY_REQUIRED");
    }
    let before = f.counts().await;
    let (status, assignment) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments"),
            Some(f.grant("MARKET_SUPPORT_LISTINGS")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(assignment["revoked_at"], Value::Null);
    let fields: BTreeSet<&str> = assignment
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from([
            "capability",
            "expires_at",
            "granted_at",
            "granted_by_user_id",
            "id",
            "reason",
            "revocation_reason",
            "revoked_at",
            "revoked_by_user_id",
            "user_id",
        ])
    );
    assert!(
        assignment["granted_at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );
    let id = assignment["id"].as_str().unwrap();
    let (status, _) = f.request(&app, f.target, "GET", &listings, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, revoked) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments/{id}/revoke"),
            Some(json!({"reason":"Request complete"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["revoked_by_user_id"], json!(f.actor));
    // A revoked capability no longer authorizes the resource it named.
    let (status, body) = f.request(&app, f.target, "GET", &listings, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["detail"]["code"], "MARKET_SUPPORT_CAPABILITY_REQUIRED");
    assert_eq!(
        f.request(&app, f.target, "GET", &format!("{ROOT}/capabilities"), None)
            .await
            .1,
        json!([])
    );
    let (_, renewed) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments"),
            Some(f.grant("MARKET_SUPPORT_LISTINGS")),
        )
        .await;
    assert_eq!(renewed["id"], assignment["id"]);
    assert_eq!(renewed["revoked_at"], Value::Null);
    assert_eq!(renewed["revocation_reason"], Value::Null);
    let after = f.counts().await;
    assert_eq!(after, (before.0 + 3, before.1, before.2));
    let audits = sqlx::query("SELECT action,changes,request_id FROM audit_logs WHERE resource_id=$1 ORDER BY timestamp,id").bind(id).fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(audits.len(), 3);
    assert!(audits.iter().all(|r|r.get::<Option<String>,_>("request_id").as_deref()==Some("support-request")));
    assert!(
        audits
            .iter()
            .any(|r| r.get::<String, _>("action") == "market_support.capability_revoked")
    );
    assert!(
        audits
            .iter()
            .all(|r| r.get::<Value, _>("changes")["target_user_id"] == json!(f.target))
    );
    let (status, _) = f.request(&app, f.target, "GET", &listings, None).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE staff_capability_assignments SET expires_at=now()-interval '1 second' WHERE id=$1::uuid").bind(id).execute(&mut f.admin).await.unwrap();
    // An expired capability no longer authorizes the resource it named.
    let (status, body) = f.request(&app, f.target, "GET", &listings, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["detail"]["code"], "MARKET_SUPPORT_CAPABILITY_REQUIRED");
    assert_eq!(
        f.request(&app, f.target, "GET", &format!("{ROOT}/capabilities"), None)
            .await
            .1,
        json!([])
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn bootstrap_allowlist_is_required() {
    let mut f = Fixture::new().await;
    let app = f.app(true, false);
    let before = f.counts().await;
    for user in [f.target, Uuid::new_v4()] {
        let mut body = f.grant("MARKET_SUPPORT_LISTINGS");
        body["user_id"] = json!(user);
        assert_eq!(
            f.request(
                &app,
                f.actor,
                "POST",
                &format!("{ROOT}/capability-assignments"),
                Some(body)
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments/{}/revoke", Uuid::new_v4()),
            Some(json!({"reason":"Request complete"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.counts().await, before);
    let app = f.app(true, true);
    // A target that does not exist is refused as Python refuses it.
    let mut absent = f.grant("MARKET_SUPPORT_LISTINGS");
    absent["user_id"] = json!(Uuid::new_v4());
    let (status, body) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments"),
            Some(absent),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["detail"]["code"], "MARKET_SUPPORT_TARGET_INVALID");
    sqlx::query("UPDATE users SET role='SUPPLIER' WHERE id=$1")
        .bind(f.target)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, body) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments"),
            Some(f.grant("MARKET_SUPPORT_LISTINGS")),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["detail"]["code"], "MARKET_SUPPORT_TARGET_INVALID");
    assert_eq!(f.counts().await, before);
    assert_eq!(
        f.request(&app, f.target, "GET", &format!("{ROOT}/capabilities"), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

/// Whether another session waits on a lock that `holder` holds.
async fn waits_on(observer: &mut PgConnection, holder: i32) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid<>$1 AND $1=ANY(pg_blocking_pids(pid)))")
        .bind(holder)
        .fetch_one(observer)
        .await
        .unwrap()
}

/// The SQLSTATE of an immediate row-lock attempt, or `None` when it succeeds.
async fn nowait_state(observer: &mut PgConnection, assignment: Uuid) -> Option<String> {
    sqlx::query("SELECT id FROM staff_capability_assignments WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(assignment)
        .fetch_optional(observer)
        .await
        .err()
        .and_then(|error| {
            error
                .as_database_error()
                .and_then(|failure| failure.code())
                .map(|code| code.into_owned())
        })
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn revoke_ends_contexts_in_one_transaction() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    let (_, assignment) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments"),
            Some(f.grant("MARKET_SUPPORT_LISTINGS")),
        )
        .await;
    let assignment_id: Uuid = assignment["id"].as_str().unwrap().parse().unwrap();
    let context = Uuid::new_v4();
    let unrelated = Uuid::new_v4();
    sqlx::query("SELECT support_fixture_context($1,$2,$3,$4)")
        .bind(context)
        .bind(f.target)
        .bind(f.organization)
        .bind("ACTIVE")
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT support_fixture_context($1,$2,$3,$4)")
        .bind(unrelated)
        .bind(f.actor)
        .bind(f.organization)
        .bind("ACTIVE")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.counts().await;
    let revoke_path = format!("{ROOT}/capability-assignments/{assignment_id}/revoke");
    let mut observer = PgConnection::connect(&f.runtime_url).await.unwrap();
    // A held context row makes the revocation wait inside its lock sequence.
    // While it waits, its assignment row must already be locked. The command's
    // lock wait is bounded, so a wait that a loaded host stretches past that
    // bound (MARKET_BUSY) repeats the attempt.
    let mut outcome = None;
    for _attempt in 0..3 {
        let mut holder = f.pool.begin().await.unwrap();
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM market_support_contexts WHERE id=$1 FOR UPDATE")
            .bind(context)
            .execute(&mut *holder)
            .await
            .unwrap();
        let finished = AtomicBool::new(false);
        let request = async {
            let response = f
                .request(
                    &app,
                    f.actor,
                    "POST",
                    &revoke_path,
                    Some(json!({"reason":"Request complete"})),
                )
                .await;
            finished.store(true, Ordering::SeqCst);
            response
        };
        let watch = async {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            let waited = loop {
                if waits_on(&mut observer, holder_pid).await {
                    break true;
                }
                if finished.load(Ordering::SeqCst) {
                    break false;
                }
                assert!(tokio::time::Instant::now() < deadline, "revocation stalled");
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            };
            let probe = if waited {
                let state = nowait_state(&mut observer, assignment_id).await;
                let unchanged: bool = sqlx::query_scalar(
                    "SELECT revoked_at IS NULL FROM staff_capability_assignments WHERE id=$1",
                )
                .bind(assignment_id)
                .fetch_one(&mut observer)
                .await
                .unwrap();
                assert!(unchanged);
                Some(state)
            } else {
                None
            };
            holder.rollback().await.unwrap();
            probe
        };
        let (response, probe) = tokio::join!(request, watch);
        if response.0 == StatusCode::CONFLICT && response.1["detail"]["code"] == "MARKET_BUSY" {
            continue;
        }
        outcome = Some((response, probe));
        break;
    }
    let (response, probe) = outcome.expect("the revocation completes within three attempts");
    let probe = probe.expect("the revocation waits for the held context row");
    assert_eq!(
        probe.as_deref(),
        Some("55P03"),
        "the assignment row is locked before the context rows"
    );
    assert_eq!(response.0, StatusCode::OK);
    let row = sqlx::query("SELECT status,version,ended_at IS NOT NULL AS ended FROM market_support_contexts WHERE id=$1").bind(context).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String, _>("status"), "REVOKED");
    assert_eq!(row.get::<i32, _>("version"), 2);
    assert!(row.get::<bool, _>("ended"));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM market_support_contexts WHERE id=$1")
            .bind(unrelated)
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        "ACTIVE"
    );
    assert_eq!(f.counts().await, (before.0 + 2, before.1, before.2));
    let changes: Value = sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='market_support.context_revoked' AND resource_id=$1").bind(context.to_string()).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(
        changes,
        json!({"status":"REVOKED","reason":"Request complete","capability_assignment_id":assignment_id})
    );
    let before = f.counts().await;
    assert_eq!(
        f.request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments/{assignment_id}/revoke"),
            Some(json!({"reason":"Repeat request"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.counts().await, (before.0 + 1, before.1, before.2));
    // A failed audit must roll back both the assignment and its context.
    let (_, _) = f
        .request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments"),
            Some(f.grant("MARKET_SUPPORT_LISTINGS")),
        )
        .await;
    sqlx::query(
        "UPDATE market_support_contexts SET status='ACTIVE',ended_at=NULL,version=1 WHERE id=$1",
    )
    .bind(context)
    .execute(&mut f.admin)
    .await
    .unwrap();
    sqlx::query("SELECT support_fixture_audit_failure(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.counts().await;
    assert_eq!(
        f.request(
            &app,
            f.actor,
            "POST",
            &format!("{ROOT}/capability-assignments/{assignment_id}/revoke"),
            Some(json!({"reason":"Request complete"}))
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    sqlx::query("SELECT support_fixture_audit_failure(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT revoked_at IS NULL FROM staff_capability_assignments WHERE id=$1"
        )
        .bind(assignment_id)
        .fetch_one(&mut f.admin)
        .await
        .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM market_support_contexts WHERE id=$1")
            .bind(context)
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        "ACTIVE"
    );
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn support_pages_are_bounded_and_real_only() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    for capability in ["MARKET_SUPPORT_LISTINGS", "MARKET_SUPPORT_AUTHORIZATIONS"] {
        assert_eq!(
            f.request(
                &app,
                f.actor,
                "POST",
                &format!("{ROOT}/capability-assignments"),
                Some(f.grant(capability))
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    let (status, page) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations?limit=1"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], json!(f.organization));
    assert_eq!(page["items"][0]["domain"], Value::Null);
    for suffix in ["organizations?limit=101", "organizations?skip=-1"] {
        assert_eq!(
            f.request(&app, f.target, "GET", &format!("{ROOT}/{suffix}"), None)
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for suffix in ["authorizations", "listings"] {
        assert_eq!(
            f.request(
                &app,
                f.target,
                "GET",
                &format!("{ROOT}/organizations/{}/{suffix}", f.other),
                None
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        let (status, page) = f
            .request(
                &app,
                f.target,
                "GET",
                &format!("{ROOT}/organizations/{}/{suffix}?limit=1", f.organization),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"], json!([]));
        assert_eq!(page["total"], 0);
        assert_eq!(page["limit"], 1);
    }
    sqlx::query("SELECT support_fixture_listing($1,$2)")
        .bind(f.target)
        .bind(f.organization)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, page) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations/{}/listings?limit=1", f.organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], 2);
    let listing = &page["items"][0];
    assert_eq!(listing["accountable_user_id"], json!(f.target));
    assert_eq!(listing["created_by_actor_user_id"], json!(f.target));
    assert_eq!(listing["creation_method"], "MARKET_SUPPORT");
    assert_eq!(
        listing["etag"],
        format!(
            "W/\"order-{}-v1\"",
            listing["order"]["id"].as_str().unwrap()
        )
    );
    assert!(
        listing["order"]["terms_digest"]
            .as_str()
            .is_some_and(|s| s.len() == 64)
    );
    assert!(listing["order"].get("organization_id").is_none());
    assert!(listing["order"]["quantity_mt"].is_string());
    assert!(
        listing["order"]["product_name"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    let (status, page) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!(
                "{ROOT}/organizations/{}/authorizations?limit=1",
                f.organization
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["order"]["quantity_mt"], "10.00");
    assert_eq!(page["items"][0]["support_case_reference"], Value::Null);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn organization_page_lists_only_approved_real_organizations() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    f.grant_target(&app, "MARKET_SUPPORT_LISTINGS").await;
    f.organization("Approved unknown customer", "APPROVED", "UNKNOWN")
        .await;
    f.organization("Pending real customer", "PENDING", "REAL")
        .await;
    let second = f.organization("Second customer", "APPROVED", "REAL").await;
    let (status, page) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page_ids(&page), vec![json!(second), json!(f.organization)]);
    assert_eq!(page["total"], 2);
    let (status, page) = f
        .request(
            &app,
            f.target,
            "GET",
            &format!("{ROOT}/organizations?query=unknown"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"], json!([]));
    assert_eq!(page["total"], 0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn organization_resources_refuse_organizations_that_are_not_real() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    f.grant_target(&app, "MARKET_SUPPORT_LISTINGS").await;
    f.grant_target(&app, "MARKET_SUPPORT_AUTHORIZATIONS").await;
    let unknown = f
        .organization("Approved unknown customer", "APPROVED", "UNKNOWN")
        .await;
    for suffix in ["authorizations", "listings"] {
        for (organization, status, code) in [
            (unknown, StatusCode::CONFLICT, "ORGANIZATION_NOT_REAL"),
            (f.other, StatusCode::CONFLICT, "ORGANIZATION_NOT_APPROVED"),
            (
                Uuid::new_v4(),
                StatusCode::NOT_FOUND,
                "ORGANIZATION_NOT_FOUND",
            ),
        ] {
            let (actual, body) = f
                .request(
                    &app,
                    f.target,
                    "GET",
                    &format!("{ROOT}/organizations/{organization}/{suffix}"),
                    None,
                )
                .await;
            assert_eq!(actual, status, "{suffix} {code}");
            assert_eq!(body["detail"]["code"], code, "{suffix}");
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn organization_page_applies_limit_and_skip() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    f.grant_target(&app, "MARKET_SUPPORT_LISTINGS").await;
    let second = f.organization("Second customer", "APPROVED", "REAL").await;
    for (query, ids) in [
        ("limit=1", vec![json!(second)]),
        ("limit=1&skip=1", vec![json!(f.organization)]),
        ("skip=2", vec![]),
    ] {
        let (status, page) = f
            .request(
                &app,
                f.target,
                "GET",
                &format!("{ROOT}/organizations?{query}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(page_ids(&page), ids, "{query}");
        assert_eq!(page["total"], 2, "{query}");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn authorization_items_carry_typed_terms() {
    let mut f = Fixture::new().await;
    let app = f.app(true, true);
    f.grant_target(&app, "MARKET_SUPPORT_AUTHORIZATIONS").await;
    let terms = json!({
        "side": "ASK",
        "sustainability_scheme": "ISCC_EU",
        "chain_of_custody": "MASS_BALANCE",
        "union_database_registered": true,
        "bio_share_pct": "100.00",
        "feedstock": "MANURE",
        "feedstock_detail": null,
        "production_country": "NL",
        "wtt_ghg_gco2e_mj": "-10.00",
        "specification_standard": "ISO 23306:2020",
        "min_methane_number": null,
        "delivery_method": "SHIP_TO_SHIP",
        "seller_is_physical_supplier": true,
        "pos_timing": "WITH_DELIVERY",
        "scheme_certificate_number": "ISCC-EU-0001",
        "physical_supplier_name": "Fixture Supplier"
    });
    sqlx::query("SELECT support_fixture_lng_authorization($1,$2,$3)")
        .bind(f.target)
        .bind(f.organization)
        .bind(terms.to_string())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let path = format!("{ROOT}/organizations/{}/authorizations", f.organization);
    let (status, page) = f.request(&app, f.target, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let item = &page["items"][0];
    assert_eq!(item["lng_terms"], terms);
    assert_eq!(item.get("fame_terms"), Some(&Value::Null));
    let order = &item["order"];
    assert_eq!(order["lng_terms"], terms);
    assert_eq!(order.get("fame_terms"), Some(&Value::Null));
    for field in ["support_confirmation", "port_id", "vessel_id"] {
        assert_eq!(order.get(field), Some(&Value::Null), "{field}");
    }
    let fame = json!({"side":"ASK","neat_fame":true,"uco_mass_pct":"100","standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU","certificate_reference":"fixture-certificate","certificate_holder":"Fixture Supplier","certificate_valid_until":"2027-12-31","evidence_status":"PENDING","document_references":[],"evidence_due":"BEFORE_DELIVERY"});
    sqlx::query("SELECT context_fixture_fame_authorization($1,$2,$3)")
        .bind(f.target)
        .bind(f.organization)
        .bind(fame.to_string())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, page) = f.request(&app, f.target, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let fame_item = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| !item["fame_terms"].is_null())
        .unwrap();
    assert_eq!(fame_item["fame_terms"], fame);
    assert_eq!(fame_item["order"]["fame_terms"], fame);
    // A stored declaration that the typed terms refuse is a server fault.
    sqlx::query("SELECT support_fixture_lng_authorization($1,$2,$3)")
        .bind(f.target)
        .bind(f.organization)
        .bind(json!({"side": "ASK"}).to_string())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, body) = f.request(&app, f.target, "GET", &path, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["detail"]["code"], "DATABASE_ERROR");
}
