use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;
use verdaxis_read::{
    Config, MarketPolicy,
    market::live_benchmarks::{LiveBenchmarkKey, live_benchmarks},
    order_write::{
        BenchmarkRebuildKey, InvalidateOrganization, InvalidationError, SliceKey,
        invalidate_organization, market_slice_lock_key, ordered_slice_lock_keys,
        rebuild_live_slice_benchmarks_for_keys,
    },
};

fn now() -> DateTime<Utc> {
    "2026-10-24T03:00:00Z".parse().unwrap()
}

struct Fixture {
    conn: PgConnection,
    schema: String,
    command: InvalidateOrganization,
    product: Uuid,
    point: Uuid,
    inventory: Uuid,
    other: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL required");
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(parsed.path().ends_with("_test"));
        assert!(!matches!(parsed.port(), Some(5432 | 8000 | 8001)));
        let mut conn = PgConnection::connect(&url).await.unwrap();
        let schema = format!("p4_t7_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE organizations(id uuid PRIMARY KEY, verification_status text DEFAULT 'APPROVED',provenance text DEFAULT 'REAL');
             CREATE TABLE users(id uuid PRIMARY KEY, organization_id uuid,role text DEFAULT 'ADMIN',status text DEFAULT 'APPROVED',email_verified boolean DEFAULT true,must_change_password boolean DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,kyc_status text DEFAULT 'PENDING',kyc_organization_id uuid);
             CREATE TABLE organization_join_requests(id uuid PRIMARY KEY);
             CREATE TABLE kyc_execution_grace(user_id uuid,organization_id uuid,expires_at timestamptz);
             CREATE TABLE products(id uuid PRIMARY KEY,name text,fuel_type text,fuel_grade text,is_active boolean DEFAULT true);
             CREATE TABLE delivery_points(id uuid PRIMARY KEY,name text,region text,is_active boolean DEFAULT true);
             CREATE TABLE inventory_items(id uuid PRIMARY KEY,current_stock_mt numeric NOT NULL CHECK(current_stock_mt>=0),reserved_stock_mt numeric NOT NULL CHECK(reserved_stock_mt>=0));
             CREATE TABLE orderbook_orders(id uuid PRIMARY KEY,organization_id uuid,side text,product_id uuid,delivery_point_id uuid,availability_window text,status text DEFAULT 'OPEN',quantity_mt numeric DEFAULT 100 CHECK(quantity_mt>=0),remaining_quantity_mt numeric DEFAULT 60 CHECK(remaining_quantity_mt>=0 AND remaining_quantity_mt<=quantity_mt),version int DEFAULT 1 CHECK(version>=1),inventory_item_id uuid,price_per_mt_usd numeric DEFAULT 200,expires_at timestamptz,created_at timestamptz DEFAULT '2026-10-01Z',updated_at timestamptz,provenance text DEFAULT 'REAL',owner_user_id uuid,created_by_actor_user_id uuid,creation_method text DEFAULT 'DIRECT',off_spec boolean DEFAULT false,msds_available boolean DEFAULT true,certification_declared boolean DEFAULT true,certification_scheme text DEFAULT 'test',specification_standard text DEFAULT 'test',carbon_intensity_gco2_mj numeric DEFAULT 10,carbon_intensity_method text,energy_density_mj_kg numeric,feedstock text DEFAULT 'test',origin text DEFAULT 'test',fame_terms jsonb,lng_terms jsonb);
             CREATE INDEX ON orderbook_orders(organization_id,status,id);
             CREATE TABLE trades(id uuid PRIMARY KEY,buyer_id uuid,seller_id uuid,bid_order_id uuid,ask_order_id uuid,product_id uuid,delivery_point_id uuid,availability_window text,status text,quantity_mt numeric,buyer_user_id uuid,seller_user_id uuid,fee_snapshot jsonb,terms_snapshot jsonb);
             CREATE INDEX ON trades(buyer_id,status,id); CREATE INDEX ON trades(seller_id,status,id);
             CREATE TABLE live_slice_benchmarks(id uuid PRIMARY KEY,side varchar(3),market_product varchar(64),delivery_point_id uuid NOT NULL,availability_window varchar(16),benchmark_price_per_mt_usd numeric(10,2),total_remaining_quantity_mt numeric(12,2),order_count int,source varchar(32),created_at timestamptz,updated_at timestamptz,CONSTRAINT uq_live_slice_benchmarks_slice_key UNIQUE(side,market_product,delivery_point_id,availability_window));
             CREATE TABLE watchlist_targets(id uuid PRIMARY KEY,watchlist_id uuid,target_type text,order_id uuid,market_product_code text,delivery_point_id uuid,availability_window_code text,snapshot_price_per_mt_usd double precision,snapshot_quantity_mt double precision,snapshot_remaining_quantity_mt double precision,snapshot_status text,snapshot_side text,snapshot_market_product text,snapshot_delivery_point_name text,snapshot_availability_window text,snapshot_counterparty_label text);
             CREATE TABLE watchlist_events(id uuid PRIMARY KEY,watchlist_id uuid,watchlist_target_id uuid,event_type text,event_payload jsonb,is_read boolean DEFAULT false,created_at timestamptz);
             CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action text,resource_type text,resource_id text,changes jsonb,ip_address text,request_id text,timestamp timestamptz);
             CREATE TABLE market_event_outbox(id uuid PRIMARY KEY,event_type text,aggregate_type text,aggregate_id text,participant_org_ids jsonb,payload jsonb,created_at timestamptz,dispatched_at timestamptz,delivery_attempts int DEFAULT 0,stream_seq bigint);"
        ).execute(&mut conn).await.unwrap();
        let product = Uuid::new_v4();
        let point = Uuid::new_v4();
        let organization = Uuid::new_v4();
        let actor = Uuid::new_v4();
        let inventory = Uuid::new_v4();
        let other = Uuid::new_v4();
        let policy: MarketPolicy = serde_json::from_value(serde_json::json!({
            "source_sha":"test", "products":[{"id":product,"code":"fixture-product","name":"fixture-product","fuel_type":"Methanol","fuel_grade":"fixture","execution_mode":"ORDERBOOK","lanes":[point],"qualification_profile":"scalar","display_order":1}],
            "points":[{"id":point,"name":"fixture-point","region":"fixture-region","timezone":"UTC","display_order":1}]
        })).unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "kyc_execution_gate_enabled":false,"environment":"test","release_sha":"test","migration_revision":"test","database_url":"unused","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":3,"python_budget":0,"configured_max_connections":100
        })).unwrap();
        sqlx::query("INSERT INTO products(id,name,fuel_type,fuel_grade) VALUES($1,'fixture-product','Methanol','fixture')").bind(product).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO delivery_points(id,name,region) VALUES($1,'fixture-point','fixture-region')").bind(point).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO organizations(id) VALUES($1),($2)")
            .bind(organization)
            .bind(other)
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users(id,organization_id) VALUES($1,$2)")
            .bind(actor)
            .bind(organization)
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO inventory_items VALUES($1,20,80)")
            .bind(inventory)
            .execute(&mut conn)
            .await
            .unwrap();
        Self {
            conn,
            schema,
            command: InvalidateOrganization {
                organization_id: organization,
                actor_id: actor,
                now: now(),
                policy: Arc::new(policy),
                config: Arc::new(config),
            },
            product,
            point,
            inventory,
            other,
        }
    }

    async fn order(&mut self, own: bool, remaining: i32, reserved: bool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(include_str!(
            "fixtures/market_invalidation_order_insert.sql"
        ))
        .bind(id)
        .bind(if own {
            self.command.organization_id
        } else {
            self.other
        })
        .bind(self.product)
        .bind(self.point)
        .bind(Decimal::from(remaining))
        .bind(reserved.then_some(self.inventory))
        .execute(&mut self.conn)
        .await
        .unwrap();
        id
    }

    async fn trade(&mut self, order: Uuid, state: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO trades VALUES($1,$2,$3,NULL,$4,$5,$6,'SPOT',$7,20,$8,$9,'{\"rate\":12.5}','{\"retained\":true}')")
            .bind(id).bind(self.other).bind(self.command.organization_id).bind(order).bind(self.product).bind(self.point).bind(state).bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(&mut self.conn).await.unwrap();
        id
    }

    fn key(&self) -> BenchmarkRebuildKey {
        BenchmarkRebuildKey {
            side: "ASK".into(),
            market_product: Some("fixture-product".into()),
            delivery_point_id: Some(self.point),
            availability_window: Some("SPOT".into()),
        }
    }

    async fn snapshot(&mut self) -> serde_json::Value {
        sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM orderbook_orders o),'trades',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM trades t),'stock',(SELECT jsonb_agg(to_jsonb(i) ORDER BY id) FROM inventory_items i),'benchmarks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM live_slice_benchmarks b),'watch',(SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM watchlist_events w),'targets',(SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM watchlist_targets w),'audit',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_logs a),'outbox',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM market_event_outbox e))").fetch_one(&mut self.conn).await.unwrap()
    }

    async fn finish(mut self) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.conn)
        .await
        .unwrap();
    }
}

#[test]
fn p4_t7_c06() {
    let p = Uuid::from_u128(1);
    let d = Uuid::from_u128(2);
    let slices = vec![
        SliceKey {
            product_id: p,
            delivery_point_id: Some(d),
            availability_window: "SPOT".into(),
        },
        SliceKey {
            product_id: p,
            delivery_point_id: None,
            availability_window: "SPOT".into(),
        },
        SliceKey {
            product_id: p,
            delivery_point_id: Some(d),
            availability_window: "SPOT".into(),
        },
    ];
    assert_eq!(
        ordered_slice_lock_keys(&slices).unwrap(),
        vec![
            market_slice_lock_key(p, None, "SPOT"),
            market_slice_lock_key(p, Some(d), "SPOT")
        ]
    );
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/market-write-spec.json"
    ))
    .unwrap();
    for vector in vectors["slice_lock_keys"].as_array().unwrap() {
        let product = vector["product_id"].as_str().unwrap().parse().unwrap();
        let point = vector["delivery_point_id"]
            .as_str()
            .map(|v| v.parse().unwrap());
        assert_eq!(
            market_slice_lock_key(
                product,
                point,
                vector["normalized_window"].as_str().unwrap()
            ),
            vector["lock_key"].as_i64().unwrap()
        );
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c01() {
    for commitments in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.order(true, 60, false).await;
        if commitments {
            for _ in 0..101 {
                f.trade(order, "PENDING_CONFIRMATION").await;
            }
        } else {
            for _ in 0..100 {
                f.order(true, 60, false).await;
            }
        }
        let before = f.snapshot().await;
        let mut tx = f.conn.begin().await.unwrap();
        let error = invalidate_organization(&mut tx, &f.command)
            .await
            .unwrap_err();
        assert!(matches!(error,InvalidationError::Application(ref e) if e.status==409));
        tx.rollback().await.unwrap();
        assert_eq!(f.snapshot().await, before);
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c02() {
    let mut f = Fixture::new().await;
    let order = f.order(true, 60, true).await;
    let trade = f.trade(order, "PENDING_CONFIRMATION").await;
    let other = f.order(false, 40, false).await;
    let before = f.snapshot().await;
    let mut tx = f.conn.begin().await.unwrap();
    let result = invalidate_organization(&mut tx, &f.command).await.unwrap();
    assert_eq!(result.withdrawn_orders, vec![order]);
    assert_eq!(result.preserved_trade_ids, vec![trade]);
    assert_eq!(result.released_uncommitted_quantity, Decimal::from(60));
    assert_eq!(result.outbox_ids.len(), 1);
    tx.commit().await.unwrap();
    let after = f.snapshot().await;
    assert_eq!(after["trades"], before["trades"]);
    let row = sqlx::query(
        "SELECT status,remaining_quantity_mt,version FROM orderbook_orders WHERE id=$1",
    )
    .bind(order)
    .fetch_one(&mut f.conn)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "CANCELLED");
    assert_eq!(
        row.get::<Decimal, _>("remaining_quantity_mt"),
        Decimal::from(60)
    );
    assert_eq!(row.get::<i32, _>("version"), 2);
    let status: String = sqlx::query_scalar("SELECT status FROM orderbook_orders WHERE id=$1")
        .bind(other)
        .fetch_one(&mut f.conn)
        .await
        .unwrap();
    assert_eq!(status, "OPEN");
    assert!(after["outbox"][0]["dispatched_at"].is_null());
    assert_eq!(
        after["outbox"][0]["payload"]["_public_market_invalidation"],
        true
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c03() {
    let mut f = Fixture::new().await;
    f.order(true, 60, true).await;
    let mut tx = f.conn.begin().await.unwrap();
    invalidate_organization(&mut tx, &f.command).await.unwrap();
    tx.commit().await.unwrap();
    let row = sqlx::query("SELECT current_stock_mt,reserved_stock_mt FROM inventory_items")
        .fetch_one(&mut f.conn)
        .await
        .unwrap();
    assert_eq!(row.get::<Decimal, _>(0), Decimal::from(80));
    assert_eq!(row.get::<Decimal, _>(1), Decimal::from(20));
    let before = f.snapshot().await;
    let mut tx = f.conn.begin().await.unwrap();
    let result = invalidate_organization(&mut tx, &f.command).await.unwrap();
    assert!(result.withdrawn_orders.is_empty());
    tx.rollback().await.unwrap();
    assert_eq!(before, f.snapshot().await);
    f.order(true, 30, true).await;
    let before = f.snapshot().await;
    let mut tx = f.conn.begin().await.unwrap();
    assert!(matches!(
        invalidate_organization(&mut tx, &f.command).await,
        Err(InvalidationError::Application(_))
    ));
    tx.rollback().await.unwrap();
    assert_eq!(before, f.snapshot().await);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c04() {
    let mut f = Fixture::new().await;
    let order = f.order(true, 0, false).await;
    for state in [
        "PENDING_CONFIRMATION",
        "CONFIRMED",
        "DELIVERED",
        "PAID",
        "CANCELLED",
        "DECLINED",
    ] {
        f.trade(order, state).await;
    }
    let before = f.snapshot().await;
    let mut tx = f.conn.begin().await.unwrap();
    let result = invalidate_organization(&mut tx, &f.command).await.unwrap();
    assert_eq!(result.preserved_trade_ids.len(), 3);
    tx.commit().await.unwrap();
    assert_eq!(before["trades"], f.snapshot().await["trades"]);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c05() {
    use verdaxis_read::auth::lock::WriteConflict;
    let mut f = Fixture::new().await;
    let order = f.order(true, 60, false).await;
    // A volatile lock wrapper changes the row exactly between preview and re-read.
    sqlx::raw_sql(sqlx::AssertSqlSafe(include_str!(
        "fixtures/market_invalidation_advisory_override.sql"
    )))
    .execute(&mut f.conn)
    .await
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET search_path TO {},pg_catalog",
        f.schema
    )))
    .execute(&mut f.conn)
    .await
    .unwrap();
    let before = f.snapshot().await;
    let mut tx = f.conn.begin().await.unwrap();
    let error = invalidate_organization(&mut tx, &f.command)
        .await
        .unwrap_err();
    let attempt: verdaxis_read::business::transaction::AttemptError = error.into();
    assert!(attempt.is_write_conflict());
    tx.rollback().await.unwrap();
    assert_eq!(before, f.snapshot().await);
    let window: String =
        sqlx::query_scalar("SELECT availability_window FROM orderbook_orders WHERE id=$1")
            .bind(order)
            .fetch_one(&mut f.conn)
            .await
            .unwrap();
    assert_eq!(window, "SPOT");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c07() {
    let mut f = Fixture::new().await;
    let order = f.order(false, 40, false).await;
    let key = f.key();
    let mut tx = f.conn.begin().await.unwrap();
    let result = rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        &[key.clone(), key.clone()],
        now(),
    )
    .await
    .unwrap();
    assert_eq!(result.rebuilt.len(), 1);
    let read_key = LiveBenchmarkKey {
        side: "ASK".into(),
        product: "fixture-product".into(),
        point: f.point,
        window: "SPOT".into(),
    };
    let read = live_benchmarks(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        std::slice::from_ref(&read_key),
        now(),
    )
    .await
    .unwrap();
    assert_eq!(read[&read_key].price, Decimal::from(200));
    let row=sqlx::query("SELECT id,created_at,benchmark_price_per_mt_usd,total_remaining_quantity_mt,source FROM live_slice_benchmarks").fetch_one(&mut *tx).await.unwrap();
    let id: Uuid = row.get(0);
    assert_eq!(row.get::<Decimal, _>(3), Decimal::from(40));
    assert_eq!(row.get::<String, _>(4), "live_slice_vwap");
    sqlx::query(include_str!("fixtures/market_invalidation_order_price.sql"))
        .bind(order)
        .execute(&mut *tx)
        .await
        .unwrap();
    rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        std::slice::from_ref(&key),
        now(),
    )
    .await
    .unwrap();
    let updated =
        sqlx::query("SELECT id,created_at,benchmark_price_per_mt_usd FROM live_slice_benchmarks")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(updated.get::<Uuid, _>(0), id);
    assert_eq!(updated.get::<DateTime<Utc>, _>(1), now());
    assert_eq!(updated.get::<Decimal, _>(2), Decimal::from(300));
    let mut null = key.clone();
    null.delivery_point_id = None;
    let ignored = rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        &[null],
        now(),
    )
    .await
    .unwrap();
    assert_eq!(ignored.ignored_null_points, 1);
    assert!(ignored.rebuilt.is_empty());
    sqlx::query(include_str!(
        "fixtures/market_invalidation_order_expiry.sql"
    ))
    .bind(now())
    .bind(order)
    .execute(&mut *tx)
    .await
    .unwrap();
    let deleted = rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        &[key],
        now(),
    )
    .await
    .unwrap();
    assert_eq!(deleted.deleted.len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM live_slice_benchmarks")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        0
    );
    tx.rollback().await.unwrap();
    assert!(f.snapshot().await["benchmarks"].is_null());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c08() {
    for table in [
        "live_slice_benchmarks",
        "watchlist_events",
        "audit_logs",
        "market_event_outbox",
    ] {
        let mut f = Fixture::new().await;
        let order = f.order(true, 60, true).await;
        f.order(false, 40, false).await;
        sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code) VALUES($1,$2,'PIN',$3,'fixture-product',$4,'SPOT')")
            .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(order).bind(f.point).execute(&mut f.conn).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION fail_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture refusal' USING ERRCODE='40001'; END $$; CREATE TRIGGER fail_effect BEFORE INSERT OR UPDATE OR DELETE ON {table} FOR EACH STATEMENT EXECUTE FUNCTION fail_effect()"))).execute(&mut f.conn).await.unwrap();
        let before = f.snapshot().await;
        let mut tx = f.conn.begin().await.unwrap();
        let error = invalidate_organization(&mut tx, &f.command)
            .await
            .unwrap_err();
        assert!(
            matches!(error,InvalidationError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("40001")),
            "{table}: {error:?}"
        );
        tx.rollback().await.unwrap();
        assert_eq!(before, f.snapshot().await, "{table}");
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c09() {
    let mut f = Fixture::new().await;
    for _ in 0..100 {
        f.order(true, 0, false).await;
    }
    let mut tx = f.conn.begin().await.unwrap();
    let result = invalidate_organization(&mut tx, &f.command).await.unwrap();
    assert_eq!(result.withdrawn_orders.len(), 100);
    tx.rollback().await.unwrap();
    let key = f.key();
    let mut bad = key.clone();
    bad.availability_window = Some("bad-window".into());
    let mut tx = f.conn.begin().await.unwrap();
    assert!(matches!(
        rebuild_live_slice_benchmarks_for_keys(
            &mut tx,
            &f.command.policy,
            &f.command.config,
            &[bad],
            now()
        )
        .await,
        Err(InvalidationError::Application(_))
    ));
    let empty = rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        &[],
        now(),
    )
    .await
    .unwrap();
    assert!(empty.rebuilt.is_empty() && empty.deleted.is_empty());
    tx.rollback().await.unwrap();
    f.finish().await;
}

struct OrderedInvalidation {
    command: InvalidateOrganization,
}

impl verdaxis_read::business::transaction::BusinessCommand for OrderedInvalidation {
    type Output = verdaxis_read::order_write::InvalidationResult;
    fn preview<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
    ) -> verdaxis_read::business::transaction::CommandFuture<
        'a,
        verdaxis_read::business::transaction::OrganizationWritePlan,
        verdaxis_read::business::transaction::AttemptError,
    > {
        Box::pin(async move {
            Ok(
                verdaxis_read::business::transaction::OrganizationWritePlan::new(
                    [self.command.organization_id],
                    [self.command.actor_id],
                    [],
                    serde_json::json!({"organization":self.command.organization_id}),
                ),
            )
        })
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a verdaxis_read::auth::CurrentActor,
    ) -> verdaxis_read::business::transaction::CommandFuture<
        'a,
        Self::Output,
        verdaxis_read::business::transaction::AttemptError,
    > {
        Box::pin(async move {
            assert_eq!(actor.id, self.command.actor_id);
            Ok(invalidate_organization(conn, &self.command).await?)
        })
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c10() {
    use verdaxis_read::{
        auth::{VerificationSettings, verify_access},
        business::transaction::run_ordered_write,
    };
    let mut f = Fixture::new().await;
    f.order(true, 60, true).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {} TO rust_app; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {} TO rust_app; CREATE SEQUENCE attempt_number; GRANT USAGE ON SEQUENCE attempt_number TO rust_app; CREATE FUNCTION retry_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF nextval('attempt_number')<3 THEN RAISE EXCEPTION 'retry fixture' USING ERRCODE='40001'; END IF; RETURN NEW; END $$; CREATE TRIGGER retry_effect BEFORE INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION retry_effect()",f.schema,f.schema))).execute(&mut f.conn).await.unwrap();
    let runtime =
        std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL required");
    let url = url::Url::parse(&runtime).unwrap();
    assert_eq!(url.username(), "rust_app");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(url.path().ends_with("_test"));
    let search = format!("SET search_path TO {}", f.schema);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |conn, _| {
            let sql = search.clone();
            Box::pin(async move {
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&runtime)
        .await
        .unwrap();
    let secret = "test-secret-key-for-testing-minimum-32-chars";
    let claims = serde_json::json!({"sub":f.command.actor_id,"iat":Utc::now().timestamp(),"exp":Utc::now().timestamp()+3600,"type":"access","iss":"test","aud":"test"});
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    let access = verify_access(
        &token,
        &VerificationSettings::new(secret.into(), None, "test".into(), "test".into()),
        Utc::now(),
    )
    .unwrap();
    let command = InvalidateOrganization {
        organization_id: f.command.organization_id,
        actor_id: f.command.actor_id,
        now: f.command.now,
        policy: f.command.policy.clone(),
        config: f.command.config.clone(),
    };
    let result = run_ordered_write(
        pool.clone(),
        Arc::new(tokio::sync::Semaphore::new(1)),
        access,
        OrderedInvalidation { command },
    )
    .await
    .unwrap();
    assert_eq!(result.withdrawn_orders.len(), 1);
    let row=sqlx::query("SELECT (SELECT last_value FROM attempt_number) AS attempts,(SELECT count(*) FROM market_event_outbox) AS events,(SELECT count(*) FROM audit_logs) AS audits").fetch_one(&mut f.conn).await.unwrap();
    assert_eq!(row.get::<i64, _>("attempts"), 3);
    assert_eq!(row.get::<i64, _>("events"), 1);
    assert_eq!(row.get::<i64, _>("audits"), 2);
    let stock: Decimal = sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items")
        .fetch_one(&mut f.conn)
        .await
        .unwrap();
    assert_eq!(stock, Decimal::from(20));
    pool.close().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c11() {
    let mut f = Fixture::new().await;
    f.order(true, 60, true).await;
    let before = f.snapshot().await;
    let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").unwrap();
    let mut contender = PgConnection::connect(&url).await.unwrap();
    let mut held = contender.begin().await.unwrap();
    let key = market_slice_lock_key(f.product, Some(f.point), "SPOT");
    sqlx::query(include_str!(
        "fixtures/market_invalidation_advisory_lock.sql"
    ))
    .bind(key)
    .execute(&mut *held)
    .await
    .unwrap();
    let mut tx = f.conn.begin().await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
        .bind(f.command.organization_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = invalidate_organization(&mut tx, &f.command)
        .await
        .unwrap_err();
    assert!(
        matches!(error,InvalidationError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
    );
    tx.rollback().await.unwrap();
    held.rollback().await.unwrap();
    assert_eq!(before, f.snapshot().await);
    let mut tx = f.conn.begin().await.unwrap();
    invalidate_organization(&mut tx, &f.command).await.unwrap();
    tx.commit().await.unwrap();
    let reserved: Decimal = sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items")
        .fetch_one(&mut f.conn)
        .await
        .unwrap();
    assert_eq!(reserved, Decimal::from(20));
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c12() {
    let mut f = Fixture::new().await;
    let owned = f.order(true, 60, false).await;
    f.order(false, 40, false).await;
    let demo = f.order(false, 40, false).await;
    sqlx::query(include_str!("fixtures/market_invalidation_order_demo.sql"))
        .bind(now() + chrono::Duration::days(1))
        .bind(demo)
        .execute(&mut f.conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code) VALUES($1,$2,'SLICE',NULL,'fixture-product',$3,'SPOT'),($4,$2,'PIN',$5,'fixture-product',$3,'SPOT')").bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(f.point).bind(Uuid::new_v4()).bind(owned).execute(&mut f.conn).await.unwrap();
    let mut tx = f.conn.begin().await.unwrap();
    invalidate_organization(&mut tx, &f.command).await.unwrap();
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT event_payload FROM watchlist_events WHERE event_type='SLICE_BEST_PRICE_MOVED'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(payload["real_count"], 1);
    assert_eq!(payload["demo_count"], 1);
    assert_eq!(payload["demo_status"], "REAL_ONLY");
    let pin: serde_json::Value = sqlx::query_scalar(
        "SELECT event_payload FROM watchlist_events WHERE event_type='PIN_WITHDRAWN'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(pin["demo_status"], "REAL_ONLY");
    tx.rollback().await.unwrap();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c13() {
    let mut f = Fixture::new().await;
    let order = f.order(false, 40, false).await;
    sqlx::query("UPDATE users SET role='SUPPLIER' WHERE id=$1")
        .bind(f.command.actor_id)
        .execute(&mut f.conn)
        .await
        .unwrap();
    sqlx::query(include_str!(
        "fixtures/market_invalidation_order_reassign.sql"
    ))
    .bind(f.command.organization_id)
    .bind(f.command.actor_id)
    .bind(order)
    .execute(&mut f.conn)
    .await
    .unwrap();
    let key = f.key();
    let mut strict = (*f.command.config).clone();
    strict.kyc_execution_gate_enabled = true;
    let mut tx = f.conn.begin().await.unwrap();
    let strict_result = rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &strict,
        std::slice::from_ref(&key),
        now(),
    )
    .await
    .unwrap();
    assert_eq!(strict_result.deleted.len(), 1);
    let advisory = rebuild_live_slice_benchmarks_for_keys(
        &mut tx,
        &f.command.policy,
        &f.command.config,
        std::slice::from_ref(&key),
        now(),
    )
    .await
    .unwrap();
    assert_eq!(advisory.rebuilt.len(), 1);
    let mut policy = (*f.command.policy).clone();
    policy.products[0].lanes.clear();
    let excluded =
        rebuild_live_slice_benchmarks_for_keys(&mut tx, &policy, &f.command.config, &[key], now())
            .await
            .unwrap();
    assert_eq!(excluded.deleted.len(), 1);
    tx.rollback().await.unwrap();
    f.finish().await;
}

async fn evidence_mix(real: i64, demo: i64, unknown: i64) {
    for side in ["ASK", "BID"] {
        let mut f = Fixture::new().await;
        let owned = f.order(true, 60, false).await;
        sqlx::query(include_str!("fixtures/market_invalidation_order_side.sql"))
            .bind(side)
            .bind(owned)
            .execute(&mut f.conn)
            .await
            .unwrap();
        for (provenance, count, price) in [
            ("REAL", real, 300),
            ("DEMO", demo, 200),
            ("UNKNOWN", unknown, 1),
        ] {
            for _ in 0..count {
                let id = f.order(false, 40, false).await;
                sqlx::query(include_str!(
                    "fixtures/market_invalidation_order_side_full.sql"
                ))
                .bind(side)
                .bind(provenance)
                .bind(Decimal::from(price))
                .bind(now() + chrono::Duration::days(1))
                .bind(id)
                .execute(&mut f.conn)
                .await
                .unwrap();
            }
        }
        sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,delivery_point_id,availability_window_code) VALUES($1,$2,'SLICE','fixture-product',$3,'SPOT')")
            .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(f.point).execute(&mut f.conn).await.unwrap();
        let before = f.snapshot().await;
        let mut tx = f.conn.begin().await.unwrap();
        invalidate_organization(&mut tx, &f.command).await.unwrap();
        let payload: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT event_payload FROM watchlist_events WHERE event_type='SLICE_BEST_PRICE_MOVED'",
        )
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
        if real + demo == 0 {
            // Python has no selected price and emits no price-moved event.
            assert!(payload.is_none());
        } else {
            let payload = payload.expect("selected evidence must persist a slice event");
            assert_eq!(payload["real_count"], real);
            assert_eq!(payload["demo_count"], demo);
            assert_eq!(payload["unknown_count"], unknown);
            assert_eq!(
                payload["source_kind"],
                if real > 0 { "LIVE_ORDER" } else { "DEMO_SEED" }
            );
            assert_eq!(
                payload["demo_status"],
                if real > 0 { "REAL_ONLY" } else { "DEMO_ONLY" }
            );
            assert_eq!(
                payload["new_price_per_mt_usd"]
                    .as_str()
                    .unwrap()
                    .parse::<Decimal>()
                    .unwrap(),
                Decimal::from(if real > 0 { 300 } else { 200 })
            );
        }
        tx.rollback().await.unwrap();
        assert_eq!(before, f.snapshot().await);
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c14() {
    evidence_mix(2, 0, 0).await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c15() {
    evidence_mix(0, 3, 0).await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c16() {
    evidence_mix(0, 0, 4).await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c17() {
    evidence_mix(2, 3, 0).await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c18() {
    evidence_mix(2, 0, 4).await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c19() {
    evidence_mix(0, 3, 4).await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c20() {
    evidence_mix(2, 3, 4).await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c21() {
    let mut f = Fixture::new().await;
    let owned = f.order(true, 60, false).await;
    sqlx::query(include_str!("fixtures/market_invalidation_order_point.sql"))
        .bind(owned)
        .execute(&mut f.conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,availability_window_code) VALUES($1,$2,'PIN',$3,'fixture-product','SPOT')")
        .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(owned).execute(&mut f.conn).await.unwrap();
    let mut tx = f.conn.begin().await.unwrap();
    invalidate_organization(&mut tx, &f.command).await.unwrap();
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT event_payload FROM watchlist_events WHERE event_type='PIN_WITHDRAWN'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(payload["scope"], "UNKNOWN");
    assert!(payload["delivery_point_id"].is_null());
    tx.rollback().await.unwrap();
    f.finish().await;
}
