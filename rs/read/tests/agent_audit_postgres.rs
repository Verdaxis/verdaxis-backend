//! Run only through run_agent_audit.sh against a disposable migrated database.
#![cfg(feature = "contract-tests")]

use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::task::JoinSet;
use uuid::Uuid;
use verdaxis_read::agent::audit::{
    AuditActor, AuditEvent, AuditKind, GENESIS_HASH, IntentKind, append, append_in, record_intent,
    record_receipt, verify_chain,
};

async fn pools() -> (PgPool, PgPool) {
    let app_url =
        std::env::var("AGENT_AUDIT_APP_DATABASE_URL").expect("disposable app URL required");
    let admin_url =
        std::env::var("AGENT_AUDIT_ADMIN_DATABASE_URL").expect("disposable admin URL required");
    for url in [&app_url, &admin_url] {
        let parsed = url::Url::parse(url).unwrap();
        assert!(matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")));
        assert_eq!(parsed.path(), "/verdaxis_agent_audit_test");
    }
    let app = verdaxis_read::runtime::db::connect_contract_pool(
        app_url.parse().unwrap(),
        20,
        "UTC".into(),
    )
    .await
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .unwrap();
    sqlx::query("TRUNCATE public.agent_audit_events RESTART IDENTITY")
        .execute(&admin)
        .await
        .unwrap();
    (app, admin)
}

fn event(payload: Value) -> AuditEvent {
    AuditEvent {
        kind: AuditKind::DraftCreated,
        actor: AuditActor {
            user_id: Some(Uuid::new_v4()),
            organization_id: Some(Uuid::new_v4()),
            conversation_id: Some(Uuid::new_v4()),
        },
        payload,
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn audit_writes_use_read_write_transactions_on_read_only_sessions() {
    let (app, _) = pools().await;
    let mut tx = app.begin().await.unwrap();
    let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
    let error = append_in(&mut tx, event(json!({}))).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "audit append requires a read-write transaction"
    );
    let error = sqlx::query("INSERT INTO public.agent_audit_events(kind,payload,prev_hash,hash) VALUES('draft_created','{}',$1,$1)")
        .bind(GENESIS_HASH)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("25006")
    );
    tx.rollback().await.unwrap();

    let first = append(&app, event(json!({"standalone": true})))
        .await
        .unwrap();
    let handle = record_intent(&app, IntentKind::ToolCall, AuditActor::default(), json!({}))
        .await
        .unwrap();
    let receipt = record_receipt(&handle, json!({"ok": true})).await.unwrap();
    assert!(first.id < handle.id());
    assert!(handle.id() < receipt.id);
    let report = verify_chain(&app, None, None).await.unwrap();
    assert_eq!(report.checked, 3);
    assert_eq!(report.first_broken_id, None);
    let read_only: String = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn chain_links_and_microsecond_hashes_round_trip() {
    let (app, _) = pools().await;
    let first = append(&app, event(json!({"nested": {"z": 1, "a": "é"}})))
        .await
        .unwrap();
    let second = append(
        &app,
        event(
            serde_json::from_str(
                r#"{"numbers":[1e2,0.00,-0.0,1.2300,123456789012345678901234567890.12]}"#,
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let links: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, prev_hash, hash FROM public.agent_audit_events ORDER BY id")
            .fetch_all(&app)
            .await
            .unwrap();
    assert_eq!(
        links,
        vec![
            (first.id, GENESIS_HASH.into(), first.hash.clone()),
            (second.id, first.hash, second.hash)
        ]
    );
    let report = verify_chain(&app, None, None).await.unwrap();
    assert_eq!(report.checked, 2);
    assert_eq!(report.first_broken_id, None);
    assert_eq!(
        verify_chain(&app, Some(second.id), Some(second.id))
            .await
            .unwrap()
            .checked,
        1
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn twenty_concurrent_appends_form_one_chain() {
    let (app, _) = pools().await;
    let mut tasks = JoinSet::new();
    for index in 0..20 {
        let app = app.clone();
        tasks.spawn(async move { append(&app, event(json!({"index": index}))).await.unwrap() });
    }
    let mut ids = Vec::new();
    while let Some(result) = tasks.join_next().await {
        ids.push(result.unwrap().id);
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 20);
    let report = verify_chain(&app, None, None).await.unwrap();
    assert_eq!(report.checked, 20);
    assert_eq!(report.first_broken_id, None);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn payload_change_reports_first_changed_id() {
    let (app, admin) = pools().await;
    append(&app, event(json!({"n": 1}))).await.unwrap();
    let changed = append(&app, event(json!({"n": 2}))).await.unwrap();
    append(&app, event(json!({"n": 3}))).await.unwrap();
    sqlx::query("UPDATE public.agent_audit_events SET payload = '{\"n\":4}' WHERE id = $1")
        .bind(changed.id)
        .execute(&admin)
        .await
        .unwrap();
    let report = verify_chain(&app, None, None).await.unwrap();
    assert_eq!(report.checked, 2);
    assert_eq!(report.first_broken_id, Some(changed.id));
    assert_eq!(
        verify_chain(&app, Some(changed.id), None)
            .await
            .unwrap()
            .first_broken_id,
        Some(changed.id)
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn receipts_require_committed_intent_and_keep_reference_when_capped() {
    let (app, _) = pools().await;
    for kind in [IntentKind::ModelRequest, IntentKind::ToolCall] {
        let handle = record_intent(
            &app,
            kind,
            AuditActor::default(),
            json!({"operation": "fake"}),
        )
        .await
        .unwrap();
        let intent_id = handle.id();
        let receipt = record_receipt(&handle, json!({"text": "x".repeat(70_000)}))
            .await
            .unwrap();
        let (payload,): (Value,) =
            sqlx::query_as("SELECT payload FROM public.agent_audit_events WHERE id = $1")
                .bind(receipt.id)
                .fetch_one(&app)
                .await
                .unwrap();
        assert_eq!(payload["intent_id"], intent_id);
        assert_eq!(payload["truncated"], true);
        let retried = record_receipt(&handle, json!({"retry": true}))
            .await
            .unwrap();
        assert_eq!(retried, receipt);
    }
    assert_eq!(verify_chain(&app, None, None).await.unwrap().checked, 4);
    assert!(
        append(
            &app,
            AuditEvent {
                kind: AuditKind::ModelRequestReceipt,
                actor: AuditActor::default(),
                payload: json!({})
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        verify_chain(&app, None, None)
            .await
            .unwrap()
            .first_broken_id,
        None
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn runtime_role_can_verify_but_not_update_or_delete() {
    let (app, _) = pools().await;
    append(&app, event(json!({}))).await.unwrap();
    for statement in [
        "UPDATE public.agent_audit_events SET payload = '{}'",
        "DELETE FROM public.agent_audit_events",
    ] {
        let mut tx = app.begin_with("BEGIN READ WRITE").await.unwrap();
        let error = sqlx::query(statement).execute(&mut *tx).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
        tx.rollback().await.unwrap();
    }
    assert_eq!(verify_chain(&app, None, None).await.unwrap().checked, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn outer_rollback_leaves_no_event_and_id_gaps_are_valid() {
    let (app, admin) = pools().await;
    let mut tx = app
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await
        .unwrap();
    append_in(&mut tx, event(json!({"rolled_back": true})))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let stored = append(&app, event(json!({}))).await.unwrap();
    assert!(stored.id > 1);
    assert_eq!(verify_chain(&app, None, None).await.unwrap().checked, 1);
    assert_eq!(
        verify_chain(&app, None, None)
            .await
            .unwrap()
            .first_broken_id,
        None
    );
    let mut tx = app
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await
        .unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = append_in(&mut tx, event(json!({}))).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "audit append requires read committed isolation"
    );
    tx.rollback().await.unwrap();

    sqlx::raw_sql("CREATE FUNCTION agent_audit_refuse_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'private diagnostic'; END $$; CREATE TRIGGER agent_audit_refuse_event BEFORE INSERT ON public.agent_audit_events FOR EACH ROW EXECUTE FUNCTION agent_audit_refuse_event();")
        .execute(&admin).await.unwrap();
    let mut tx = app
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await
        .unwrap();
    let error = append_in(&mut tx, event(json!({}))).await.unwrap_err();
    assert_eq!(error.to_string(), "audit database operation failed");
    // The failed insert must roll back only the append's savepoint.
    sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
    let secret = format!("sk-{}", "x".repeat(20));
    let error = append_in(&mut tx, event(json!({secret: 1, "[REDACTED]": 2})))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "audit redacted key collision");
    sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::raw_sql("DROP TRIGGER agent_audit_refuse_event ON public.agent_audit_events; DROP FUNCTION agent_audit_refuse_event();")
        .execute(&admin).await.unwrap();
    assert_eq!(verify_chain(&app, None, None).await.unwrap().checked, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL runner"]
async fn redaction_is_stored_and_verification_crosses_batch_boundary() {
    let (app, _) = pools().await;
    let secret = format!("sk-{}", "x".repeat(20));
    let row = append(&app, event(json!({"nested": [{secret.clone(): secret}]})))
        .await
        .unwrap();
    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM public.agent_audit_events WHERE id = $1")
            .bind(row.id)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(payload, json!({"nested": [{"[REDACTED]": "[REDACTED]"}]}));
    for n in 0..256 {
        append(&app, event(json!({"n": n}))).await.unwrap();
    }
    let report = verify_chain(&app, None, None).await.unwrap();
    assert_eq!(report.checked, 257);
    assert_eq!(report.first_broken_id, None);
    assert!(verify_chain(&app, Some(2), Some(1)).await.is_err());
    assert_eq!(
        verify_chain(&app, Some(1000), None).await.unwrap().checked,
        0
    );
}
