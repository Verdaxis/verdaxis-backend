#![cfg(feature = "contract-tests")]

use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use uuid::Uuid;
use verdaxis_read::market_contract::effects::{EffectProbe, MutationKind, run_effect_probe};

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
            .fetch_one(&mut admin).await.unwrap();
        assert_eq!(revision, "p89_20261022_watchlist_producer");
        let evidence_type: String = sqlx::query_scalar("SELECT data_type FROM information_schema.columns WHERE table_schema='public' AND table_name='market_event_outbox' AND column_name='producer_evidence'")
            .fetch_one(&mut admin).await.unwrap();
        assert_eq!(evidence_type, "jsonb");
        sqlx::raw_sql("TRUNCATE organizations,watchlists,market_event_outbox,live_slice_benchmarks,audit_logs CASCADE")
            .execute(&mut admin).await.unwrap();
        let policy: Value = serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = policy["products"].as_array().unwrap().iter()
            .find(|p| p["qualification_profile"] == "scalar_v1" && p["execution_mode"] == "ORDERBOOK")
            .unwrap_or_else(|| policy["products"].as_array().unwrap().iter().find(|p| p["qualification_profile"] != "fame_v1" && p["qualification_profile"] != "lng_v1" && p["execution_mode"] == "ORDERBOOK").unwrap());
        let product_id: Uuid = product["id"].as_str().unwrap().parse().unwrap();
        let point: Uuid = product["lanes"][0].as_str().unwrap().parse().unwrap();
        let organization = Uuid::new_v4();
        let actor = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES($1,'Effect fixture','FUEL_SUPPLIER','APPROVED','REAL')")
            .bind(organization).execute(&mut admin).await.unwrap();
        sqlx::query("INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id) VALUES($1,$2,'unused','SUPPLIER','APPROVED',$3,true,false,'APPROVED',$3)")
            .bind(actor).bind(format!("{actor}@example.test")).bind(organization).execute(&mut admin).await.unwrap();
        let order = Uuid::new_v4();
        for (id, side, price) in [(order,"ASK",100), (Uuid::new_v4(),"BID",90)] {
            sqlx::query("INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin) VALUES($1,$2,$3,$3,'SELF_SERVICE','REAL',$4,$5,$6,10,10,$7,'SPOT','OPEN',false,true,'ISCC EU',$8,true,10,'LIFECYCLE','residue','NL')")
                .bind(id).bind(organization).bind(actor).bind(side).bind(product_id).bind(point).bind(rust_decimal::Decimal::from(price))
                .bind(product["specification_standard"].as_str().unwrap_or("ISO 8217"))
                .execute(&mut admin).await.unwrap();
        }
        let watchlist = Uuid::new_v4();
        let target = Uuid::new_v4();
        sqlx::query("INSERT INTO watchlists(id,user_id,name,kind) VALUES($1,$2,'Effects','CUSTOM')")
            .bind(watchlist).bind(actor).execute(&mut admin).await.unwrap();
        sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code,snapshot_price_per_mt_usd,snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status) VALUES($1,$2,'PIN',$3,$4,$5,'SPOT',100,10,10,'OPEN')")
            .bind(target).bind(watchlist).bind(order).bind(product["code"].as_str().unwrap()).bind(point).execute(&mut admin).await.unwrap();
        let runtime_url = std::env::var("RUST_EFFECTS_DATABASE_URL").unwrap();
        let runtime = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(runtime.host_str(), Some("127.0.0.1"));
        assert_eq!(runtime.port(), url.port());
        assert_eq!(runtime.path(), url.path());
        assert_ne!(runtime.username(), url.username());
        let pool = PgPoolOptions::new().max_connections(4).connect(&runtime_url).await.unwrap();
        Self { pool, admin, order, actor, organization, target }
    }

    fn probe(&self, kind: MutationKind) -> EffectProbe {
        EffectProbe { order_id: self.order, actor_id: self.actor, organization_id: self.organization, kind, replay: false, commit: true }
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
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled)).await.unwrap();
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
    assert_eq!(row.get::<Value,_>("participants"), json!([f.organization.to_string()]));
    assert_eq!(row.get::<Option<i64>,_>("stream_seq"), None);
    assert!(row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("dispatched_at").is_none());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn effect_set_per_mutation_kind() {
    for (kind, action) in [(MutationKind::OrderCancelled,"order.cancelled"), (MutationKind::InventoryPublished,"inventory.published"), (MutationKind::SupportContextStarted,"market_support.context_started"), (MutationKind::CommissionChanged,"commission.updated")] {
        let mut f = Fixture::new().await;
        run_effect_probe(&f.pool, f.probe(kind)).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT action FROM audit_logs").fetch_one(&mut f.admin).await.unwrap(), action);
        for table in ["notifications","market_event_outbox"] {
            let sql = format!("SELECT count(*) FROM {table}");
            assert_eq!(sqlx::query_scalar::<_,i64>(sqlx::AssertSqlSafe(sql)).fetch_one(&mut f.admin).await.unwrap(), 1);
        }
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM watchlist_events").fetch_one(&mut f.admin).await.unwrap(), 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn replay_appends_no_effect() {
    let mut f = Fixture::new().await;
    let mut probe = f.probe(MutationKind::OrderCancelled);
    probe.replay = true;
    run_effect_probe(&f.pool, probe).await.unwrap();
    for table in ["audit_logs","notifications","market_event_outbox","watchlist_events","live_slice_benchmarks"] {
        let sql = format!("SELECT count(*) FROM {table}");
        assert_eq!(sqlx::query_scalar::<_,i64>(sqlx::AssertSqlSafe(sql)).fetch_one(&mut f.admin).await.unwrap(), 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn benchmark_rebuilt_on_both_sides_after_cancel() {
    let mut f = Fixture::new().await;
    sqlx::query("INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source) SELECT gen_random_uuid(),side,wt.market_product_code,o.delivery_point_id,'SPOT',1,1,1,'live_slice_vwap' FROM orderbook_orders o CROSS JOIN watchlist_targets wt")
        .execute(&mut f.admin).await.unwrap();
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled)).await.unwrap();
    let rows = sqlx::query("SELECT side,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count FROM live_slice_benchmarks ORDER BY side").fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String,_>("side"), "BID");
    assert_eq!(rows[0].get::<rust_decimal::Decimal,_>("benchmark_price_per_mt_usd"), rust_decimal::Decimal::from(90));
    assert_eq!(rows[0].get::<i32,_>("order_count"), 1);
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
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM market_event_outbox").fetch_one(&mut f.admin).await.unwrap(), 0);
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status::text FROM orderbook_orders WHERE id=$1").bind(f.order).fetch_one(&mut f.admin).await.unwrap(), "OPEN");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn transport_payload_excludes_internal_subscription_evidence() {
    let mut f = Fixture::new().await;
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled)).await.unwrap();
    let row = f.row().await;
    let payload: Value = row.get("payload");
    assert_eq!(payload, json!({"schema_version":1,"order_id":f.order.to_string(),"status":"CANCELLED","_public_market_invalidation":true}));
    let encoded = payload.to_string();
    for private in [f.target.to_string(), "targets".into(), "watchlist_id".into(), "observer_id".into(), "producer_evidence".into(), "mutation_id".into()] {
        assert!(!encoded.contains(&private), "transport contains {private}");
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
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT snapshot_status FROM watchlist_targets WHERE id=$1").bind(f.target).fetch_one(&mut f.admin).await.unwrap(), "OPEN");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_pause_does_not_pause_pin_display_updates() {
    let mut f = Fixture::new().await;
    run_effect_probe(&f.pool, f.probe(MutationKind::OrderCancelled)).await.unwrap();
    let row = sqlx::query("SELECT snapshot_status,snapshot_price_per_mt_usd::numeric AS price,snapshot_remaining_quantity_mt::numeric AS remaining,snapshot_counterparty_label FROM watchlist_targets WHERE id=$1").bind(f.target).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String,_>("snapshot_status"), "CANCELLED");
    assert_eq!(row.get::<rust_decimal::Decimal,_>("price"), rust_decimal::Decimal::from(100));
    assert_eq!(row.get::<rust_decimal::Decimal,_>("remaining"), rust_decimal::Decimal::from(10));
    assert_eq!(row.get::<Option<String>,_>("snapshot_counterparty_label"), None);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM watchlist_events").fetch_one(&mut f.admin).await.unwrap(), 0);
}
