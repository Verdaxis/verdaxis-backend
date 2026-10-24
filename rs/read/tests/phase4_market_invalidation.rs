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
        sqlx::query("INSERT INTO organizations(id) VALUES($1),($2)").bind(organization).bind(other).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO users(id,organization_id) VALUES($1,$2)").bind(actor).bind(organization).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO inventory_items VALUES($1,20,80)").bind(inventory).execute(&mut conn).await.unwrap();
        Self { conn, schema, command: InvalidateOrganization { organization_id:organization,actor_id:actor,now:now(),policy:Arc::new(policy),config:Arc::new(config) },product,point,inventory,other }
    }

    async fn order(&mut self, own: bool, remaining: i32, reserved: bool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO orderbook_orders(id,organization_id,side,product_id,delivery_point_id,availability_window,remaining_quantity_mt,inventory_item_id) VALUES($1,$2,'ASK',$3,$4,'SPOT',$5,$6)")
            .bind(id).bind(if own {self.command.organization_id} else {self.other}).bind(self.product).bind(self.point).bind(Decimal::from(remaining)).bind(reserved.then_some(self.inventory)).execute(&mut self.conn).await.unwrap();
        id
    }

    async fn trade(&mut self, order: Uuid, state: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO trades VALUES($1,$2,$3,NULL,$4,$5,$6,'SPOT',$7,20,$8,$9,'{\"rate\":12.5}','{\"retained\":true}')")
            .bind(id).bind(self.other).bind(self.command.organization_id).bind(order).bind(self.product).bind(self.point).bind(state).bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(&mut self.conn).await.unwrap();
        id
    }

    fn key(&self) -> BenchmarkRebuildKey {
        BenchmarkRebuildKey { side:"ASK".into(), market_product:Some("fixture-product".into()),delivery_point_id:Some(self.point),availability_window:Some("SPOT".into()) }
    }

    async fn snapshot(&mut self) -> serde_json::Value {
        sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM orderbook_orders o),'trades',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM trades t),'stock',(SELECT jsonb_agg(to_jsonb(i) ORDER BY id) FROM inventory_items i),'benchmarks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM live_slice_benchmarks b),'watch',(SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM watchlist_events w),'targets',(SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM watchlist_targets w),'audit',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_logs a),'outbox',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM market_event_outbox e))").fetch_one(&mut self.conn).await.unwrap()
    }

    async fn finish(mut self) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {} CASCADE",self.schema))).execute(&mut self.conn).await.unwrap();
    }
}

#[test]
fn p4_t7_c06() {
    let p=Uuid::from_u128(1); let d=Uuid::from_u128(2);
    let slices=vec![SliceKey{product_id:p,delivery_point_id:Some(d),availability_window:"SPOT".into()},SliceKey{product_id:p,delivery_point_id:None,availability_window:"SPOT".into()},SliceKey{product_id:p,delivery_point_id:Some(d),availability_window:"SPOT".into()}];
    assert_eq!(ordered_slice_lock_keys(&slices).unwrap(),vec![market_slice_lock_key(p,None,"SPOT"),market_slice_lock_key(p,Some(d),"SPOT")]);
    let vectors:serde_json::Value=serde_json::from_str(include_str!("../../../tests/rust_contract/phase2/market-write-spec.json")).unwrap();
    assert!(vectors.is_object());
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c01() {
    for commitments in [false,true] {
        let mut f=Fixture::new().await;
        let order=f.order(true,60,false).await;
        if commitments { for _ in 0..101 {f.trade(order,"PENDING_CONFIRMATION").await;} }
        else {for _ in 0..100 {f.order(true,60,false).await;} }
        let before=f.snapshot().await;
        let mut tx=f.conn.begin().await.unwrap();
        let error=invalidate_organization(&mut tx,&f.command).await.unwrap_err();
        assert!(matches!(error,InvalidationError::Application(ref e) if e.status==409));
        tx.rollback().await.unwrap();
        assert_eq!(f.snapshot().await,before);
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c02() {
    let mut f=Fixture::new().await;
    let order=f.order(true,60,true).await;
    let trade=f.trade(order,"PENDING_CONFIRMATION").await;
    let other=f.order(false,40,false).await;
    let before=f.snapshot().await;
    let mut tx=f.conn.begin().await.unwrap();
    let result=invalidate_organization(&mut tx,&f.command).await.unwrap();
    assert_eq!(result.withdrawn_orders,vec![order]);
    assert_eq!(result.preserved_trade_ids,vec![trade]);
    assert_eq!(result.released_uncommitted_quantity,Decimal::from(60));
    assert_eq!(result.outbox_ids.len(),1);
    tx.commit().await.unwrap();
    let after=f.snapshot().await;
    assert_eq!(after["trades"],before["trades"]);
    let row=sqlx::query("SELECT status,remaining_quantity_mt,version FROM orderbook_orders WHERE id=$1").bind(order).fetch_one(&mut f.conn).await.unwrap();
    assert_eq!(row.get::<String,_>("status"),"CANCELLED");
    assert_eq!(row.get::<Decimal,_>("remaining_quantity_mt"),Decimal::from(60));
    assert_eq!(row.get::<i32,_>("version"),2);
    let status:String=sqlx::query_scalar("SELECT status FROM orderbook_orders WHERE id=$1").bind(other).fetch_one(&mut f.conn).await.unwrap();
    assert_eq!(status,"OPEN");
    assert!(after["outbox"][0]["dispatched_at"].is_null());
    assert_eq!(after["outbox"][0]["payload"]["_public_market_invalidation"],true);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c03() {
    let mut f=Fixture::new().await;
    f.order(true,60,true).await;
    let mut tx=f.conn.begin().await.unwrap();
    invalidate_organization(&mut tx,&f.command).await.unwrap();
    tx.commit().await.unwrap();
    let row=sqlx::query("SELECT current_stock_mt,reserved_stock_mt FROM inventory_items").fetch_one(&mut f.conn).await.unwrap();
    assert_eq!(row.get::<Decimal,_>(0),Decimal::from(80));
    assert_eq!(row.get::<Decimal,_>(1),Decimal::from(20));
    let before=f.snapshot().await;
    let mut tx=f.conn.begin().await.unwrap();
    let result=invalidate_organization(&mut tx,&f.command).await.unwrap();
    assert!(result.withdrawn_orders.is_empty());
    tx.rollback().await.unwrap();
    assert_eq!(before,f.snapshot().await);
    f.order(true,30,true).await;
    let before=f.snapshot().await;
    let mut tx=f.conn.begin().await.unwrap();
    assert!(matches!(invalidate_organization(&mut tx,&f.command).await,Err(InvalidationError::Application(_))));
    tx.rollback().await.unwrap();
    assert_eq!(before,f.snapshot().await);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c04() {
    let mut f=Fixture::new().await;
    let order=f.order(true,0,false).await;
    for state in ["PENDING_CONFIRMATION","CONFIRMED","DELIVERED","PAID","CANCELLED","DECLINED"] { f.trade(order,state).await; }
    let before=f.snapshot().await;
    let mut tx=f.conn.begin().await.unwrap();
    let result=invalidate_organization(&mut tx,&f.command).await.unwrap();
    assert_eq!(result.preserved_trade_ids.len(),3);
    tx.commit().await.unwrap();
    assert_eq!(before["trades"],f.snapshot().await["trades"]);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c05() {
    use verdaxis_read::auth::lock::WriteConflict;
    let mut f=Fixture::new().await;
    let order=f.order(true,60,false).await;
    // A volatile lock wrapper changes the row exactly between preview and re-read.
    sqlx::raw_sql("CREATE FUNCTION pg_advisory_xact_lock(bigint) RETURNS void LANGUAGE plpgsql AS $$ BEGIN UPDATE orderbook_orders SET availability_window='2027-Q1'; END $$").execute(&mut f.conn).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET search_path TO {},pg_catalog",f.schema))).execute(&mut f.conn).await.unwrap();
    let before=f.snapshot().await;
    let mut tx=f.conn.begin().await.unwrap();
    let error=invalidate_organization(&mut tx,&f.command).await.unwrap_err();
    let attempt:verdaxis_read::business::transaction::AttemptError=error.into();
    assert!(attempt.is_write_conflict());
    tx.rollback().await.unwrap();
    assert_eq!(before,f.snapshot().await);
    let window:String=sqlx::query_scalar("SELECT availability_window FROM orderbook_orders WHERE id=$1").bind(order).fetch_one(&mut f.conn).await.unwrap();
    assert_eq!(window,"SPOT");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c07() {
    let mut f=Fixture::new().await;
    let order=f.order(false,40,false).await;
    let key=f.key();
    let mut tx=f.conn.begin().await.unwrap();
    let result=rebuild_live_slice_benchmarks_for_keys(&mut tx,&f.command.policy,&f.command.config,&[key.clone(),key.clone()],now()).await.unwrap();
    assert_eq!(result.rebuilt.len(),1);
    let read_key=LiveBenchmarkKey{side:"ASK".into(),product:"fixture-product".into(),point:f.point,window:"SPOT".into()};
    let read=live_benchmarks(&mut tx,&f.command.policy,&f.command.config,std::slice::from_ref(&read_key),now()).await.unwrap();
    assert_eq!(read[&read_key].price,Decimal::from(200));
    let row=sqlx::query("SELECT id,created_at,benchmark_price_per_mt_usd,total_remaining_quantity_mt,source FROM live_slice_benchmarks").fetch_one(&mut *tx).await.unwrap();
    let id:Uuid=row.get(0);
    assert_eq!(row.get::<Decimal,_>(3),Decimal::from(40));
    assert_eq!(row.get::<String,_>(4),"live_slice_vwap");
    sqlx::query("UPDATE orderbook_orders SET price_per_mt_usd=300 WHERE id=$1").bind(order).execute(&mut *tx).await.unwrap();
    rebuild_live_slice_benchmarks_for_keys(&mut tx,&f.command.policy,&f.command.config,std::slice::from_ref(&key),now()).await.unwrap();
    let updated=sqlx::query("SELECT id,created_at,benchmark_price_per_mt_usd FROM live_slice_benchmarks").fetch_one(&mut *tx).await.unwrap();
    assert_eq!(updated.get::<Uuid,_>(0),id);
    assert_eq!(updated.get::<DateTime<Utc>,_>(1),now());
    assert_eq!(updated.get::<Decimal,_>(2),Decimal::from(300));
    let mut null=key.clone(); null.delivery_point_id=None;
    let ignored=rebuild_live_slice_benchmarks_for_keys(&mut tx,&f.command.policy,&f.command.config,&[null],now()).await.unwrap();
    assert_eq!(ignored.ignored_null_points,1);
    assert!(ignored.rebuilt.is_empty());
    sqlx::query("UPDATE orderbook_orders SET expires_at=$1 WHERE id=$2").bind(now()).bind(order).execute(&mut *tx).await.unwrap();
    let deleted=rebuild_live_slice_benchmarks_for_keys(&mut tx,&f.command.policy,&f.command.config,&[key],now()).await.unwrap();
    assert_eq!(deleted.deleted.len(),1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM live_slice_benchmarks").fetch_one(&mut *tx).await.unwrap(),0);
    tx.rollback().await.unwrap();
    assert!(f.snapshot().await["benchmarks"].is_null());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c08() {
    for table in ["live_slice_benchmarks","watchlist_events","audit_logs","market_event_outbox"] {
        let mut f=Fixture::new().await;
        let order=f.order(true,60,true).await;
        f.order(false,40,false).await;
        sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code) VALUES($1,$2,'PIN',$3,'fixture-product',$4,'SPOT')")
            .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(order).bind(f.point).execute(&mut f.conn).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION fail_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture refusal' USING ERRCODE='40001'; END $$; CREATE TRIGGER fail_effect BEFORE INSERT OR UPDATE OR DELETE ON {table} FOR EACH STATEMENT EXECUTE FUNCTION fail_effect()"))).execute(&mut f.conn).await.unwrap();
        let before=f.snapshot().await;
        let mut tx=f.conn.begin().await.unwrap();
        let error=invalidate_organization(&mut tx,&f.command).await.unwrap_err();
        assert!(matches!(error,InvalidationError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("40001")),"{table}: {error:?}");
        tx.rollback().await.unwrap();
        assert_eq!(before,f.snapshot().await,"{table}");
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t7_c09() {
    let mut f=Fixture::new().await;
    for _ in 0..100 {f.order(true,0,false).await;}
    let mut tx=f.conn.begin().await.unwrap();
    let result=invalidate_organization(&mut tx,&f.command).await.unwrap();
    assert_eq!(result.withdrawn_orders.len(),100);
    tx.rollback().await.unwrap();
    let key=f.key();
    let mut bad=key.clone();bad.availability_window=Some("bad-window".into());
    let mut tx=f.conn.begin().await.unwrap();
    assert!(matches!(rebuild_live_slice_benchmarks_for_keys(&mut tx,&f.command.policy,&f.command.config,&[bad],now()).await,Err(InvalidationError::Application(_))));
    let empty=rebuild_live_slice_benchmarks_for_keys(&mut tx,&f.command.policy,&f.command.config,&[],now()).await.unwrap();
    assert!(empty.rebuilt.is_empty() && empty.deleted.is_empty());
    tx.rollback().await.unwrap();
    f.finish().await;
}
