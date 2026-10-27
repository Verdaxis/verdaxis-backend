#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;
use verdaxis_read::MarketErrorCode;
use verdaxis_read::market_contract::matching::{
    CompletionBarrier, CompletionOptions, run_completed_match, run_completed_match_paused,
};

fn now() -> DateTime<Utc> {
    "2026-10-25T12:00:00Z".parse().unwrap()
}

struct Fixture {
    admin: PgConnection,
    observer: PgConnection,
    pool: PgPool,
    aggressor: Uuid,
    resting: Vec<Uuid>,
}

impl Fixture {
    async fn new(count: i32, quantity: &str) -> Self {
        let admin_url = std::env::var("RUST_MATCHING_ADMIN_DATABASE_URL").unwrap();
        let runtime_url = std::env::var("RUST_MATCHING_DATABASE_URL").unwrap();
        let admin_target = url::Url::parse(&admin_url).unwrap();
        let runtime_target = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
        assert!(admin_target.path().ends_with("_test"));
        assert_eq!(admin_target.host_str(), runtime_target.host_str());
        assert_eq!(admin_target.port(), runtime_target.port());
        assert_eq!(admin_target.path(), runtime_target.path());
        assert_ne!(admin_target.username(), runtime_target.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/market_write_matching.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/market_write_match_completion.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let aggressor = Uuid::from_u128(1);
        sqlx::query("SELECT matching_fixture_seed($1,$2,$3)")
            .bind(aggressor)
            .bind(count)
            .bind(Decimal::from_str_exact(quantity).unwrap())
            .execute(&mut admin)
            .await
            .unwrap();
        let resting = sqlx::query_scalar(
            "SELECT id FROM orderbook_orders WHERE side='ASK' ORDER BY acceptance_ordinal,id",
        )
        .fetch_all(&mut admin)
        .await
        .unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        let pool = PgPool::connect(&runtime_url).await.unwrap();
        Self {
            admin,
            observer,
            pool,
            aggressor,
            resting,
        }
    }

    async fn execute(&self) {
        run_completed_match(
            &self.pool,
            self.aggressor,
            now(),
            CompletionOptions::default(),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn match_freezes_the_fee_basis() {
    let mut f = Fixture::new(2, "1.25").await;
    f.execute().await;
    let rows = sqlx::query("SELECT commission_plan,commission_fee_per_mt_usd,commission_rate_pct FROM trades ORDER BY id")
        .fetch_all(&mut f.observer).await.unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row.get::<Option<String>, _>(0).as_deref(), Some("free"));
        assert_eq!(row.get::<Option<Decimal>, _>(1), Some(Decimal::from(2)));
        assert_eq!(row.get::<Decimal, _>(2), Decimal::ZERO);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn match_consumes_reserved_stock_exactly() {
    let mut f = Fixture::new(2, "1.25").await;
    let item = Uuid::new_v4();
    sqlx::query("SELECT completion_fixture_back_inventory($1,$2)")
        .bind(f.resting[0])
        .bind(item)
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.execute().await;
    let row =
        sqlx::query("SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1")
            .bind(item)
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(row.get::<Decimal, _>(0), Decimal::ZERO);
    assert_eq!(row.get::<Decimal, _>(1), Decimal::ZERO);
    let remaining: Decimal =
        sqlx::query_scalar("SELECT remaining_quantity_mt FROM orderbook_orders WHERE id=$1")
            .bind(f.resting[0])
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(remaining, Decimal::ZERO);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn match_emits_one_effect_set_per_trade() {
    let mut f = Fixture::new(2, "1.25").await;
    f.execute().await;
    let trades: i64 = sqlx::query_scalar("SELECT count(*) FROM trades")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action='trade.auto_matched'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE type='TRADE_CONFIRMED'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM market_event_outbox WHERE event_type='trade_auto_matched'",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!((trades, audits, notifications, events), (2, 2, 4, 2));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn match_rebuilds_both_benchmarks_once() {
    let mut f = Fixture::new(2, "0.5").await;
    f.execute().await;
    let sides: Vec<String> =
        sqlx::query_scalar("SELECT side::text FROM live_slice_benchmarks ORDER BY side::text")
            .fetch_all(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(sides, vec!["ASK"]);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn auto_confirmed_match_advances_verified_referrals_once() {
    let mut f = Fixture::new(2, "1.25").await;
    sqlx::query("SELECT completion_fixture_referral()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.execute().await;
    let row = sqlx::query("SELECT status::text,activated_at FROM referrals")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>(0), "ACTIVE");
    assert_eq!(row.get::<Option<DateTime<Utc>>, _>(1), Some(now()));
    let counts: (i64, i64) = sqlx::query_as("SELECT count(*) FILTER (WHERE status IN ('VERIFIED','ACTIVE')),count(*) FILTER (WHERE status='ACTIVE') FROM referrals")
        .fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(counts, (1, 1));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn matched_event_payloads_have_only_the_python_fields() {
    let mut f = Fixture::new(1, "1").await;
    f.execute().await;
    let rows = sqlx::query("SELECT event_type,payload::text,participant_org_ids::text,producer_evidence::text FROM market_event_outbox ORDER BY event_type")
        .fetch_all(&mut f.observer).await.unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let kind: String = row.get(0);
        let payload: Value = serde_json::from_str(&row.get::<String, _>(1)).unwrap();
        let mut keys: Vec<_> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = if kind == "trade_auto_matched" {
            vec![
                "trade_id",
                "product_name",
                "fuel_type",
                "quantity",
                "price",
                "is_anonymous",
                "source_kind",
                "scope",
                "demo_status",
                "unknown_count",
                "product_id",
                "market_product",
                "delivery_point_id",
                "availability_window",
                "observed_at",
                "schema_version",
                "_public_market_invalidation",
            ]
        } else {
            assert_eq!(kind, "orders_matched");
            vec![
                "order_id",
                "matches",
                "source_kind",
                "scope",
                "demo_status",
                "is_demo_listing",
                "unknown_count",
                "market_product",
                "delivery_point_id",
                "availability_window",
                "observed_at",
                "delivery_point",
                "schema_version",
                "_public_market_invalidation",
            ]
        };
        expected.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(payload["_public_market_invalidation"], true);
        assert_eq!(payload["scope"], "DELIVERY_POINT");
        assert_eq!(payload["demo_status"], "REAL_ONLY");
        let participants: Vec<Uuid> = serde_json::from_str(&row.get::<String, _>(2)).unwrap();
        assert_eq!(
            participants,
            vec![Uuid::from_u128(0x1000), Uuid::from_u128(0x2000)]
        );
        let evidence: Value = serde_json::from_str(&row.get::<String, _>(3)).unwrap();
        assert!(evidence["orders"].is_array());
        assert!(payload.get("producer_evidence").is_none());
        assert!(payload.get("targets").is_none());
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn matched_snapshot_uses_triggering_order_and_nullable_demo_owners() {
    for ask_aggressor in [false, true] {
        let mut f = Fixture::new(1, "1").await;
        let trigger = if ask_aggressor {
            f.resting[0]
        } else {
            f.aggressor
        };
        run_completed_match(&f.pool, trigger, now(), CompletionOptions::default())
            .await
            .unwrap();
        let row = sqlx::query("SELECT bid_order_id,ask_order_id,buyer_user_id,seller_user_id,initiator_org_id,initiated_by,market_snapshot_version,confirmed_at,is_anonymous,product_id,delivery_point_id,availability_window FROM trades")
            .fetch_one(&mut f.observer).await.unwrap();
        assert_eq!(row.get::<Uuid, _>(0), f.aggressor);
        assert_eq!(row.get::<Uuid, _>(1), f.resting[0]);
        assert_eq!(row.get::<Option<Uuid>, _>(2), Some(Uuid::from_u128(0x1001)));
        assert_eq!(row.get::<Option<Uuid>, _>(3), Some(Uuid::from_u128(0x2001)));
        assert_eq!(
            row.get::<Uuid, _>(4),
            Uuid::from_u128(if ask_aggressor { 0x2000 } else { 0x1000 })
        );
        assert_eq!(
            row.get::<String, _>(5),
            if ask_aggressor { "SELLER" } else { "BUYER" }
        );
        assert_eq!(row.get::<i16, _>(6), 1);
        assert_eq!(row.get::<Option<DateTime<Utc>>, _>(7), Some(now()));
        assert!(row.get::<bool, _>(8));
        let identity: (Uuid,Option<Uuid>,String) = sqlx::query_as("SELECT product_id,delivery_point_id,availability_window FROM orderbook_orders WHERE id=$1")
            .bind(trigger).fetch_one(&mut f.observer).await.unwrap();
        assert_eq!(
            (
                row.get::<Uuid, _>(9),
                row.get::<Option<Uuid>, _>(10),
                row.get::<String, _>(11)
            ),
            identity
        );
    }
    let mut f = Fixture::new(0, "1").await;
    let bid = Uuid::from_u128(9001);
    let ask = Uuid::from_u128(9002);
    sqlx::query("SELECT matching_fixture_demo($1,$2,$3,$4)")
        .bind(bid)
        .bind(ask)
        .bind(Uuid::from_u128(9003))
        .bind(Uuid::from_u128(9004))
        .execute(&mut f.admin)
        .await
        .unwrap();
    let options = CompletionOptions {
        demo_pair: verdaxis_read::market_matching::DemoOrderPair::new(bid, ask),
        actor: Some(Uuid::from_u128(0x1001)),
        ..Default::default()
    };
    run_completed_match(&f.pool, bid, now(), options)
        .await
        .unwrap();
    let row:(Option<Uuid>,Option<Uuid>,String,String)=sqlx::query_as("SELECT buyer_user_id,seller_user_id,buyer_provenance::text,seller_provenance::text FROM trades").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(row, (None, None, "DEMO".into(), "DEMO".into()));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn referral_counts_and_all_effects_remain_unchanged_on_receipt_replay() {
    let mut f = Fixture::new(2, "1.25").await;
    sqlx::query("SELECT completion_fixture_referral()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let options = CompletionOptions {
        receipt_key: Some("completion-replay".into()),
        ..Default::default()
    };
    let first = run_completed_match(&f.pool, f.aggressor, now(), options.clone())
        .await
        .unwrap();
    let before:Value=sqlx::query_scalar("SELECT jsonb_build_array((SELECT count(*) FROM trades),(SELECT count(*) FROM audit_logs),(SELECT count(*) FROM notifications),(SELECT count(*) FROM market_event_outbox),(SELECT count(*) FROM referrals WHERE status IN ('VERIFIED','ACTIVE')),(SELECT count(*) FROM referrals WHERE status='ACTIVE'),(SELECT activated_at FROM referrals LIMIT 1))").fetch_one(&mut f.observer).await.unwrap();
    let replay = run_completed_match(
        &f.pool,
        f.aggressor,
        now() + chrono::Duration::hours(1),
        options,
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert!(!first.replayed);
    assert_eq!(first.trade_ids, replay.trade_ids);
    let after:Value=sqlx::query_scalar("SELECT jsonb_build_array((SELECT count(*) FROM trades),(SELECT count(*) FROM audit_logs),(SELECT count(*) FROM notifications),(SELECT count(*) FROM market_event_outbox),(SELECT count(*) FROM referrals WHERE status IN ('VERIFIED','ACTIVE')),(SELECT count(*) FROM referrals WHERE status='ACTIVE'),(SELECT activated_at FROM referrals LIMIT 1))").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(before, after);
    assert_eq!(after[4], 1);
    assert_eq!(after[5], 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn outbox_is_commit_visible_and_polling_finds_events_without_a_listener() {
    let mut f = Fixture::new(1, "1").await;
    let barrier = Arc::new(CompletionBarrier::default());
    let pool = f.pool.clone();
    let aggressor = f.aggressor;
    let paused = barrier.clone();
    let task = tokio::spawn(async move {
        run_completed_match_paused(
            &pool,
            aggressor,
            now(),
            CompletionOptions::default(),
            paused,
        )
        .await
    });
    barrier.staged().await;
    assert_eq!(dispatch_without_notifications(false),0);
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(before, 0);
    barrier.resume();
    task.await.unwrap().unwrap();
    // A separate connection deliberately never LISTENs. Polling must still find committed work.
    let events: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type,payload::text FROM market_event_outbox ORDER BY event_type",
    )
    .fetch_all(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "orders_matched");
    assert_eq!(events[1].0, "trade_auto_matched");
    for (_, payload) in events {
        let payload: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["_public_market_invalidation"], true);
    }
    assert_eq!(dispatch_without_notifications(true),6);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn fee_configuration_and_subscription_boundaries_are_frozen() {
    for (tier, active, expiry, negotiated, expected, plan) in [
        ("free", true, None, None, "7.25", "free"),
        ("standard", true, None, None, "3.50", "standard"),
        ("enterprise", true, None, Some("0.75"), "0.75", "enterprise"),
        ("standard", false, None, None, "7.25", "free"),
        ("standard", true, Some(now()), None, "7.25", "free"),
    ] {
        let mut f = Fixture::new(2, "1.25").await;
        sqlx::query("SELECT completion_fixture_subscription($1,$2,$3,$4)")
            .bind(tier)
            .bind(active)
            .bind(expiry)
            .bind(negotiated.map(|v| Decimal::from_str_exact(v).unwrap()))
            .execute(&mut f.admin)
            .await
            .unwrap();
        let options = CompletionOptions {
            fee_config: verdaxis_read::subscriptions::FeeConfig::new(
                Decimal::from_str_exact("7.25").unwrap(),
                Decimal::from_str_exact("3.50").unwrap(),
            )
            .unwrap(),
            ..Default::default()
        };
        run_completed_match(&f.pool, f.aggressor, now(), options)
            .await
            .unwrap();
        let rows: Vec<(String, Decimal)> =
            sqlx::query_as("SELECT commission_plan,commission_fee_per_mt_usd FROM trades")
                .fetch_all(&mut f.observer)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![(plan.into(), Decimal::from_str_exact(expected).unwrap()); 2]
        );
        sqlx::query("SELECT completion_fixture_subscription('free',true,NULL,NULL)")
            .execute(&mut f.admin)
            .await
            .unwrap();
        let stored: Vec<(String, Decimal)> =
            sqlx::query_as("SELECT commission_plan,commission_fee_per_mt_usd FROM trades")
                .fetch_all(&mut f.observer)
                .await
                .unwrap();
        assert_eq!(rows, stored);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn invalid_seller_fee_and_stock_shortfall_roll_back_the_match() {
    for failure in ["enterprise", "invalid", "stock", "after_effects"] {
        let mut f = Fixture::new(2, "1.25").await;
        sqlx::query("SELECT completion_fixture_referral()")
            .execute(&mut f.admin)
            .await
            .unwrap();
        let mut options = CompletionOptions::default();
        if failure == "enterprise" || failure == "invalid" {
            sqlx::query("SELECT completion_fixture_subscription($1,true,NULL,NULL)")
                .bind(if failure == "invalid" {
                    "invalid"
                } else {
                    "enterprise"
                })
                .execute(&mut f.admin)
                .await
                .unwrap();
        } else if failure == "stock" {
            sqlx::query("SELECT completion_fixture_back_inventory($1,$2)")
                .bind(f.resting[1])
                .bind(Uuid::from_u128(999))
                .execute(&mut f.admin)
                .await
                .unwrap();
            sqlx::query("SELECT completion_fixture_short_stock($1)")
                .bind(Uuid::from_u128(999))
                .execute(&mut f.admin)
                .await
                .unwrap();
        } else {
            options.fail_after_effects = true;
        }
        let error = run_completed_match(&f.pool, f.aggressor, now(), options)
            .await
            .unwrap_err();
        assert_eq!(error.status.as_u16(), 409);
        let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM trades),(SELECT count(*) FROM audit_logs),(SELECT count(*) FROM notifications),(SELECT count(*) FROM market_event_outbox)").fetch_one(&mut f.observer).await.unwrap();
        assert_eq!(counts, (0, 0, 0, 0));
        let status: String = sqlx::query_scalar("SELECT status::text FROM referrals")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
        assert_eq!(status, "VERIFIED");
        let versions: bool = sqlx::query_scalar("SELECT bool_and(version=1) FROM orderbook_orders")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
        assert!(versions);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn preview_over_cap_refuses_only_an_unsatisfied_aggressor() {
    for (quantity, expected) in [("100", 100usize), ("101", 0usize)] {
        let f = Fixture::new(101, quantity).await;
        let result =
            run_completed_match(&f.pool, f.aggressor, now(), CompletionOptions::default()).await;
        if expected == 0 {
            assert_eq!(
                result.unwrap_err().code,
                MarketErrorCode::MatchFanoutExceeded
            );
        } else {
            assert_eq!(result.unwrap().trade_ids.len(), expected);
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn audit_context_is_bounded_by_the_shared_ingress_identity() {
    let mut f = Fixture::new(1, "1").await;
    let options = CompletionOptions {
        request_id: Some("x".repeat(500)),
        ..Default::default()
    };
    run_completed_match(&f.pool, f.aggressor, now(), options)
        .await
        .unwrap();
    let row: (String, String) = sqlx::query_as(
        "SELECT ip_address,request_id FROM audit_logs WHERE action='trade.auto_matched'",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(row.0, "127.0.0.1");
    assert!(Uuid::parse_str(&row.1).is_ok());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn match_captures_conditional_pin_and_both_slice_transitions() {
    for quantity in ["0.5", "1"] {
        let mut f = Fixture::new(1, quantity).await;
        sqlx::query("SELECT completion_fixture_watch($1)")
            .bind(f.resting[0])
            .execute(&mut f.admin)
            .await
            .unwrap();
        f.execute().await;
        let pin:(String,f64)=sqlx::query_as("SELECT snapshot_status,snapshot_remaining_quantity_mt FROM watchlist_targets WHERE target_type='PIN'").fetch_one(&mut f.observer).await.unwrap();
        assert_eq!(
            pin.0,
            if quantity == "1" {
                "FILLED"
            } else {
                "PARTIALLY_FILLED"
            }
        );
        assert_eq!(pin.1, if quantity == "1" { 0.0 } else { 0.5 });
        let evidence:String=sqlx::query_scalar("SELECT producer_evidence::text FROM market_event_outbox WHERE event_type='orders_matched'").fetch_one(&mut f.observer).await.unwrap();
        let evidence: Value = serde_json::from_str(&evidence).unwrap();
        assert_eq!(evidence["targets"].as_array().unwrap().len(), 2);
        let before = evidence["aggregates_before"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["side"] == "ASK")
            .unwrap();
        let after = evidence["aggregates_after"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["side"] == "ASK")
            .unwrap();
        assert_eq!(before["best_price_per_mt_usd"], "100");
        assert_eq!(
            after["best_price_per_mt_usd"],
            if quantity == "1" {
                Value::Null
            } else {
                json!("100")
            }
        );
        assert_eq!(before["real_count"], 1);
        assert_eq!(before["demo_count"], 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn typed_trade_snapshots_keep_side_mapping_and_private_declarations() {
    let fame_bid = json!({"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU"});
    let fame_ask = json!({"schema_version":1,"side":"ASK","neat_fame":true,"uco_mass_pct":100,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU","certificate_reference":"private-certificate","certificate_holder":"private-supplier","evidence_status":"PENDING","document_references":[],"certificate_valid_until":"2027-01-01","evidence_due":"BEFORE_LOADING","cfpp_c":-5});
    let lng_bid = json!({"schema_version":1,"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_delivery_methods":["SHIP_TO_SHIP"]});
    let lng_ask = json!({"side":"ASK","sustainability_scheme":"ISCC_EU","scheme_certificate_number":"PRIVATE-123","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"100","feedstock":"MANURE","production_country":"NL","wtt_ghg_gco2e_mj":"-20.25","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"physical_supplier_name":"Private supplier","pos_timing":"WITH_DELIVERY"});
    for (profile, bid, ask) in [("fame", fame_bid, fame_ask), ("lng", lng_bid, lng_ask)] {
        for ask_aggressor in [false, true] {
            let mut f = Fixture::new(1, "1").await;
            sqlx::query("SELECT completion_fixture_terms($1,$2,$3)")
                .bind(profile)
                .bind(&bid)
                .bind(&ask)
                .execute(&mut f.admin)
                .await
                .unwrap();
            let trigger = if ask_aggressor {
                f.resting[0]
            } else {
                f.aggressor
            };
            let compatible = if profile == "fame" {
                verdaxis_read::market_matching::fame_terms_compatible(
                    &bid,
                    &ask,
                    now().date_naive(),
                )
            } else {
                verdaxis_read::market_matching::lng_terms_compatible(&bid, &ask)
            };
            assert!(compatible, "typed pair must qualify: {profile}");
            let result = run_completed_match(&f.pool, trigger, now(), CompletionOptions::default())
                .await
                .unwrap();
            assert_eq!(
                result.trade_ids.len(),
                1,
                "profile={profile} ask_aggressor={ask_aggressor}"
            );
            let row: (Option<Value>, Option<Value>) = sqlx::query_as(
                "SELECT fame_terms_snapshot::jsonb,lng_terms_snapshot::jsonb FROM trades",
            )
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
            let snapshot = if profile == "fame" {
                assert!(row.1.is_none());
                row.0.unwrap()
            } else {
                assert!(row.0.is_none());
                row.1.unwrap()
            };
            assert_eq!(
                snapshot,
                if profile == "fame" {
                    json!({"schema_version":1,"bid":bid,"ask":ask})
                } else {
                    json!({"bid":bid,"ask":ask})
                }
            );
            sqlx::query("SELECT completion_fixture_amend_source()")
                .execute(&mut f.admin)
                .await
                .unwrap();
            let after: Value = sqlx::query_scalar(if profile == "fame" {
                "SELECT fame_terms_snapshot::jsonb FROM trades"
            } else {
                "SELECT lng_terms_snapshot::jsonb FROM trades"
            })
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
            assert_eq!(snapshot, after);
            let payloads: Vec<String> =
                sqlx::query_scalar("SELECT payload::text FROM market_event_outbox")
                    .fetch_all(&mut f.observer)
                    .await
                    .unwrap();
            assert!(
                payloads
                    .iter()
                    .all(|payload| !payload.contains("private-certificate")
                        && !payload.contains("PRIVATE-123")
                        && !payload.contains("Private supplier"))
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn notifications_fan_out_to_every_current_organization_user() {
    let mut f = Fixture::new(2, "1.25").await;
    sqlx::query("SELECT completion_fixture_extra_member()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.execute().await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE type='TRADE_CONFIRMED'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(count, 6);
    let data: Vec<Value> = sqlx::query_scalar("SELECT data::jsonb FROM notifications")
        .fetch_all(&mut f.observer)
        .await
        .unwrap();
    assert!(
        data.iter()
            .all(|v| v["auto_matched"] == true && v["trade_id"].is_string())
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn linked_fill_counts_both_sides_and_all_items_in_uuid_order() {
    for shared in [false, true] {
        let mut f = Fixture::new(2, "1.25").await;
        let first = Uuid::from_u128(990);
        let second = Uuid::from_u128(980);
        sqlx::query("SELECT completion_fixture_back_inventory($1,$2)")
            .bind(f.resting[0])
            .bind(first)
            .execute(&mut f.admin)
            .await
            .unwrap();
        if shared {
            sqlx::query("SELECT completion_fixture_link($1,$2)")
                .bind(f.resting[1])
                .bind(first)
                .execute(&mut f.admin)
                .await
                .unwrap();
            sqlx::query("SELECT completion_fixture_link($1,$2)")
                .bind(f.aggressor)
                .bind(first)
                .execute(&mut f.admin)
                .await
                .unwrap();
        } else {
            sqlx::query("SELECT completion_fixture_back_inventory($1,$2)")
                .bind(f.resting[1])
                .bind(second)
                .execute(&mut f.admin)
                .await
                .unwrap();
        }
        f.execute().await;
        let valid:bool=sqlx::query_scalar("SELECT bool_and(i.reserved_stock_mt=COALESCE((SELECT sum(o.remaining_quantity_mt) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.status IN ('OPEN','PARTIALLY_FILLED')),0)) FROM inventory_items i").fetch_one(&mut f.observer).await.unwrap();
        assert!(valid);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn slice_evidence_distinguishes_best_price_move_from_quiet() {
    let mut f = Fixture::new(2, "1").await;
    sqlx::query("SELECT completion_fixture_price($1,105)")
        .bind(f.resting[1])
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT completion_fixture_watch($1)")
        .bind(f.resting[0])
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.execute().await;
    let raw: String = sqlx::query_scalar(
        "SELECT producer_evidence::text FROM market_event_outbox WHERE event_type='orders_matched'",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    let evidence: Value = serde_json::from_str(&raw).unwrap();
    let best = |name: &str| {
        evidence[name]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["side"] == "ASK")
            .unwrap()["best_price_per_mt_usd"]
            .clone()
    };
    assert_eq!(best("aggregates_before"), "100");
    assert_eq!(best("aggregates_after"), "105");
    assert_eq!(evidence["targets"].as_array().unwrap().len(), 2);
}

fn dispatch_without_notifications(run_loops:bool)->usize {
    let script=r#"
import asyncio, json, os, sys
import asyncpg
from sqlalchemy.ext.asyncio import create_async_engine
from app.services.market_event_dispatch import MarketEventDispatcher
class Bus:
    def __init__(self): self.events=[]
    async def publish(self,channel,kind,payload,**kwargs):
        assert 'producer_evidence' not in payload and 'targets' not in payload
        self.events.append((channel,kind,payload))
async def main():
    url=os.environ['RUST_MATCHING_DATABASE_URL']
    engine=create_async_engine(url.replace('postgresql://','postgresql+asyncpg://'))
    bus=Bus(); dispatcher=MarketEventDispatcher(engine,bus,poll_seconds=0.05)
    # A real dedicated dispatcher session, deliberately without add_listener.
    dispatcher._listener_conn=await asyncpg.connect(url)
    assert await dispatcher._try_acquire_leadership()
    if sys.argv[1]=='true':
        dispatcher._tasks=[asyncio.create_task(dispatcher._sequencer_loop()),asyncio.create_task(dispatcher._hub_loop())]
        for _ in range(100):
            if len(bus.events)==6: break
            await asyncio.sleep(0.05)
        assert sorted((channel,kind) for channel,kind,_ in bus.events)==sorted([
            ('trades:00000000-0000-0000-0000-000000001000','trade_auto_matched'),
            ('trades:00000000-0000-0000-0000-000000002000','trade_auto_matched'),
            ('trades:00000000-0000-0000-0000-000000001000','orders_matched'),
            ('trades:00000000-0000-0000-0000-000000002000','orders_matched'),
            ('prices','market_invalidated'),('orderbook','market_invalidated')])
    else:
        assert await dispatcher._assign_pending_sequences()==0
        await dispatcher._drain_new_events()
    print(len(bus.events))
    await dispatcher.stop(); await engine.dispose()
asyncio.run(main())
"#;
    let python=std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_|"venv/bin/python".into());
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result=std::process::Command::new(python).current_dir(root).arg("-c").arg(script).arg(run_loops.to_string()).output().unwrap();
    assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().parse().unwrap()
}
