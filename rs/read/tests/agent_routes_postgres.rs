//! Agent HTTP contracts with runtime sessions and migrated disposable PostgreSQL.
#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request, StatusCode},
};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    AppState,
    agent::{
        audit::{self, AuditActor, AuditEvent, AuditKind},
        llm,
        routes::{RouteState, admin_routes, user_routes},
        turn::{ModelClient, ModelFuture},
    },
};

const SECRET: &str = "agent-routes-test-secret-at-least-32-characters";
const CONVERSATIONS: &str = "/api/agent/conversations";
const ADMIN: &str = "/api/auth/admin/agent";
const SAFE_REPLY: &str = "I can help with your market request.";

struct Fixture {
    app: PgPool,
    admin: PgPool,
    router: Router,
    model: Arc<TestModel>,
    buyer: Uuid,
    other_buyer: Uuid,
    supplier: Uuid,
    administrator: Uuid,
    organization: Uuid,
}

impl Fixture {
    async fn new(options: Value, blocking: bool) -> Self {
        let app_url = std::env::var("AGENT_ROUTES_APP_DATABASE_URL").expect("disposable app URL");
        let admin_url =
            std::env::var("AGENT_ROUTES_ADMIN_DATABASE_URL").expect("disposable admin URL");
        for value in [&app_url, &admin_url] {
            let url = url::Url::parse(value).unwrap();
            assert_eq!(url.host_str(), Some("127.0.0.1"));
            assert_eq!(url.path(), "/verdaxis_agent_routes_test");
        }
        let app = verdaxis_read::runtime::db::connect_contract_pool(
            app_url.parse().unwrap(),
            6,
            "UTC".into(),
        )
        .await
        .unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .unwrap();
        sqlx::raw_sql("TRUNCATE public.agent_audit_events RESTART IDENTITY; TRUNCATE public.agent_controls,public.agent_conversations,public.order_drafts,public.agent_messages,public.organizations,public.users CASCADE;")
            .execute(&admin).await.unwrap();
        let organization = Uuid::new_v4();
        sqlx::query("INSERT INTO public.organizations(id,name,type,verification_status,provenance,created_at) VALUES($1,'Agent route buyer','SHIP_OPERATOR','APPROVED','REAL',now())")
            .bind(organization).execute(&admin).await.unwrap();
        let buyer = Uuid::new_v4();
        let other_buyer = Uuid::new_v4();
        let supplier = Uuid::new_v4();
        let administrator = Uuid::new_v4();
        for (user, role) in [
            (buyer, "BUYER"),
            (other_buyer, "BUYER"),
            (supplier, "SUPPLIER"),
            (administrator, "ADMIN"),
        ] {
            sqlx::query("INSERT INTO public.users(id,email,password_hash,role,status,email_verified,organization_id,kyc_status) VALUES($1,$2,'test-hash',$3,'APPROVED',true,$4,'PENDING')")
                .bind(user).bind(format!("{user}@example.test")).bind(role)
                .bind(organization).execute(&admin).await.unwrap();
        }
        let mut agent = json!({
            "enabled":true, "model":llm::models::MODELS[0].id,
            "rate_limit_messages":100, "rate_limit_window_seconds":3600,
            "daily_token_budget":100000
        });
        for (key, value) in options.as_object().unwrap() {
            agent[key] = value.clone();
        }
        let config = serde_json::from_value(json!({
            "environment":"test", "release_sha":verdaxis_read::compiled_sha(),
            "migration_revision":verdaxis_read::runtime::artifact::ALLOWED_REVISIONS[0],
            "database_url":app_url, "db_session_time_zone":"UTC", "bind":"127.0.0.1:0",
            "read_admission_slots":6, "pool_max":7, "python_budget":0,
            "configured_max_connections":100,
            "phase2":{
                "jwt_secret":SECRET,"jwt_issuer":"agent-routes-test",
                "jwt_audience":"agent-routes-test","frontend_url":"https://app.example.test",
                "email_from":"team@example.test","resend_api_key":"test",
                "auth_domain_datasets":"null","agent":agent
            }
        }))
        .unwrap();
        let state = AppState::new(
            config,
            verdaxis_read::runtime::db::Pools {
                read: app.clone(),
                health: verdaxis_read::runtime::db::connect_contract_pool(
                    app_url.parse().unwrap(),
                    1,
                    "UTC".into(),
                )
                .await
                .unwrap(),
            },
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap(),
        )
        .unwrap();
        let model = Arc::new(TestModel {
            calls: AtomicUsize::new(0),
            blocking,
            started: Notify::new(),
            release: Notify::new(),
        });
        let route_state = RouteState {
            app: state.clone(),
            model_client: model.clone(),
        };
        let router = user_routes(route_state.clone()).merge(admin_routes(route_state));
        Self {
            app,
            admin,
            router,
            model,
            buyer,
            other_buyer,
            supplier,
            administrator,
            organization,
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        user: Option<Uuid>,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        send(&self.router, method, path, user, body).await
    }

    async fn conversation(&self, user: Uuid) -> Uuid {
        let (status, _, body) = self
            .send(Method::POST, CONVERSATIONS, Some(user), Some(json!({})))
            .await;
        assert!(status.is_success(), "{status}: {body}");
        body["id"].as_str().unwrap().parse().unwrap()
    }

    async fn message(&self, conversation: Uuid) -> (StatusCode, HeaderMap, Value) {
        self.send(
            Method::POST,
            &format!("{CONVERSATIONS}/{conversation}/messages"),
            Some(self.buyer),
            Some(json!({"text":"Help me review the market."})),
        )
        .await
    }

    async fn control(&self, scope: &str, scope_id: Option<Uuid>, paused: bool) -> Value {
        let (status, _, body) = self
            .send(
                Method::PUT,
                &format!("{ADMIN}/controls"),
                Some(self.administrator),
                Some(json!({"scope":scope,"scope_id":scope_id,"paused":paused,"reason":"Operator maintenance"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn count(&self, table: &str) -> i64 {
        let sql = match table {
            "messages" => "SELECT count(*) FROM public.agent_messages",
            "controls" => "SELECT count(*) FROM public.agent_controls",
            "control_events" => {
                "SELECT count(*) FROM public.agent_audit_events WHERE kind='control_changed'"
            }
            _ => panic!("unsupported test table"),
        };
        sqlx::query_scalar(sql).fetch_one(&self.app).await.unwrap()
    }

    async fn assert_read_only(&self) {
        let mode: String = sqlx::query_scalar("SHOW default_transaction_read_only")
            .fetch_one(&self.app)
            .await
            .unwrap();
        assert_eq!(mode, "on");
    }
}

struct TestModel {
    calls: AtomicUsize,
    blocking: bool,
    started: Notify,
    release: Notify,
}

impl ModelClient for TestModel {
    fn complete<'a>(&'a self, request: &'a llm::Request, remaining: Duration) -> ModelFuture<'a> {
        Box::pin(async move {
            assert!(!remaining.is_zero());
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.blocking {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(llm::Completion {
                content: json!({"reply":SAFE_REPLY,"citations":[]}).to_string(),
                receipts: vec![llm::Receipt {
                    slot: 1,
                    headroom_known: true,
                    model: request.model.clone(),
                    request_id: Uuid::new_v4(),
                    attempt: 1,
                    latency_ms: 1,
                    status: Some(200),
                    usage: llm::Usage {
                        prompt_tokens: Some(80),
                        completion_tokens: Some(20),
                        total_tokens: Some(100),
                    },
                    cost: Some(serde_json::Number::from(0)),
                    error: None,
                }],
            })
        })
    }
}

fn token(user: Uuid) -> String {
    let now = Utc::now().timestamp();
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({"sub":user,"type":"access","iat":now-1,"exp":now+600,
            "iss":"agent-routes-test","aud":"agent-routes-test"}),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

async fn send(
    router: &Router,
    method: Method,
    path: &str,
    user: Option<Uuid>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(user) = user {
        request = request.header("authorization", format!("Bearer {}", token(user)));
    }
    let body = match body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"response_text":String::from_utf8_lossy(&bytes)}));
    (status, headers, value)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn authentication_and_buyer_role_are_required() {
    let f = Fixture::new(json!({}), false).await;
    for (user, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(f.supplier), StatusCode::FORBIDDEN),
    ] {
        let (status, _, body) = f
            .send(Method::POST, CONVERSATIONS, user, Some(json!({})))
            .await;
        assert_eq!(status, expected, "{body}");
    }
    let conversation = f.conversation(f.buyer).await;
    let path = format!("{CONVERSATIONS}/{conversation}");
    for (user, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(f.supplier), StatusCode::FORBIDDEN),
    ] {
        for (method, suffix, body) in [
            (Method::GET, "", None),
            (Method::POST, "/messages", Some(json!({"text":"Hello"}))),
        ] {
            let (status, _, body) = f.send(method, &format!("{path}{suffix}"), user, body).await;
            assert_eq!(status, expected, "{body}");
        }
    }
    let (status, _, body) = f.send(Method::GET, &path, Some(f.buyer), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], conversation.to_string());
    let (status, _, body) = f.message(conversation).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], SAFE_REPLY);
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    f.assert_read_only().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn other_buyers_conversations_are_not_visible_or_writable() {
    let f = Fixture::new(json!({}), false).await;
    let conversation = f.conversation(f.other_buyer).await;
    for (method, suffix, body) in [
        (Method::GET, "", None),
        (Method::POST, "/messages", Some(json!({"text":"Hello"}))),
    ] {
        let (status, _, body) = f
            .send(
                method,
                &format!("{CONVERSATIONS}/{conversation}{suffix}"),
                Some(f.buyer),
                body,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count("messages").await, 0);
}

async fn pause_and_resume(scope: &str) {
    let f = Fixture::new(json!({}), false).await;
    let conversation = f.conversation(f.buyer).await;
    let scope_id = match scope {
        "global" => None,
        "organization" => Some(f.organization),
        "user" => Some(f.buyer),
        _ => unreachable!(),
    };
    f.control(scope, scope_id, true).await;
    let (status, _, body) = f.message(conversation).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(
        body,
        json!({"detail":"The assistant is paused.","reason":"Operator maintenance"})
    );
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count("messages").await, 0);
    f.control(scope, scope_id, false).await;
    let (status, _, body) = f.message(conversation).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], SAFE_REPLY);
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    f.assert_read_only().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn global_pause_blocks_dispatch_until_resumed() {
    pause_and_resume("global").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn organization_pause_blocks_dispatch_until_resumed() {
    pause_and_resume("organization").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn user_pause_blocks_dispatch_until_resumed() {
    pause_and_resume("user").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn control_changes_are_audited_and_identical_updates_are_idempotent() {
    let f = Fixture::new(json!({}), false).await;
    let first = f.control("user", Some(f.buyer), true).await;
    let repeated = f.control("user", Some(f.buyer), true).await;
    assert_eq!(first, repeated);
    assert_eq!(f.count("controls").await, 1);
    assert_eq!(f.count("control_events").await, 1);
    f.control("user", Some(f.buyer), false).await;
    assert_eq!(f.count("controls").await, 1);
    assert_eq!(f.count("control_events").await, 2);
    let rows: Vec<(Option<Uuid>, Value)> = sqlx::query_as(
        "SELECT user_id,payload FROM public.agent_audit_events WHERE kind='control_changed' ORDER BY id",
    ).fetch_all(&f.app).await.unwrap();
    assert!(rows.iter().all(|row| row.0 == Some(f.administrator)));
    assert!(
        rows.iter()
            .all(|row| row.1.to_string().contains("Operator maintenance"))
    );
    let (status, _, body) = f
        .send(
            Method::GET,
            &format!("{ADMIN}/controls"),
            Some(f.administrator),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let controls = body["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0]["paused"], false);
    let report = audit::verify_chain(&f.app, None, None).await.unwrap();
    assert_eq!(report.checked, 2);
    assert_eq!(report.first_broken_id, None);
    f.assert_read_only().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn rate_limit_counts_buyer_messages_across_conversations() {
    let f = Fixture::new(json!({"rate_limit_messages":2}), false).await;
    for _ in 0..2 {
        let conversation = f.conversation(f.buyer).await;
        let (status, _, body) = f.message(conversation).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let conversation = f.conversation(f.buyer).await;
    let (status, _, body) = f.message(conversation).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body.to_string().contains("AGENT_RATE_LIMITED"));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.count("messages").await, 4);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn concurrent_conversations_share_one_user_rate_limit() {
    let f = Fixture::new(json!({"rate_limit_messages":1}), false).await;
    let first = f.conversation(f.buyer).await;
    let second = f.conversation(f.buyer).await;
    let (left, right) = tokio::join!(f.message(first), f.message(second));
    let mut statuses = [left.0.as_u16(), right.0.as_u16()];
    statuses.sort_unstable();
    assert!(
        matches!(statuses, [200, 429] | [200, 503]),
        "left={} right={}",
        left.2,
        right.2
    );
    let (status, headers, _) = f.message(second).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.get("retry-after").is_some());
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.count("messages").await, 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn daily_budget_uses_persisted_model_receipt_usage() {
    let f = Fixture::new(json!({"daily_token_budget":100}), false).await;
    let first = f.conversation(f.buyer).await;
    assert_eq!(f.message(first).await.0, StatusCode::OK);
    let usage: Value = sqlx::query_scalar(
        "SELECT payload->'outcome'->'receipts'->0->'usage' FROM public.agent_audit_events WHERE kind='model_request_receipt'",
    ).fetch_one(&f.app).await.unwrap();
    assert_eq!(usage["prompt_tokens"], 80);
    assert_eq!(usage["completion_tokens"], 20);
    assert_eq!(usage["total_tokens"], 100);
    let second = f.conversation(f.buyer).await;
    let (status, _, body) = f.message(second).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body, json!({"detail":"Daily assistant limit reached."}));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.count("messages").await, 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn occupied_turn_capacity_returns_busy_with_retry_after() {
    let f = Fixture::new(json!({}), true).await;
    let first = f.conversation(f.buyer).await;
    let second = f.conversation(f.buyer).await;
    let router = f.router.clone();
    let buyer = f.buyer;
    let first_request = tokio::spawn(async move {
        send(
            &router,
            Method::POST,
            &format!("{CONVERSATIONS}/{first}/messages"),
            Some(buyer),
            Some(json!({"text":"Review the market."})),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), f.model.started.notified())
        .await
        .expect("first request reached the model");
    let (status, headers, body) = tokio::time::timeout(Duration::from_secs(15), f.message(second))
        .await
        .expect("busy response is bounded");
    f.model.release.notify_one();
    let first_response = first_request.await.unwrap();
    assert_eq!(first_response.0, StatusCode::OK, "{}", first_response.2);
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(headers["retry-after"], "5");
    assert!(body.to_string().contains("AGENT_BUSY"));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.count("messages").await, 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn history_exposes_only_safe_user_and_assistant_content() {
    let f = Fixture::new(json!({}), false).await;
    let conversation = f.conversation(f.buyer).await;
    assert_eq!(f.message(conversation).await.0, StatusCode::OK);
    for (seq, role, content) in [
        (3, "tool", json!({"raw":"private-tool-sentinel"})),
        (4, "system_note", json!({"error":"private-error-sentinel"})),
        (
            5,
            "assistant",
            json!({"reply":"A safe stored answer.","citations":[],"drafts":[],"question":null,"assistant_seq":5,"stopped":null,"error":"private-error-sentinel","tool_result":"private-tool-sentinel"}),
        ),
    ] {
        sqlx::query("INSERT INTO public.agent_messages(id,conversation_id,seq,role,content) VALUES($1,$2,$3,$4,$5)")
            .bind(Uuid::new_v4()).bind(conversation).bind(seq).bind(role).bind(content)
            .execute(&f.admin).await.unwrap();
    }
    let (status, _, body) = f
        .send(
            Method::GET,
            &format!("{CONVERSATIONS}/{conversation}"),
            Some(f.buyer),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .map(|row| row["seq"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 5]
    );
    assert!(
        messages
            .iter()
            .all(|row| matches!(row["role"].as_str(), Some("user" | "assistant")))
    );
    let text = body.to_string();
    assert!(text.contains(SAFE_REPLY));
    assert!(text.contains("A safe stored answer."));
    assert!(!text.contains("private-tool-sentinel"));
    assert!(!text.contains("private-error-sentinel"));
    let (status, _, body) = f
        .send(
            Method::GET,
            &format!("{CONVERSATIONS}/{conversation}?limit=1&before_seq=5"),
            Some(f.buyer),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(body["messages"][0]["seq"], 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn admin_audit_filters_and_verification_require_admin_role() {
    let f = Fixture::new(json!({}), false).await;
    for user in [f.buyer, f.other_buyer] {
        audit::append(&f.app, AuditEvent {
            kind: AuditKind::ControlChanged,
            actor: AuditActor { user_id:Some(user), organization_id:Some(f.organization), conversation_id:None },
            payload:json!({"scope":"user","scope_id":user,"paused":true,"reason":"Test maintenance"}),
        }).await.unwrap();
    }
    for path in ["/audit", "/audit/verify", "/controls"] {
        let (status, _, body) = f
            .send(Method::GET, &format!("{ADMIN}{path}"), Some(f.buyer), None)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    let (status, _, body) = f
        .send(
            Method::PUT,
            &format!("{ADMIN}/controls"),
            Some(f.buyer),
            Some(json!({"scope":"global","paused":true,"reason":"Test maintenance"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(f.count("controls").await, 0);
    let mut url = url::Url::parse("https://example.test").unwrap();
    url.set_path(&format!("{ADMIN}/audit"));
    url.query_pairs_mut()
        .append_pair("user_id", &f.buyer.to_string())
        .append_pair(
            "from",
            &(Utc::now() - ChronoDuration::minutes(1)).to_rfc3339(),
        )
        .append_pair(
            "to",
            &(Utc::now() + ChronoDuration::minutes(1)).to_rfc3339(),
        );
    let path = format!("{}?{}", url.path(), url.query().unwrap());
    let (status, _, body) = f
        .send(Method::GET, &path, Some(f.administrator), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["user_id"], f.buyer.to_string());
    url.query_pairs_mut().clear().append_pair(
        "from",
        &(Utc::now() + ChronoDuration::minutes(1)).to_rfc3339(),
    );
    let path = format!("{}?{}", url.path(), url.query().unwrap());
    let (status, _, body) = f
        .send(Method::GET, &path, Some(f.administrator), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["events"].as_array().unwrap().is_empty());
    url.query_pairs_mut().clear().append_pair(
        "to",
        &(Utc::now() - ChronoDuration::minutes(1)).to_rfc3339(),
    );
    let path = format!("{}?{}", url.path(), url.query().unwrap());
    let (status, _, body) = f
        .send(Method::GET, &path, Some(f.administrator), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["events"].as_array().unwrap().is_empty());
    let (status, _, body) = f
        .send(
            Method::GET,
            &format!("{ADMIN}/audit/verify"),
            Some(f.administrator),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["checked"], 2);
    assert_eq!(body["valid"], true);
    assert!(body["first_broken_id"].is_null());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run_agent_routes.sh"]
async fn unknown_empty_and_excessive_bodies_are_rejected_before_dispatch() {
    let f = Fixture::new(json!({}), false).await;
    let conversation = f.conversation(f.buyer).await;
    let path = format!("{CONVERSATIONS}/{conversation}/messages");
    for body in [
        json!({"text":"Hello","extra":true}),
        json!({}),
        json!({"text":""}),
        json!({"text":"   "}),
        json!({"text":"x".repeat(4001)}),
        json!({"text":"Hello","untrusted":vec![json!({"label":"attachment","text":"data"});4]}),
        json!({"text":"Hello","untrusted":[{"label":"x".repeat(61),"text":"data"}]}),
        json!({"text":"Hello","untrusted":[{"label":"attachment","text":"x".repeat(20001)}]}),
        json!({"text":"Hello","untrusted":[{"label":"attachment","text":"data","extra":true}]}),
    ] {
        let (status, _, response) = f.send(Method::POST, &path, Some(f.buyer), Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    }
    let (status, _, body) = f
        .send(
            Method::POST,
            CONVERSATIONS,
            Some(f.buyer),
            Some(json!({"extra":true})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    for body in [
        json!({"scope":"global","paused":true,"reason":"","extra":true}),
        json!({"scope":"global","paused":true,"reason":"   "}),
        json!({"scope":"global","paused":true,"reason":"x".repeat(201)}),
    ] {
        let (status, _, response) = f
            .send(
                Method::PUT,
                &format!("{ADMIN}/controls"),
                Some(f.administrator),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    }
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count("messages").await, 0);
    assert_eq!(f.count("controls").await, 0);
}
