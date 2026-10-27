//! Phase 5 Task 2b: the authenticated boundary of the compliance reads.
//!
//! Three ordinary scenarios live here: `phase5_auth_order_cases` (the accepted
//! order of authentication, support policy and typed rules), `compliance_body_cases`
//! (the body byte bound and the fuel-mix container) and
//! `phase5_transaction_cases` (the current-account lock, the scoped timeouts,
//! commit without business state and release after cancellation).
//!
//! The lane module is `rs/read/src/compliance/http.rs`. The integration owner
//! registers it through `compliance/mod.rs`; until that registration lands,
//! this test compiles the same source into the test binary and re-exports the
//! crate items the module names, so the behavior is exercised instead of being
//! asserted by file presence.

pub use verdaxis_read::{ApiError, ApiErrorKind};
pub use verdaxis_read::{auth, compliance};

#[path = "../src/compliance/http.rs"]
mod compliance_http;

use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{http::StatusCode, response::IntoResponse};
use serde_json::{Map, Value, json};
use verdaxis_read::auth::{AuthError, UserLockMode, limits::SharedLimits};

use compliance_http::{
    ComplianceBodyError, CompliancePost, FUEL_MIX_KEY_MAX_BYTES, FUEL_MIX_MAX_ENTRIES,
    OVERLAY_PATH, POST_BODY_MAX_BYTES, SCENARIO_PATH, authenticated_request, bounded_body,
    fuel_mix_container, parse_post_body,
};

const SCENARIO_TEMPLATE: &str = "/api/compliance/vessels/{vessel_id}/score";

fn post(support_present: bool) -> CompliancePost {
    CompliancePost::new(
        SCENARIO_PATH,
        "phase5-auth-order-test".into(),
        1_760_000_000,
        SharedLimits::default(),
        support_present,
    )
}

fn invalid_input(message: &str) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!({"code": "INVALID_INPUT", "message": message}),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

/// The refusals a compliance POST can meet, in the order the route applies them.
enum Refusal {
    Body(ComplianceBodyError),
    Auth(AuthError),
    Policy(ApiError),
}

impl Refusal {
    fn status(&self) -> StatusCode {
        match self {
            Self::Body(error) => error.status(),
            Self::Auth(error) => StatusCode::from_u16(error.status()).expect("auth status"),
            Self::Policy(error) => error.status,
        }
    }
}

/// The composition a compliance route uses, with the bearer/current-account
/// stage replaced by a value: parse (syntax may precede authentication), then
/// authenticate, then finish the order inside the admitted closure.
async fn admit_route(
    raw: &[u8],
    post: &CompliancePost,
    authenticate: impl Future<Output = Result<(), AuthError>>,
    validate: impl FnOnce(Value) -> Result<usize, ApiError>,
) -> Result<usize, Refusal> {
    let prepared = parse_post_body(raw).map_err(Refusal::Body)?;
    authenticate.await.map_err(Refusal::Auth)?;
    prepared
        .admit(post, Uuid::nil(), validate)
        .map_err(Refusal::Policy)
}

#[tokio::test]
async fn phase5_auth_order_cases() {
    for path in [SCENARIO_PATH, OVERLAY_PATH] {
        check_auth_order(path).await;
    }
}

async fn check_auth_order(path: &'static str) {
    let post = |support_present| {
        CompliancePost::new(
            path,
            "phase5-auth-order-test".into(),
            1_760_000_000,
            SharedLimits::default(),
            support_present,
        )
    };
    let semantic =
        json!({"vessel_id": "not-a-uuid", "fuel_mix": {"NOT_A_FUEL": "2"}, "year": 1999});
    let raw = serde_json::to_vec(&semantic).expect("scenario body");
    let calls = AtomicUsize::new(0);
    let validate = |_body: Value| {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<usize, ApiError>(invalid_input("the scenario rules refused this body"))
    };

    // A syntactically valid body with invalid semantics is a 401 while the
    // caller is unauthenticated: no typed rule runs first.
    let refused = admit_route(
        &raw,
        &post(false),
        async { Err(AuthError::Credentials) },
        &validate,
    )
    .await
    .expect_err("unauthenticated request must be refused");
    assert!(matches!(&refused, Refusal::Auth(AuthError::Credentials)));
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // A current account that the accepted policy denies is refused before the
    // typed rules, with the status the accepted auth policy assigns.
    let refused = admit_route(
        &raw,
        &post(false),
        async { Err(AuthError::Pending) },
        &validate,
    )
    .await
    .expect_err("a pending account must be refused");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // An authenticated request that carries a Market Support context cannot
    // call a compliance POST, and the refusal still precedes the typed rules.
    let refused = admit_route(&raw, &post(true), async { Ok(()) }, &validate)
        .await
        .expect_err("a support context must be refused");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let Refusal::Policy(error) = refused else {
        panic!("a support refusal is a route policy result");
    };
    let response = error.into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("error body");
    let value: Value = serde_json::from_slice(&bytes).expect("error envelope");
    assert_eq!(
        value["detail"]["code"],
        json!("MARKET_SUPPORT_MUTATION_NOT_ALLOWED")
    );

    // Authenticated, outside Market Support mode: the typed rules decide, and
    // the refusal keeps the Phase 5 input envelope.
    let refused = admit_route(&raw, &post(false), async { Ok(()) }, &validate)
        .await
        .expect_err("the scenario rules must refuse this body");
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Size and syntax may fail first; neither reaches the typed rules.
    let refused = admit_route(
        b"{not json",
        &post(false),
        async { Err(AuthError::Credentials) },
        &validate,
    )
    .await
    .expect_err("malformed JSON must be refused");
    assert!(matches!(
        &refused,
        Refusal::Body(ComplianceBodyError::InvalidSyntax)
    ));
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let oversized = vec![b'x'; POST_BODY_MAX_BYTES + 1];
    let refused = admit_route(
        &oversized,
        &post(false),
        async { Err(AuthError::Credentials) },
        &validate,
    )
    .await
    .expect_err("an oversized body must be refused");
    assert!(matches!(
        &refused,
        Refusal::Body(ComplianceBodyError::TooLarge)
    ));
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(post(false).path(), path);
    assert!(!post(false).support_present());
    assert!(post(true).support_present());
}

/// A fuel mix with `count` entries whose keys are `key_bytes` bytes each.
fn mix(count: usize, key_bytes: usize) -> Value {
    let mut entries = Map::new();
    for index in 0..count {
        let mut key = format!("F{index}");
        key.push_str(&"x".repeat(key_bytes - key.len()));
        entries.insert(key, json!("0.5"));
    }
    json!({"fuel_mix": entries})
}

/// A validator that applies the known-fuel rule after the container rule.
fn known_fuels_only(body: &Value) -> Result<usize, ApiError> {
    let entries = fuel_mix_container(body)?;
    if let Some(key) = entries
        .keys()
        .find(|key| !matches!(key.as_str(), "VLSFO" | "B24"))
    {
        return Err(invalid_input(&format!("unknown fuel {key}")));
    }
    Ok(entries.len())
}

#[test]
fn compliance_body_cases_byte_bound() {
    let padding = POST_BODY_MAX_BYTES - "{\"pad\":\"\"}".len();
    let exact = format!("{{\"pad\":\"{}\"}}", "x".repeat(padding)).into_bytes();
    assert_eq!(exact.len(), POST_BODY_MAX_BYTES);
    assert!(parse_post_body(&exact).is_ok());

    // One more byte is still one JSON document, so only the byte bound can be
    // the reason for the refusal.
    let mut oversized = exact;
    oversized.push(b' ');
    assert_eq!(oversized.len(), POST_BODY_MAX_BYTES + 1);
    assert_eq!(
        parse_post_body(&oversized).err(),
        Some(ComplianceBodyError::TooLarge)
    );
    assert_eq!(
        ComplianceBodyError::TooLarge.message(),
        format!("Request body is larger than {POST_BODY_MAX_BYTES} bytes.")
    );
    assert_eq!(ComplianceBodyError::InvalidSyntax.code(), "INVALID_INPUT");
}

#[tokio::test]
async fn compliance_body_cases_streaming_read() {
    let exact = axum::body::Body::from(vec![b'x'; POST_BODY_MAX_BYTES]);
    assert_eq!(
        bounded_body(exact)
            .await
            .expect("the bound is inclusive")
            .len(),
        POST_BODY_MAX_BYTES
    );
    let oversized = axum::body::Body::from(vec![b'x'; POST_BODY_MAX_BYTES + 1]);
    assert_eq!(
        bounded_body(oversized).await.err(),
        Some(ComplianceBodyError::TooLarge)
    );
}

#[test]
fn compliance_body_cases_mix_container() {
    // The container bound accepts 32 entries and refuses 33 before any fuel or
    // coefficient rule runs.
    assert_eq!(
        fuel_mix_container(&mix(FUEL_MIX_MAX_ENTRIES, 8))
            .expect("32 entries are inside the container bound")
            .len(),
        FUEL_MIX_MAX_ENTRIES
    );
    // Entries are names this validator does not know, so the refusal names a
    // fuel only after the container accepted all 32.
    let refused = known_fuels_only(&mix(FUEL_MIX_MAX_ENTRIES, 8)).expect_err("unknown fuels");
    let message = refused.detail["message"].as_str().expect("a message");
    assert!(
        message.starts_with("unknown fuel"),
        "the container bound must accept 32 entries: {refused:?}"
    );
    let refused =
        known_fuels_only(&mix(FUEL_MIX_MAX_ENTRIES + 1, 8)).expect_err("too many entries");
    let message = refused.detail["message"].as_str().expect("a message");
    assert!(
        message.contains("at most 32 entries"),
        "the container bound must refuse 33 entries before naming a fuel: {refused:?}"
    );
    assert!(!message.contains("unknown fuel"));

    // Each key is at most 64 UTF-8 bytes.
    assert_eq!(
        known_fuels_only(&mix(1, FUEL_MIX_KEY_MAX_BYTES))
            .expect_err("unknown fuel")
            .detail
            .get("code"),
        Some(&json!("INVALID_INPUT"))
    );
    let refused = known_fuels_only(&mix(1, FUEL_MIX_KEY_MAX_BYTES + 1)).expect_err("long key");
    assert!(
        refused.detail["message"]
            .as_str()
            .unwrap()
            .contains("at most 64 UTF-8 bytes"),
        "{refused:?}"
    );

    // A missing or non-object mix is refused by the same stage.
    assert!(fuel_mix_container(&json!({"vessel_id": "x"})).is_err());
    assert!(fuel_mix_container(&json!({"fuel_mix": ["VLSFO"]})).is_err());
    assert_eq!(
        fuel_mix_container(&json!({"fuel_mix": {"VLSFO": "1.0"}}))
            .expect("a known fuel is a container question")
            .len(),
        1
    );
}

#[test]
fn phase5_transaction_cases_account_lock() {
    for (method, route) in [
        ("GET", "/api/compliance/fleet"),
        ("GET", SCENARIO_TEMPLATE),
        ("POST", SCENARIO_PATH),
        ("POST", OVERLAY_PATH),
    ] {
        for support_present in [false, true] {
            assert_eq!(
                authenticated_request(method, route, support_present).lock_mode(),
                UserLockMode::Update,
                "{method} {route} must lock the current account in update mode"
            );
        }
    }
}

#[test]
fn phase5_transaction_cases_published_bounds() {
    let inventory: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase5/routes.json"
    ))
    .expect("the Phase 5 inventory must parse");
    let bounds = &inventory["bounds"];
    assert_eq!(
        bounds["body_bytes"].as_u64().expect("body_bytes") as usize,
        POST_BODY_MAX_BYTES
    );
    assert_eq!(
        bounds["mix_keys"].as_u64().expect("mix_keys") as usize,
        FUEL_MIX_MAX_ENTRIES
    );
    assert_eq!(
        bounds["mix_key_bytes"].as_u64().expect("mix_key_bytes") as usize,
        FUEL_MIX_KEY_MAX_BYTES
    );
    // The scoped values the authenticated read must observe before it locks
    // the current account.
    assert_eq!(bounds["actor_lock_timeout_ms"].as_u64(), Some(500));
    assert_eq!(bounds["statement_timeout_ms"].as_u64(), Some(5_000));
}

/// The scoped actor read against a disposable PostgreSQL database.
///
/// Without an interop URL the scenario returns early, like the accepted Phase 2
/// auth-state tests. `REQUIRE_AUTH_INTEROP=true` turns a missing URL into a
/// failure, so the disposable suite cannot report a silent skip.
#[tokio::test]
async fn phase5_transaction_cases_scoped_actor_read() {
    use sqlx::postgres::PgPoolOptions;
    use verdaxis_read::auth::{VerificationSettings, run_authenticated, verify_access};
    use verdaxis_read::{AppState, Config, MarketPolicy, connect};

    let Ok(url) = std::env::var("AUTH_INTEROP_APP_URL") else {
        assert!(
            std::env::var("REQUIRE_AUTH_INTEROP").is_err(),
            "auth interop URL required"
        );
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("AUTH_INTEROP_ADMIN_URL").expect("admin URL"))
        .await
        .expect("admin connection");
    let config: Config = serde_json::from_value(json!({
        "kyc_execution_gate_enabled": true,
        "environment": "test",
        "release_sha": verdaxis_read::compiled_sha(),
        "migration_revision": "kycg_20261014_staging",
        "database_url": url,
        "db_session_time_zone": "UTC",
        "prepared_budget": null,
        "bind": "127.0.0.1:0",
        "read_admission_slots": 4,
        "pool_max": 5,
        "python_budget": 52,
        "configured_max_connections": 100,
    }))
    .expect("the deployed configuration shape");
    let pools = connect(&config).await.expect("read pools");
    let state = Arc::new(
        AppState::new(
            config.clone(),
            pools.clone(),
            MarketPolicy {
                source_sha: config.release_sha.clone(),
                products: vec![],
                points: vec![],
            },
        )
        .expect("app state"),
    );

    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/auth-vectors.json"
    ))
    .expect("auth vectors");
    let settings = VerificationSettings::new(
        fixture["settings"]["current"]
            .as_str()
            .expect("secret")
            .into(),
        None,
        fixture["settings"]["issuer"]
            .as_str()
            .expect("issuer")
            .into(),
        fixture["settings"]["audience"]
            .as_str()
            .expect("audience")
            .into(),
    );
    let token = fixture["vectors"][0]["token"].as_str().expect("token");
    let access = || {
        verify_access(
            token,
            &settings,
            chrono::DateTime::from_timestamp(fixture["now"].as_i64().expect("now"), 0)
                .expect("fixture time"),
        )
        .expect("signed access token")
    };
    sqlx::query("UPDATE users SET authentication_revoked_at=NULL,password_changed_at=NULL,status='APPROVED',email_verified=false,organization_id=NULL,must_change_password=false WHERE id='20000000-0000-4000-8000-000000000001'")
        .execute(&admin)
        .await
        .expect("seed the account");
    let subject = access().subject;
    sqlx::query("UPDATE users SET id=$1 WHERE id='20000000-0000-4000-8000-000000000001'")
        .bind(subject)
        .execute(&admin)
        .await
        .expect("align the seeded account");

    sqlx::query("INSERT INTO vessels (id,name,imo_number,dwt) VALUES ('80000000-0000-4000-8000-000000000001','Boundary vessel','H000001',12345)")
        .execute(&admin)
        .await
        .expect("seed a vessel value");
    let before = business_state(&admin).await;
    let permits = state.reads.available_permits();
    let raw = serde_json::to_vec(&json!({
        "vessel_id": "generic-vessel",
        "fuel_mix": {"VLSFO": "1.0"},
        "year": 2026,
    }))
    .expect("scenario body");
    let prepared = parse_post_body(&raw).expect("scenario syntax");
    let admission = post(false);
    let checks = Arc::new(AtomicUsize::new(0));
    let checked = checks.clone();
    let result = run_authenticated(
        &state,
        access(),
        authenticated_request("POST", SCENARIO_PATH, false),
        move |tx| {
            Box::pin(async move {
                let mode: String = sqlx::query_scalar("SHOW transaction_read_only")
                    .fetch_one(&mut *tx.connection())
                    .await
                    .expect("transaction mode");
                assert_eq!(mode, "off");
                // The scoped Phase 5 actor read: the lock wait and the domain
                // deadline are set before the current-account row is locked.
                let lock_wait: String = sqlx::query_scalar("SHOW lock_timeout")
                    .fetch_one(&mut *tx.connection())
                    .await
                    .expect("lock timeout");
                assert_eq!(lock_wait, "500ms");
                let deadline: String = sqlx::query_scalar("SHOW statement_timeout")
                    .fetch_one(&mut *tx.connection())
                    .await
                    .expect("statement timeout");
                assert_eq!(deadline, "5s");
                assert_eq!(tx.actor().id, subject);
                let admitted = prepared.admit(&admission, tx.actor().id, |body| {
                    fuel_mix_container(&body)?;
                    checked.fetch_add(1, Ordering::SeqCst);
                    Ok(1)
                })?;
                assert_eq!(admitted, 1);
                Ok(())
            })
        },
    )
    .await;
    assert!(
        result.is_ok(),
        "the helper commits a successful read: {result:?}"
    );
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    assert_eq!(business_state(&admin).await, before);
    assert_eq!(
        state.reads.available_permits(),
        permits,
        "the admitted owner must return its permit"
    );

    // Current-account admission precedes both POSTs' typed rules.
    sqlx::query("UPDATE users SET status='PENDING' WHERE id=$1")
        .bind(subject)
        .execute(&admin)
        .await
        .unwrap();
    for path in [SCENARIO_PATH, OVERLAY_PATH] {
        let result = run_authenticated(
            &state,
            access(),
            authenticated_request("POST", path, false),
            |_| {
                Box::pin(async {
                    panic!("pending account reached semantic validation");
                    #[allow(unreachable_code)]
                    Ok(())
                })
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(auth::AuthenticatedError::Authentication(AuthError::Pending))
        ));
    }
    sqlx::query("UPDATE users SET status='APPROVED' WHERE id=$1")
        .bind(subject)
        .execute(&admin)
        .await
        .unwrap();

    // A cancelled client does not release the actor lock or permit early:
    // the detached owner retains both until its work and commit finish.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn({
        let state = state.clone();
        let entered = entered.clone();
        let release = release.clone();
        let admitted_access = access();
        async move {
            run_authenticated(
                &state,
                admitted_access,
                authenticated_request("POST", SCENARIO_PATH, false),
                move |_tx| {
                    Box::pin(async move {
                        entered.notify_one();
                        release.notified().await;
                        Ok(())
                    })
                },
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the cancelled request must be admitted");
    task.abort();
    assert!(
        task.await
            .expect_err("client task was cancelled")
            .is_cancelled()
    );
    assert_eq!(state.reads.available_permits(), permits - 1);
    let mut contender = admin.begin().await.expect("actor contender");
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *contender)
        .await
        .unwrap();
    let blocked = sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
        .bind(subject)
        .execute(&mut *contender)
        .await
        .expect_err("detached owner retains the exclusive actor lock");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    );
    contender.rollback().await.unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.reads.available_permits() != permits {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached owner must release the permit");
    let after_cancel = run_authenticated(
        &state,
        access(),
        authenticated_request("GET", "/api/compliance/fleet", false),
        |tx| {
            Box::pin(async move {
                sqlx::query("SELECT 1")
                    .execute(&mut *tx.connection())
                    .await
                    .expect("the account lock is released");
                Ok(())
            })
        },
    )
    .await;
    assert!(
        after_cancel.is_ok(),
        "the account lock must be released: {after_cancel:?}"
    );

    // A connection lost before the commit acknowledgement is not replayed.
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let control = admin.clone();
    let result = run_authenticated(
        &state,
        access(),
        authenticated_request("POST", OVERLAY_PATH, false),
        move |tx| {
            Box::pin(async move {
                observed.fetch_add(1, Ordering::SeqCst);
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(tx.connection())
                    .await
                    .unwrap();
                let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
                    .bind(pid)
                    .fetch_one(&control)
                    .await
                    .unwrap();
                assert!(terminated);
                Ok(())
            })
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(auth::AuthenticatedError::AmbiguousCommit)
    ));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(state.reads.available_permits(), permits);
    assert_eq!(business_state(&admin).await, before);

    pools.read.close().await;
    pools.health.close().await;
    admin.close().await;
}

/// Full row values, including changes that leave table counts unchanged.
async fn business_state(pool: &sqlx::PgPool) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object( \
         'vessels',(SELECT coalesce(jsonb_agg(to_jsonb(v) ORDER BY id),'[]'::jsonb) FROM vessels v), \
         'outbox',(SELECT coalesce(jsonb_agg(to_jsonb(o) ORDER BY id),'[]'::jsonb) FROM market_event_outbox o), \
         'ledger',(SELECT coalesce(jsonb_agg(to_jsonb(l) ORDER BY id),'[]'::jsonb) FROM compliance_ledger l))",
    )
    .fetch_one(pool)
    .await
    .expect("business, outbox and ledger values")
}
