//! The composed disposable schema, its role grants and the waiting primitives
//! every race in `phase4_races.rs` shares.
//!
//! One schema serves every business command. Its grants mirror
//! `deploy/postgres/app_acl_policy.sql`, so a statement the runtime role may not
//! run in production fails here too. A race waits on observed lock state (a
//! backend reported blocked by `pg_blocking_pids`), never on a sleep.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tokio::{sync::Notify, sync::Semaphore, task::JoinHandle};
use uuid::Uuid;
use verdaxis_read::{
    ApiError, Config, MarketPolicy,
    auth::{
        CurrentActor, VerificationSettings, VerifiedAccess,
        store::{self, NewRefreshRow},
        verify_access,
    },
    business::transaction::{
        AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan, run_ordered_write,
    },
};

const SIGNING_KEY: &str = "test-secret-key-for-testing-minimum-32-chars";

/// How long a race may wait for an observed condition before it reports a hang.
const OBSERVATION_LIMIT: Duration = Duration::from_secs(60);

/// A device digest shared by the sessions the suite seeds.
pub const DEVICE_HASH: &str = "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0";

pub fn access(user: Uuid) -> VerifiedAccess {
    access_issued_at(user, Utc::now())
}

/// A verified bearer token issued at `issued`, for the cutoff comparisons.
pub fn access_issued_at(user: Uuid, issued: DateTime<Utc>) -> VerifiedAccess {
    let claims = json!({
        "sub": user,
        "iat": issued.timestamp(),
        "exp": Utc::now().timestamp() + 3600,
        "type": "access",
        "iss": "test",
        "aud": "test",
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SIGNING_KEY.as_bytes()),
    )
    .expect("test token");
    verify_access(
        &token,
        &VerificationSettings::new(SIGNING_KEY.into(), None, "test".into(), "test".into()),
        Utc::now(),
    )
    .expect("test token verifies")
}

pub type Verdict = Result<(), ApiError>;
pub type Pending = JoinHandle<Verdict>;

/// Start one ordered business write on its own pooled connection and discard
/// its output: the races compare outcomes, not payloads.
pub fn spawn_write<C: BusinessCommand>(pool: &PgPool, actor: Uuid, command: C) -> Pending {
    let pool = pool.clone();
    tokio::spawn(async move {
        run_ordered_write(pool, Arc::new(Semaphore::new(1)), access(actor), command)
            .await
            .map(|_| ())
    })
}

pub async fn finished(task: Pending) -> Verdict {
    task.await.expect("the write task did not panic")
}

/// The refusal a domain rule returns: its status and stable code.
pub fn refusal(verdict: &Verdict) -> (u16, String) {
    let error = verdict.as_ref().expect_err("the command was refused");
    (
        error.status.as_u16(),
        error.detail["code"].as_str().unwrap_or_default().to_owned(),
    )
}

// ---------------------------------------------------------------------------
// Observed lock state.
// ---------------------------------------------------------------------------

/// Wait until at least `waiters` backends are blocked by `holder` and return
/// them. The condition is the lock state PostgreSQL reports, not elapsed time.
pub async fn wait_blocked(watcher: &mut PgConnection, holder: i32, waiters: usize) -> Vec<i32> {
    let deadline = Instant::now() + OBSERVATION_LIMIT;
    loop {
        let blocked: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) ORDER BY pid",
        )
        .bind(holder)
        .fetch_all(&mut *watcher)
        .await
        .expect("observe blocked backends");
        if blocked.len() >= waiters {
            return blocked;
        }
        assert!(
            Instant::now() < deadline,
            "{waiters} backend(s) never blocked on {holder}; saw {blocked:?}"
        );
        tokio::task::yield_now().await;
    }
}

/// Wait until at least `waiters` backends of this database wait for a lock.
/// Row-lock waiters queue behind one another, so a second waiter is reported as
/// blocked by the first rather than by the lock's holder.
pub async fn wait_blocked_backends(watcher: &mut PgConnection, waiters: usize) -> Vec<i32> {
    let deadline = Instant::now() + OBSERVATION_LIMIT;
    loop {
        let blocked: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE datname = current_database() \
             AND cardinality(pg_blocking_pids(pid)) > 0 ORDER BY pid",
        )
        .fetch_all(&mut *watcher)
        .await
        .expect("observe blocked backends");
        if blocked.len() >= waiters {
            return blocked;
        }
        assert!(
            Instant::now() < deadline,
            "{waiters} backend(s) never blocked; saw {blocked:?}"
        );
        tokio::task::yield_now().await;
    }
}

/// Wait until the backend no longer exists: the pooled connection that ran an
/// uncertain commit was closed, not returned for reuse.
pub async fn wait_backend_gone(watcher: &mut PgConnection, pid: i32) {
    let deadline = Instant::now() + OBSERVATION_LIMIT;
    loop {
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid = $1)")
                .bind(pid)
                .fetch_one(&mut *watcher)
                .await
                .expect("observe backends");
        if !present {
            return;
        }
        assert!(Instant::now() < deadline, "backend {pid} was never closed");
        tokio::task::yield_now().await;
    }
}

pub async fn backend_pid(connection: &mut PgConnection) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(connection)
        .await
        .expect("backend pid")
}

/// Pauses a command inside `execute`, after the ordered owner took every lock.
#[derive(Clone)]
pub struct Gate {
    entered: Arc<Notify>,
    released: Arc<Semaphore>,
    pids: Arc<Mutex<Vec<i32>>>,
}

impl Gate {
    pub fn closed() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            released: Arc::new(Semaphore::new(0)),
            pids: Arc::default(),
        }
    }

    /// Wait for the paused command and return its backend pid.
    pub async fn arrived(&self) -> i32 {
        tokio::time::timeout(OBSERVATION_LIMIT, self.entered.notified())
            .await
            .expect("the gated command reached execute");
        *self
            .pids
            .lock()
            .unwrap()
            .last()
            .expect("the gated command recorded its backend")
    }

    /// Let every present and later attempt of the gated command continue.
    pub fn open(&self) {
        self.released.add_permits(Semaphore::MAX_PERMITS / 2);
    }

    /// How many attempts reached `execute`.
    pub fn attempts(&self) -> usize {
        self.pids.lock().unwrap().len()
    }

    /// The backend of every attempt, oldest first.
    pub fn backends(&self) -> Vec<i32> {
        self.pids.lock().unwrap().clone()
    }
}

pub struct Gated<C> {
    inner: C,
    gate: Gate,
}

impl<C> Gated<C> {
    pub fn new(inner: C, gate: &Gate) -> Self {
        Self {
            inner,
            gate: gate.clone(),
        }
    }
}

impl<C: BusinessCommand> BusinessCommand for Gated<C> {
    type Output = C::Output;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        self.inner.preview(conn)
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move {
            let pid = backend_pid(&mut *conn).await;
            self.gate.pids.lock().unwrap().push(pid);
            self.gate.entered.notify_one();
            // The permit returns on drop, so a retried attempt passes as well.
            drop(
                self.gate
                    .released
                    .acquire()
                    .await
                    .expect("the gate semaphore stays open"),
            );
            self.inner.execute(conn, actor).await
        })
    }
}

// ---------------------------------------------------------------------------
// The schema and its rows.
// ---------------------------------------------------------------------------

/// One executable listing: an open order and the inventory it reserves.
pub struct Listing {
    pub order: Uuid,
    pub inventory: Uuid,
}

pub struct World {
    pub pool: PgPool,
    /// The schema owner: seeds rows and installs fault injection.
    pub admin: PgConnection,
    /// Observes lock state; never holds a lock itself.
    pub watcher: PgConnection,
    pub schema: String,
    pub runtime_url: String,
    /// An approved administrator without an organization.
    pub actor: Uuid,
    pub product: Uuid,
    pub point: Uuid,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
}

impl World {
    pub async fn new() -> Self {
        let migrator_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL required");
        let runtime_url = std::env::var("RUST_TEST_DATABASE_URL")
            .expect("explicit disposable runtime URL required");
        let migrator = url::Url::parse(&migrator_url).unwrap();
        let runtime = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(migrator.host_str(), Some("127.0.0.1"));
        assert!(!matches!(migrator.port(), Some(5432 | 8000 | 8001)));
        assert!(migrator.path().ends_with("_test"));
        assert_eq!(runtime.host_str(), migrator.host_str());
        assert_eq!(runtime.port(), migrator.port());
        assert_eq!(runtime.path(), migrator.path());
        assert_eq!(runtime.username(), "rust_app");
        assert_ne!(runtime.username(), migrator.username());

        let schema = format!("p4r_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&migrator_url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../fixtures/phase4_races_schema.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        // The runtime role's authority, as the ACL policy declares it: full DML
        // on the working tables, append-only history, a fixed set of organization
        // columns, and read-only market catalog and grace tables.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app; \
             GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO rust_app; \
             REVOKE UPDATE,DELETE ON {schema}.audit_logs,{schema}.user_status_transitions,{schema}.kyc_documents FROM rust_app; \
             REVOKE DELETE ON {schema}.market_event_outbox,{schema}.organization_join_requests FROM rust_app; \
             REVOKE INSERT,UPDATE ON {schema}.organizations FROM rust_app; \
             GRANT INSERT(id,name,domain,type,tax_id,country_code,created_at,provenance) ON {schema}.organizations TO rust_app; \
             GRANT UPDATE(name,domain,type,tax_id,country_code,verification_status) ON {schema}.organizations TO rust_app; \
             REVOKE INSERT,UPDATE,DELETE ON {schema}.kyc_execution_grace,{schema}.products,{schema}.delivery_points FROM rust_app"
        )))
        .execute(&mut admin)
        .await
        .unwrap();

        let search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .after_connect({
                let search_path = search_path.clone();
                move |connection, _| {
                    let search_path = search_path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(search_path))
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                }
            })
            .connect(&runtime_url)
            .await
            .unwrap();
        let mut watcher = PgConnection::connect(&migrator_url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(search_path))
            .execute(&mut watcher)
            .await
            .unwrap();

        let actor = Uuid::new_v4();
        let product = Uuid::new_v4();
        let point = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,role) VALUES($1,'ADMIN')")
            .bind(actor)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO products(id,name,fuel_type,fuel_grade) VALUES($1,'fixture-product','Methanol','fixture')")
            .bind(product)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO delivery_points(id,name,region) VALUES($1,'fixture-point','fixture-region')")
            .bind(point)
            .execute(&mut admin)
            .await
            .unwrap();
        let policy: MarketPolicy = serde_json::from_value(json!({
            "source_sha": "race-fixture",
            "products": [{
                "id": product, "code": "fixture-product", "name": "fixture-product",
                "fuel_type": "Methanol", "fuel_grade": "fixture", "execution_mode": "ORDERBOOK",
                "lanes": [point], "qualification_profile": "scalar", "display_order": 1
            }],
            "points": [{
                "id": point, "name": "fixture-point", "region": "fixture-region",
                "timezone": "UTC", "display_order": 1
            }],
        }))
        .unwrap();
        let config: Config = serde_json::from_value(json!({
            "kyc_execution_gate_enabled": true, "environment": "test",
            "release_sha": verdaxis_read::compiled_sha(), "migration_revision": "race",
            "database_url": "postgres://race@127.0.0.1:1/race", "db_session_time_zone": "UTC",
            "bind": "127.0.0.1:0", "pool_max": 5, "python_budget": 1,
            "configured_max_connections": 100
        }))
        .unwrap();
        Self {
            pool,
            admin,
            watcher,
            schema,
            runtime_url,
            actor,
            product,
            point,
            policy: Arc::new(policy),
            config: Arc::new(config),
        }
    }

    pub async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.admin)
        .await
        .unwrap();
    }

    /// A dedicated runtime-role connection: the stand-in for a writer outside
    /// the Rust pool (a Python worker or timer), with the same authority.
    pub async fn runtime_connection(&self) -> PgConnection {
        let mut connection = PgConnection::connect(&self.runtime_url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "SET search_path TO {}",
            self.schema
        )))
        .execute(&mut connection)
        .await
        .unwrap();
        connection
    }

    // -- rows ---------------------------------------------------------------

    /// An approved, real buy-side organization.
    pub async fn organization(&mut self) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id,verification_status,provenance) VALUES($1,'APPROVED','REAL')")
            .bind(id)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    /// An organization still waiting for its review.
    pub async fn pending_organization(&mut self) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id,provenance) VALUES($1,'REAL')")
            .bind(id)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    /// An approved, verified buyer account whose KYC review is still pending.
    pub async fn user(&mut self, organization: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,organization_id,role) VALUES($1,$2,'BUYER')")
            .bind(id)
            .bind(organization)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    pub async fn set_status(&mut self, user: Uuid, status: &str) {
        sqlx::query("UPDATE users SET status=$2 WHERE id=$1")
            .bind(user)
            .bind(status)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    pub async fn bind_kyc(&mut self, user: Uuid, status: &str, organization: Option<Uuid>) {
        sqlx::query("UPDATE users SET kyc_status=$2,kyc_organization_id=$3 WHERE id=$1")
            .bind(user)
            .bind(status)
            .bind(organization)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    pub async fn join_request(&mut self, user: Uuid, organization: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organization_join_requests(id,user_id,organization_id) VALUES($1,$2,$3)",
        )
        .bind(id)
        .bind(user)
        .bind(organization)
        .execute(&mut self.admin)
        .await
        .unwrap();
        id
    }

    /// An open order with 60 MT remaining over an inventory of 20 MT available
    /// and 80 MT reserved: 60 MT of the reservation is uncommitted.
    pub async fn stocked_listing(&mut self, organization: Uuid) -> Listing {
        let inventory = Uuid::new_v4();
        let order = Uuid::new_v4();
        sqlx::query("INSERT INTO inventory_items VALUES($1,20,80)")
            .bind(inventory)
            .execute(&mut self.admin)
            .await
            .unwrap();
        sqlx::query(include_str!(
            "../fixtures/market_invalidation_order_insert.sql"
        ))
        .bind(order)
        .bind(organization)
        .bind(self.product)
        .bind(self.point)
        .bind(Decimal::from(60))
        .bind(Some(inventory))
        .execute(&mut self.admin)
        .await
        .unwrap();
        Listing { order, inventory }
    }

    /// A pre-existing trade whose participants are fixed at creation.
    pub async fn trade(
        &mut self,
        (buyer_org, buyer_user): (Uuid, Uuid),
        (seller_org, seller_user): (Uuid, Uuid),
        ask_order: Option<Uuid>,
        status: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO trades(id,buyer_id,seller_id,ask_order_id,product_id,delivery_point_id,availability_window,status,quantity_mt,buyer_user_id,seller_user_id,fee_snapshot,terms_snapshot) VALUES($1,$2,$3,$4,$5,$6,'SPOT',$7,20,$8,$9,$10,$11)")
            .bind(id)
            .bind(buyer_org)
            .bind(seller_org)
            .bind(ask_order)
            .bind(self.product)
            .bind(self.point)
            .bind(status)
            .bind(buyer_user)
            .bind(seller_user)
            .bind(json!({"rate": 12.5}))
            .bind(json!({"retained": true}))
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    /// A live refresh session; returns the digest of its token id.
    pub async fn live_session(&mut self, user: Uuid) -> String {
        self.session(user, Utc::now() + chrono::Duration::days(7))
            .await
    }

    /// A refresh session that expires at `expires_at`.
    pub async fn session(&mut self, user: Uuid, expires_at: DateTime<Utc>) -> String {
        let jti_hash = format!(
            "{:032x}{:032x}",
            Uuid::new_v4().as_u128(),
            Uuid::new_v4().as_u128()
        );
        store::insert_refresh(
            &mut self.admin,
            &NewRefreshRow {
                user_id: user,
                family_id: Uuid::new_v4(),
                jti_hash: jti_hash.clone(),
                device_id_hash: DEVICE_HASH.into(),
                expires_at,
            },
        )
        .await
        .unwrap();
        jti_hash
    }

    /// An open order without an inventory reservation.
    pub async fn open_order(&mut self, organization: Uuid) -> Uuid {
        let order = Uuid::new_v4();
        sqlx::query(include_str!(
            "../fixtures/market_invalidation_order_insert.sql"
        ))
        .bind(order)
        .bind(organization)
        .bind(self.product)
        .bind(self.point)
        .bind(Decimal::from(60))
        .bind(Option::<Uuid>::None)
        .execute(&mut self.admin)
        .await
        .unwrap();
        order
    }

    /// An approved supplier-side organization, for supplier invitations.
    pub async fn supplier_organization(&mut self) -> Uuid {
        let id = self.organization().await;
        sqlx::query("UPDATE organizations SET type='FUEL_SUPPLIER' WHERE id=$1")
            .bind(id)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    // -- observation --------------------------------------------------------

    pub async fn scalar_text(&self, sql: &'static str, id: Uuid) -> Option<String> {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn count_for(&self, sql: &'static str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn events(&self, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE event_type=$1")
            .bind(event_type)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// Available and reserved stock of one inventory item.
    pub async fn stock(&self, inventory: Uuid) -> (Decimal, Decimal) {
        sqlx::query_as("SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1")
            .bind(inventory)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// Every table the commands can write, as one JSON document. Two equal
    /// snapshots mean no row was added, changed or removed in between.
    pub async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_object(\
             'organizations',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM organizations t),\
             'users',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM users t),\
             'joins',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM organization_join_requests t),\
             'audit',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM audit_logs t),\
             'history',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM user_status_transitions t),\
             'sessions',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM refresh_sessions t),\
             'documents',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM kyc_documents t),\
             'referrals',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM referrals t),\
             'subscriptions',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM subscriptions t),\
             'orders',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM orderbook_orders t),\
             'trades',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM trades t),\
             'stock',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM inventory_items t),\
             'benchmarks',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM live_slice_benchmarks t),\
             'watchlist',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM watchlist_events t),\
             'outbox',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]') FROM market_event_outbox t))",
        )
        .fetch_one(&mut self.admin)
        .await
        .expect("snapshot")
    }
}
