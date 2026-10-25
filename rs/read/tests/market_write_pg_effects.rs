#![cfg(feature = "contract-tests")]

use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use uuid::Uuid;
use verdaxis_read::market_contract::effects::{
    EffectProbe, MutationKind, run_effect_probe, run_ordered_effect_probe,
    run_expiry_effect_probe, run_terminal_phase_probe,
};

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    order: Uuid,
    actor: Uuid,
    organization: Uuid,
    target: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_EFFECTS_ADMIN_DATABASE_URL")
            .expect("explicit migrated disposable administrator URL required");
        let url = url::Url::parse(&admin_url).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(url.path().ends_with("_test"));
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert_eq!(revision, "p89_20261022_watchlist_producer");
        let evidence_type: String = sqlx::query_scalar("SELECT data_type FROM information_schema.columns WHERE table_schema='public' AND table_name='market_event_outbox' AND column_name='producer_evidence'")
            .fetch_one(&mut admin).await.unwrap();
        assert_eq!(evidence_type, "jsonb");
        sqlx::raw_sql(include_str!("fixtures/phase8_effects.sql"))
            .execute(&mut admin).await.unwrap();
        sqlx::query("SELECT effect_fixture_reset()")
            .execute(&mut admin).await.unwrap();
        let policy: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = policy["products"].as_array().unwrap().iter()
            .find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK")
            .unwrap();
        let product_id: Uuid = product["id"].as_str().unwrap().parse().unwrap();
        let point: Uuid = product["lanes"][0].as_str().unwrap().parse().unwrap();
        let organization = Uuid::new_v4();
        let actor = Uuid::new_v4();
        let order = Uuid::new_v4();
        let watchlist = Uuid::new_v4();
        let target = Uuid::new_v4();
        sqlx::query("SELECT effect_fixture_seed($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(organization).bind(actor).bind(order).bind(Uuid::new_v4())
            .bind(watchlist).bind(target).bind(product_id).bind(point)
            .bind(product["code"].as_str().unwrap())
            .bind(product["specification_standard"].as_str().unwrap_or("ISO 8217"))
            .execute(&mut admin).await.unwrap();
        let runtime_url = std::env::var("RUST_EFFECTS_DATABASE_URL").unwrap();
        let runtime = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(runtime.host_str(), Some("127.0.0.1"));
        assert_eq!(runtime.port(), url.port());
        assert_eq!(runtime.path(), url.path());
        assert_ne!(runtime.username(), url.username());
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&runtime_url)
            .await
            .unwrap();
        Self {
            pool,
            admin,
            order,
            actor,
            organization,
            target,
        }
    }

    fn probe(&self, kind: MutationKind) -> EffectProbe {
        EffectProbe {
            order_id: self.order,
            actor_id: self.actor,
            organization_id: self.organization,
            kind,
            replay: false,
            commit: true,
        }
    }

    async fn row(&mut self) -> sqlx::postgres::PgRow {
        sqlx::query("SELECT payload,producer_evidence,participant_org_ids::jsonb AS participants,stream_seq,dispatched_at FROM market_event_outbox")
            .fetch_one(&mut self.admin).await.unwrap()
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn outbox_row_carries_mutation_identity_and_before_after() {
    let mut f = Fixture::new().await;
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled))
        .await
        .unwrap();
    let row = f.row().await;
    let evidence: Value = row.get("producer_evidence");
    assert_eq!(evidence["version"], 1);
    assert!(Uuid::parse_str(evidence["mutation_id"].as_str().unwrap()).is_ok());
    assert_eq!(evidence["orders"][0]["before"]["status"], "OPEN");
    assert_eq!(evidence["orders"][0]["after"]["status"], "CANCELLED");
    assert_eq!(evidence["orders"][0]["before"]["public_admitted"], true);
    assert_eq!(evidence["orders"][0]["after"]["public_admitted"], false);
    assert_eq!(evidence["orders"][0]["transition_index"], 0);
    assert_eq!(evidence["targets"][0]["target_id"], f.target.to_string());
    assert_eq!(evidence["aggregates_before"].as_array().unwrap().len(), 2);
    assert_eq!(evidence["aggregates_after"].as_array().unwrap().len(), 2);
    assert!(evidence["captured_at"].is_string());
    assert_eq!(
        row.get::<Value, _>("participants"),
        json!([f.organization.to_string()])
    );
    assert_eq!(row.get::<Option<i64>, _>("stream_seq"), None);
    assert!(
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("dispatched_at")
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn effect_set_per_mutation_kind() {
    for (kind, action) in [
        (MutationKind::OrderCancelled, "order.cancelled"),
        (MutationKind::InventoryPublished, "inventory.published"),
        (
            MutationKind::SupportContextStarted,
            "market_support.context_started",
        ),
        (MutationKind::CommissionChanged, "commission.updated"),
    ] {
        let mut f = Fixture::new().await;
        run_effect_probe(&f.pool, f.probe(kind)).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT action FROM audit_logs")
                .fetch_one(&mut f.admin)
                .await
                .unwrap(),
            action
        );
        for table in ["notifications", "market_event_outbox"] {
            let sql = format!("SELECT count(*) FROM {table}");
            assert_eq!(
                sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
                    .fetch_one(&mut f.admin)
                    .await
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM watchlist_events")
                .fetch_one(&mut f.admin)
                .await
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn replay_appends_no_effect() {
    let mut f = Fixture::new().await;
    let mut probe = f.probe(MutationKind::OrderCancelled);
    probe.replay = true;
    run_effect_probe(&f.pool, probe).await.unwrap();
    for table in [
        "audit_logs",
        "notifications",
        "market_event_outbox",
        "watchlist_events",
        "live_slice_benchmarks",
    ] {
        let sql = format!("SELECT count(*) FROM {table}");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
                .fetch_one(&mut f.admin)
                .await
                .unwrap(),
            0
        );
    }
    let key = Uuid::new_v4().to_string();
    let first = run_ordered_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled), key.clone())
        .await.unwrap();
    assert!(!first.replayed);
    assert_eq!(first.response_status, 204);
    let committed = effect_state(&mut f.admin).await;
    assert_eq!(committed["audit_logs"].as_array().unwrap().len(), 1);
    assert_eq!(committed["notifications"].as_array().unwrap().len(), 1);
    assert_eq!(committed["market_event_outbox"].as_array().unwrap().len(), 1);
    assert_eq!(committed["market_command_results"].as_array().unwrap().len(), 1);
    assert_eq!(committed["live_slice_benchmarks"].as_array().unwrap().len(), 1);
    let replay = run_ordered_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled), key)
        .await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.response_status, first.response_status);
    assert_eq!(replay.response_body, first.response_body);
    assert_eq!(effect_state(&mut f.admin).await, committed);

}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn benchmark_rebuilt_on_both_sides_after_cancel() {
    let mut f = Fixture::new().await;
    sqlx::query("SELECT effect_fixture_stale_benchmarks()")
        .execute(&mut f.admin).await.unwrap();
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled))
        .await
        .unwrap();
    let rows = sqlx::query("SELECT side,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count FROM live_slice_benchmarks ORDER BY side").fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("side"), "BID");
    assert_eq!(
        rows[0].get::<rust_decimal::Decimal, _>("benchmark_price_per_mt_usd"),
        rust_decimal::Decimal::from(90)
    );
    assert_eq!(rows[0].get::<i32, _>("order_count"), 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn outbox_row_is_invisible_before_commit() {
    let mut f = Fixture::new().await;
    let mut probe = f.probe(MutationKind::OrderCancelled);
    probe.commit = false;
    let result = run_effect_probe(&f.pool, probe).await.unwrap();
    assert_eq!(result.visible_from_writer, 1);
    assert_eq!(result.visible_from_observer, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status::text FROM orderbook_orders WHERE id=$1")
            .bind(f.order)
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        "OPEN"
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn transport_payload_excludes_internal_subscription_evidence() {
    let mut f = Fixture::new().await;
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled))
        .await
        .unwrap();
    let row = f.row().await;
    let payload: Value = row.get("payload");
    assert_eq!(
        payload,
        json!({"schema_version":1,"order_id":f.order.to_string(),"status":"CANCELLED","_public_market_invalidation":true})
    );
    let encoded = payload.to_string();
    for private in [
        f.target.to_string(),
        "targets".into(),
        "watchlist_id".into(),
        "observer_id".into(),
        "producer_evidence".into(),
        "mutation_id".into(),
    ] {
        assert!(!encoded.contains(&private), "transport contains {private}");
    }
    let transport: Value = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'event_type',event_type,'aggregate_type',aggregate_type,'aggregate_id',aggregate_id,'participant_org_ids',participant_org_ids,'payload',payload,'created_at',created_at,'stream_seq',stream_seq,'dispatched_at',dispatched_at) FROM market_event_outbox")
        .fetch_one(&mut f.admin).await.unwrap();
    let evidence: Value = row.get("producer_evidence");
    assert_eq!(evidence["targets"][0]["target_id"], f.target.to_string());
    let watchlist = evidence["targets"][0]["watchlist_id"].as_str().unwrap();
    let transport_text = transport.to_string();
    for private in [f.target.to_string(), watchlist.to_owned(), f.actor.to_string(),
        "targets".into(), "watchlist_id".into(), "observer_id".into(),
        "producer_evidence".into(), "mutation_id".into()] {
        assert!(!transport_text.contains(&private), "transport columns contain {private}");
    }

}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn economic_writer_updates_pin_display_before_commit() {
    let mut f = Fixture::new().await;
    let mut probe = f.probe(MutationKind::OrderCancelled);
    probe.commit = false;
    let result = run_effect_probe(&f.pool, probe).await.unwrap();
    assert_eq!(result.writer_pin_status, "CANCELLED");
    assert_eq!(result.observer_pin_status, "OPEN");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT snapshot_status FROM watchlist_targets WHERE id=$1"
        )
        .bind(f.target)
        .fetch_one(&mut f.admin)
        .await
        .unwrap(),
        "OPEN"
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_pause_does_not_pause_pin_display_updates() {
    let mut f = Fixture::new().await;
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled))
        .await
        .unwrap();
    let row = sqlx::query("SELECT snapshot_status,snapshot_price_per_mt_usd::numeric AS price,snapshot_remaining_quantity_mt::numeric AS remaining,snapshot_counterparty_label FROM watchlist_targets WHERE id=$1").bind(f.target).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String, _>("snapshot_status"), "CANCELLED");
    assert_eq!(
        row.get::<rust_decimal::Decimal, _>("price"),
        rust_decimal::Decimal::from(100)
    );
    assert_eq!(
        row.get::<rust_decimal::Decimal, _>("remaining"),
        rust_decimal::Decimal::from(10)
    );
    assert_eq!(
        row.get::<Option<String>, _>("snapshot_counterparty_label"),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM watchlist_events")
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        0
    );
}


async fn effect_state(conn: &mut PgConnection) -> Value {
    let mut state = serde_json::Map::new();
    for table in ["audit_logs", "notifications", "market_event_outbox", "market_command_results",
        "live_slice_benchmarks", "watchlist_targets", "watchlist_events"] {
        let statement = format!("SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]'::jsonb) FROM {table} t");
        let rows: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
            .fetch_one(&mut *conn).await.unwrap();
        state.insert(table.into(), rows);
    }
    Value::Object(state)
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn effect_matrix_preserves_each_mutation_contract() {
    for (kind, action, event_type, resource_type, status) in [
        (MutationKind::OrderCancelled,"order.cancelled","order_cancelled","order","CANCELLED"),
        (MutationKind::InventoryCreated,"inventory.created","inventory_created","inventory","created"),
        (MutationKind::InventoryUpdated,"inventory.updated","inventory_updated","inventory","updated"),
        (MutationKind::InventoryDeleted,"inventory.deleted","inventory_deleted","inventory","deleted"),
        (MutationKind::InventoryPublished,"inventory.published","inventory_published","inventory","published"),
        (MutationKind::SupportCapabilityGranted,"market_support.capability_granted","capability_granted","capability","ACTIVE"),
        (MutationKind::SupportCapabilityRevoked,"market_support.capability_revoked","capability_revoked","capability","REVOKED"),
        (MutationKind::SupportContextStarted,"market_support.context_started","context_started","context","ACTIVE"),
        (MutationKind::SupportContextExited,"market_support.context_exited","context_exited","context","EXITED"),
        (MutationKind::SupportContextRevoked,"market_support.context_revoked","context_revoked","context","REVOKED"),
        (MutationKind::SupportAuthorizationCreated,"market_support.authorization_created","authorization_created","authorization","ACTIVE"),
        (MutationKind::SupportAuthorizationRevoked,"market_support.authorization_revoked","authorization_revoked","authorization","REVOKED"),
        (MutationKind::CommissionChanged,"commission.updated","commission_updated","commission","PAID"),
    ] {
        let mut f = Fixture::new().await;
        let initial = effect_state(&mut f.admin).await;
        run_effect_probe(&f.pool, f.probe(kind)).await.unwrap();
        let final_state = effect_state(&mut f.admin).await;
        for table in ["audit_logs","notifications","market_event_outbox"] {
            assert_eq!(final_state[table].as_array().unwrap().len(), 1, "{action}: {table}");
        }
        let audit = &final_state["audit_logs"][0];
        assert_eq!(audit["action"], action);
        assert_eq!(audit["user_id"], f.actor.to_string());
        assert_eq!(audit["resource_type"], resource_type);
        assert_eq!(audit["resource_id"], f.order.to_string());
        let notice = &final_state["notifications"][0];
        assert_eq!(notice["recipient_id"], f.actor.to_string());
        assert_eq!(notice["type"], "ORDER_UPDATE");
        assert_eq!(notice["data"]["action"], action);
        let event = &final_state["market_event_outbox"][0];
        assert_eq!(event["event_type"], event_type);
        assert_eq!(event["aggregate_type"], resource_type);
        assert_eq!(event["aggregate_id"], f.order.to_string());
        assert_eq!(event["participant_org_ids"], json!([f.organization.to_string()]));
        assert_eq!(event["payload"]["status"], status);
        assert_eq!(event["payload"]["schema_version"], 1);
        assert_eq!(event["stream_seq"], Value::Null);
        assert_eq!(event["dispatched_at"], Value::Null);
        assert_eq!(final_state["watchlist_events"], json!([]));
        if matches!(kind, MutationKind::OrderCancelled) {
            assert_eq!(final_state["watchlist_targets"][0]["snapshot_status"], "CANCELLED");
            assert_eq!(final_state["live_slice_benchmarks"].as_array().unwrap().len(), 1);
            assert_eq!(final_state["live_slice_benchmarks"][0]["side"], "BID");
            assert_eq!(event["producer_evidence"]["orders"].as_array().unwrap().len(), 1);
        } else {
            assert_eq!(final_state["watchlist_targets"], initial["watchlist_targets"], "{action}: PIN");
            assert_eq!(final_state["live_slice_benchmarks"], initial["live_slice_benchmarks"], "{action}: benchmarks");
            assert_eq!(event["producer_evidence"]["orders"], json!([]));
            assert_eq!(event["producer_evidence"]["aggregates_after"], json!([]));
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expiry_facts_share_the_transactional_effect_path() {
    let mut f = Fixture::new().await;
    sqlx::query("SELECT effect_fixture_expiry($1)").bind(f.order)
        .execute(&mut f.admin).await.unwrap();
    let facts = run_expiry_effect_probe(&f.pool, f.probe(MutationKind::OrderExpired))
        .await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].order_id, f.order);
    assert_eq!(facts[0].previous_status, "OPEN");
    assert_eq!(facts[0].previous_version, 1);
    assert_eq!(facts[0].version, 2);
    let row = f.row().await;
    let evidence: Value = row.get("producer_evidence");
    assert_eq!(evidence["orders"][0]["before"]["version"], 1);
    assert_eq!(evidence["orders"][0]["after"]["version"], 2);
    assert_eq!(evidence["orders"][0]["before"]["status"], "OPEN");
    assert_eq!(evidence["orders"][0]["after"]["status"], "EXPIRED");
    assert_eq!(evidence["orders"][0]["before"]["remaining_quantity_mt"], "10");
    let state = effect_state(&mut f.admin).await;
    assert_eq!(state["audit_logs"][0]["action"], "order.expired");
    assert_eq!(state["notifications"].as_array().unwrap().len(), 1);
    assert_eq!(state["watchlist_targets"][0]["snapshot_status"], "EXPIRED");
    assert_eq!(state["live_slice_benchmarks"].as_array().unwrap().len(), 1);
    assert_eq!(state["live_slice_benchmarks"][0]["side"], "BID");
    assert_eq!(state["market_event_outbox"].as_array().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn target_lock_phase_refuses_later_economic_locks() {
    let f = Fixture::new().await;
    let outcomes = run_terminal_phase_probe(&f.pool, f.probe(MutationKind::OrderCancelled))
        .await.unwrap();
    assert_eq!(outcomes, vec!["MARKET_SLICE_CHANGED"; 4]);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn aggregate_evidence_separates_real_and_demo_prices_and_quantity() {
    let mut f = Fixture::new().await;
    sqlx::query("SELECT effect_fixture_mixed_slice($1)").bind(f.order)
        .execute(&mut f.admin).await.unwrap();
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled)).await.unwrap();
    let row = f.row().await;
    let evidence: Value = row.get("producer_evidence");
    let ask = evidence["aggregates_before"].as_array().unwrap().iter()
        .find(|a| a["side"] == "ASK").unwrap();
    assert_eq!(ask["real"]["quantity_mt"], "10");
    assert_eq!(ask["real"]["order_count"], 1);
    assert_eq!(ask["real"]["best_price_per_mt_usd"], "100");
    assert_eq!(ask["demo"]["quantity_mt"], "20");
    assert_eq!(ask["demo"]["order_count"], 1);
    assert_eq!(ask["demo"]["best_price_per_mt_usd"], "80");
    assert_eq!(ask["benchmark_price_per_mt_usd"], "86.67");
    let after = evidence["aggregates_after"].as_array().unwrap().iter()
        .find(|a| a["side"] == "ASK").unwrap();
    assert_eq!(after["real"]["quantity_mt"], "0");
    assert_eq!(after["real"]["best_price_per_mt_usd"], Value::Null);
    assert_eq!(after["demo"], ask["demo"]);
    let live: rust_decimal::Decimal = sqlx::query_scalar("SELECT benchmark_price_per_mt_usd FROM live_slice_benchmarks WHERE side='ASK'")
        .fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(live, rust_decimal::Decimal::from(80));
}
