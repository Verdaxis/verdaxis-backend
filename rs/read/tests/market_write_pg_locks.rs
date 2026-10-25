//! Per-operation acquisition and locked admission. No serving route is enabled.
#[allow(dead_code)]
#[path = "../src/order_write/authority.rs"]
mod authority;
#[allow(dead_code)]
#[path = "../src/order_write/error.rs"]
mod error;
#[allow(dead_code)]
#[path = "../src/order_write/locks.rs"]
mod locks;
use authority::admission;

mod auth {
    pub use verdaxis_read::auth::*;
}
mod market {
    pub use verdaxis_read::market::windows;
}
pub use verdaxis_read::{ApiError, ApiErrorKind};

use admission::{AdmissionClass, PartyRef};
use authority::{ActorRole, TradeSide};
use chrono::{Duration, Utc};
use locks::{LockClass, MarketOperation, OperationLockPlan, PartyLockPreview, SliceKey};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn preview(actor: Uuid) -> PartyLockPreview {
    PartyLockPreview::new(
        [id(20), id(10)],
        [id(2), id(1)],
        actor,
        serde_json::json!({"revision":1}),
    )
}
fn slice() -> SliceKey {
    SliceKey {
        product_id: id(100),
        delivery_point_id: None,
        availability_window: "SPOT".into(),
    }
}

#[test]
fn rust_pair_writers_use_org_then_sorted_users_before_slice() {
    for actor in [id(1), id(2)] {
        let mut plan = OperationLockPlan::new(MarketOperation::Cancel);
        plan.parties(&preview(actor)).unwrap();
        plan.slices([slice()]).unwrap();
        assert_eq!(
            plan.trace(),
            vec![
                (LockClass::Organization, id(10).to_string()),
                (LockClass::Organization, id(20).to_string()),
                (LockClass::User, id(1).to_string()),
                (LockClass::User, id(2).to_string()),
                (
                    LockClass::Slice,
                    locks::market_slice_lock_key(id(100), None, "SPOT").to_string()
                ),
            ]
        );
        assert!(
            plan.users([id(3)]).is_err(),
            "a party cannot appear after a slice"
        );
    }
}

#[test]
fn lock_plan_matches_the_declared_operation_graph() {
    use LockClass::*;
    use MarketOperation::*;
    for (op, graph) in [
        (
            Create,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Capability,
                Context,
                Authorization,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (
            Amend,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (
            Cancel,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Capability,
                Context,
                Authorization,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (InventoryCreate, vec![Organization, User, Idempotency]),
        (InventoryEdit, vec![Organization, User, Inventory]),
        (
            Publish,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (
            Take,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (
            Confirm,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Trade,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (
            Decline,
            vec![
                Organization,
                User,
                Idempotency,
                Slice,
                Trade,
                Order,
                Inventory,
                Pin,
            ],
        ),
        (Deliver, vec![Organization, User, Idempotency, Trade]),
        (Pay, vec![Organization, User, Idempotency, Trade]),
        (CapabilityGrant, vec![User, Capability]),
        (CapabilityRevoke, vec![User, Capability, Context]),
        (ContextStart, vec![Organization, User, Capability, Context]),
        (ContextExit, vec![Organization, User, Capability, Context]),
        (
            MarketOperation::Commission,
            vec![User, LockClass::Commission],
        ),
        (Freeze, vec![User, Trade, FreezeRow]),
    ] {
        assert_eq!(op.graph(), graph, "{op:?}");
    }
    let mut plan = OperationLockPlan::new(Cancel);
    assert!(
        plan.slices([slice()]).is_err(),
        "complete parties must precede slices"
    );
}

struct Fixture {
    admin: PgConnection,
    pool: sqlx::PgPool,
    schema: String,
}
impl Fixture {
    async fn open() -> Self {
        let admin_url =
            std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").expect("disposable migrator URL");
        let runtime_url = std::env::var("RUST_TEST_DATABASE_URL").expect("disposable runtime URL");
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert_eq!(r.host_str(), a.host_str());
        assert_eq!(r.port(), a.port());
        assert_eq!(r.path(), a.path());
        assert!(a.path().ends_with("_test"));
        assert_eq!(r.username(), "rust_app");
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        let schema = format!("p89_l_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_locks.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT,UPDATE ON ALL TABLES IN SCHEMA {schema} TO rust_app"))).execute(&mut admin).await.unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&runtime_url)
            .await
            .unwrap();
        Self {
            admin,
            pool,
            schema,
        }
    }
    async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.admin)
        .await
        .unwrap();
    }
    async fn access(&self, actor: Uuid) -> auth::VerifiedAccess {
        let now = Utc::now();
        let secret = "disposable-fixture-signing-material-at-least-32-bytes";
        let settings = auth::VerificationSettings::new(
            secret.into(),
            None,
            "fixture".into(),
            "fixture".into(),
        );
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &serde_json::json!({
            "sub":actor,"type":"access","iss":"fixture","aud":"fixture","iat":now.timestamp(),"exp":(now+Duration::hours(1)).timestamp()
        }), &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes())).unwrap();
        auth::verify_access(&token, &settings, now).unwrap()
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn timeout_prefix_is_the_first_statement() {
    let f = Fixture::open().await;
    let mut conn = f.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let mut plan = OperationLockPlan::new(MarketOperation::Deliver);
    plan.parties(&preview(id(1))).unwrap();
    let locked = plan.acquire(&mut tx).await.unwrap();
    assert_eq!(
        locked.statement_trace()[..2],
        [
            "SET LOCAL lock_timeout = '500ms'",
            "SET LOCAL statement_timeout = '5s'"
        ]
    );
    let row = sqlx::query(
        "SELECT current_setting('lock_timeout') AS l,current_setting('statement_timeout') AS s",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("l"), "500ms");
    assert_eq!(row.get::<String, _>("s"), "5s");
    tx.rollback().await.unwrap();
    drop(conn);
    f.finish().await;
}

async fn cleanup(actor: Uuid, side: TradeSide) -> Result<(), error::MarketWriteError> {
    let f = Fixture::open().await;
    let access = f.access(actor).await;
    let mut conn = f.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let mut plan = OperationLockPlan::new(MarketOperation::Deliver);
    plan.parties(&preview(actor)).unwrap();
    plan.trade(id(30)).unwrap();
    let locked = plan.acquire(&mut tx).await.unwrap();
    let result = admission::cleanup_context_for_trade(&mut tx, &locked, access, id(30), side)
        .await
        .map(|_| ());
    tx.rollback().await.unwrap();
    drop(conn);
    f.finish().await;
    result
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn cleanup_admission_survives_rejection() {
    cleanup(id(1), TradeSide::Buyer).await.unwrap();
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn cleanup_rejects_a_stranger_and_the_counterparty() {
    for actor in [id(3), id(2)] {
        let error = cleanup(actor, TradeSide::Buyer).await.unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
    }
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn cleanup_rejects_a_different_member_of_the_same_organization() {
    assert_eq!(
        cleanup(id(4), TradeSide::Buyer).await.unwrap_err().status,
        axum::http::StatusCode::FORBIDDEN
    );
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn execution_admission_requires_approved_kyc_or_grace() {
    let mut f = Fixture::open().await;
    for (status, bound, grace, gate, expected) in [
        ("APPROVED", true, false, true, true),
        ("PENDING", true, false, true, false),
        ("PENDING", true, true, true, true),
        ("REJECTED", true, true, true, false),
        ("APPROVED", false, false, true, false),
        ("PENDING", false, false, false, true),
        ("REJECTED", true, true, false, false),
        ("APPROVED", false, false, false, false),
    ] {
        sqlx::query("UPDATE users SET status='APPROVED',email_verified=true,must_change_password=false,authentication_revoked_at=NULL,organization_id=$1,kyc_status=$2,kyc_organization_id=$3 WHERE id=$4")
            .bind(id(10)).bind(status).bind(if bound { Some(id(10)) } else if status=="PENDING" { None } else { Some(id(20)) }).bind(id(1)).execute(&mut f.admin).await.unwrap();
        sqlx::query("UPDATE kyc_execution_grace SET expires_at=$1")
            .bind(if grace {
                Utc::now() + Duration::hours(1)
            } else {
                Utc::now() - Duration::hours(1)
            })
            .execute(&mut f.admin)
            .await
            .unwrap();
        let mut conn = f.pool.acquire().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        let mut plan = OperationLockPlan::new(MarketOperation::Create);
        plan.parties(&preview(id(1))).unwrap();
        let locked = plan.acquire(&mut tx).await.unwrap();
        let result = admission::require_execution(
            &mut tx,
            &locked,
            &PartyRef {
                user_id: id(1),
                organization_id: id(10),
                role: ActorRole::Buyer,
            },
            Utc::now(),
            gate,
        )
        .await;
        assert_eq!(result.is_ok(), expected, "{status}/{bound}/{grace}/{gate}");
        tx.rollback().await.unwrap();
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn revocation_cutoff_is_skipped_only_for_rejected() {
    let mut f = Fixture::open().await;
    for (status, password, expected) in [
        ("REJECTED", false, true),
        ("PENDING", false, false),
        ("APPROVED", false, false),
        ("REJECTED", true, false),
    ] {
        let access = f.access(id(1)).await;
        sqlx::query("UPDATE users SET status=$1,authentication_revoked_at=$2,password_changed_at=$3 WHERE id=$4")
            .bind(status).bind(Utc::now()+Duration::minutes(1)).bind(password.then(||Utc::now()+Duration::minutes(1))).bind(id(1)).execute(&mut f.admin).await.unwrap();
        let mut conn = f.pool.acquire().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        let mut plan = OperationLockPlan::new(MarketOperation::Deliver);
        plan.parties(&preview(id(1))).unwrap();
        let locked = plan.acquire(&mut tx).await.unwrap();
        let result =
            admission::authenticate(&mut tx, &locked, access, AdmissionClass::Cleanup).await;
        assert_eq!(result.is_ok(), expected, "{status}/{password}");
        tx.rollback().await.unwrap();
    }
    f.finish().await;
}

#[test]
fn preview_drift_restarts_without_late_party_locks() {
    let first = preview(id(1));
    let changed = PartyLockPreview::new(
        [id(10), id(20)],
        [id(1), id(2), id(3)],
        id(1),
        serde_json::json!({"revision":1}),
    );
    assert!(!first.matches(&changed));
    assert_eq!(first.users(), &[id(1), id(2)]);
}

#[test]
fn rust_lock_plan_names_every_row_it_locks() {
    let source = include_str!("../src/order_write/locks.rs");
    for table in [
        "organizations",
        "users",
        "trades",
        "orderbook_orders",
        "inventory_items",
    ] {
        assert!(source.contains(table), "missing named resource {table}");
    }
    assert!(source.contains("LockClass::Pin"));
}

#[test]
fn lock_failures_keep_their_operation_class() {
    for (state, class, code) in [
        (
            "55P03",
            LockClass::Idempotency,
            error::MarketErrorCode::IdempotencyBusy,
        ),
        (
            "57014",
            LockClass::Idempotency,
            error::MarketErrorCode::MarketTimeout,
        ),
        (
            "55P03",
            LockClass::Slice,
            error::MarketErrorCode::MarketBusy,
        ),
        (
            "57014",
            LockClass::Slice,
            error::MarketErrorCode::MarketTimeout,
        ),
    ] {
        let failure = locks::lock_failure_for_test(state, class);
        assert_eq!(failure.code, code);
        assert!(!failure.is_retryable());
    }
}

#[test]
fn continuing_python_writers_keep_their_contracts() {
    let reminder = include_str!("../../../app/services/order_expiry_reminders.py");
    assert!(reminder.contains("8246192048"));
    assert!(reminder.contains("timedelta(minutes=15)"));
    for key in [
        "order_expiring",
        "order_id",
        "order_key",
        "_email_payload",
        "_email_retry_at",
        "_email_deadline",
    ] {
        assert!(reminder.contains(key));
    }
    let phase7 = include_str!("../../../app/routers/negotiations.py");
    assert!(phase7.contains("acquire_market_slice_locks"));
    assert!(phase7.contains("OrderBookOrder"));
    assert!(include_str!("../../../app/seeds/market_seed.py").contains("Trade("));
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn continuing_writer_user_prefix_has_a_bounded_outcome() {
    let f = Fixture::open().await;
    let mut python = f.pool.acquire().await.unwrap();
    let mut python_tx = python.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(id(2))
        .fetch_one(&mut *python_tx)
        .await
        .unwrap();
    let mut rust = f.pool.acquire().await.unwrap();
    let mut rust_tx = rust.begin().await.unwrap();
    let mut plan = OperationLockPlan::new(MarketOperation::Cancel);
    plan.parties(&preview(id(1))).unwrap();
    plan.slices([slice()]).unwrap();
    let error = plan.acquire(&mut rust_tx).await.unwrap_err();
    assert_eq!(error.code, error::MarketErrorCode::MarketBusy);
    assert!(!error.is_retryable());
    rust_tx.rollback().await.unwrap();
    python_tx.rollback().await.unwrap();
    drop(rust);
    drop(python);
    f.finish().await;
}

#[test]
fn maintenance_and_phase7_sources_keep_canonical_locks_and_columns() {
    let quarantine = include_str!("../../../app/services/market_quarantine.py");
    assert!(
        quarantine.contains("Acquire maintenance idempotency then canonical sorted slice locks")
    );
    assert!(quarantine.contains("SET LOCAL lock_timeout = '500ms'"));
    assert!(quarantine.contains("SET LOCAL statement_timeout = '5s'"));
    assert!(quarantine.contains("market_slice_lock_key("));
    let demo = include_str!("../../../app/services/demo_activity.py");
    assert!(demo.contains("acquire_market_slice_locks("));
    assert!(demo.contains("remaining_quantity_mt"));
    assert!(demo.contains("trades = await match_order("));
    let matcher = include_str!("../../../app/services/matching_engine.py");
    for column in ["buyer_user_id", "seller_user_id"] {
        assert!(matcher.contains(column));
    }
    let seed = include_str!("../../../app/seeds/market_seed.py");
    for column in ["bid_order_id", "ask_order_id"] {
        assert!(seed.contains(column));
    }
    for source in [
        include_str!("../../../app/routers/rfq.py"),
        include_str!("../../../app/routers/supplier_offers.py"),
    ] {
        assert!(source.contains("acquire_market_slice_lock"));
    }
}

#[cfg(feature = "contract-tests")]
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn maintenance_slice_holder_bounds_rust_without_late_party_locks() {
    let f = Fixture::open().await;
    let mut maintenance = f.pool.acquire().await.unwrap();
    let mut maintenance_tx = maintenance.begin().await.unwrap();
    // Existing maintenance and Phase 7 writers use this exact shared identity.
    maintenance_probe::hold_slice(
        &mut maintenance_tx,
        locks::market_slice_lock_key(id(100), None, "SPOT"),
    )
    .await;

    let mut rust = f.pool.acquire().await.unwrap();
    let mut rust_tx = rust.begin().await.unwrap();
    let mut rust_plan = OperationLockPlan::new(MarketOperation::Publish);
    rust_plan.parties(&preview(id(1))).unwrap();
    rust_plan.slices([slice()]).unwrap();
    let error = rust_plan.acquire(&mut rust_tx).await.unwrap_err();
    assert_eq!(error.code, error::MarketErrorCode::MarketBusy);
    rust_tx.rollback().await.unwrap();
    maintenance_tx.rollback().await.unwrap();
    drop(rust);
    drop(maintenance);
    f.finish().await;
}

#[cfg(feature = "contract-tests")]
mod maintenance_probe {
    pub async fn hold_slice(conn: &mut sqlx::PgConnection, key: i64) {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(key)
            .execute(conn)
            .await
            .unwrap();
    }
}

#[cfg(feature = "contract-tests")]
#[allow(dead_code)]
#[path = "../src/order_write/command.rs"]
mod command;
#[cfg(feature = "contract-tests")]
#[path = "../src/order_write/transaction.rs"]
mod transaction;
#[cfg(feature = "contract-tests")]
use authority::AuthenticatedActor;
#[cfg(feature = "contract-tests")]
use error::{MarketErrorCode, MarketWriteError};
#[cfg(feature = "contract-tests")]
use verdaxis_read::AppState;

#[cfg(feature = "contract-tests")]
mod ordered_cases {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Command {
        preview: Arc<AtomicUsize>,
        executions: Arc<AtomicUsize>,
        state: &'static str,
        acquisition: bool,
        always: bool,
        drift: bool,
    }
    impl transaction::MarketCommand for Command {
        type Output = ();
        fn preview<'a>(
            &'a self,
            _conn: &'a mut PgConnection,
            actor: Uuid,
        ) -> transaction::BoxFuture<
            'a,
            Result<transaction::MarketPreview, transaction::MarketAttemptError>,
        > {
            Box::pin(async move {
                let count = self.preview.fetch_add(1, Ordering::SeqCst);
                let parties = PartyLockPreview::new(
                    [id(10), id(20)],
                    [id(1), id(2)],
                    actor,
                    serde_json::json!({"generation":if self.drift && count==1 {1}else{0}}),
                );
                let mut locks = OperationLockPlan::new(MarketOperation::Cancel);
                locks.parties(&parties)?;
                locks.slices([slice()])?;
                if self.acquisition {
                    locks.orders([id(90)])?;
                }
                Ok(transaction::MarketPreview { parties, locks })
            })
        }
        fn execute<'a>(
            &'a self,
            tx: &'a mut transaction::LockedMarketTransaction<'_>,
            _actor: &'a AuthenticatedActor,
        ) -> transaction::BoxFuture<'a, Result<(), transaction::MarketAttemptError>> {
            Box::pin(async move {
                let n = self.executions.fetch_add(1, Ordering::SeqCst);
                tx.test_stage_effect().await?;
                if !self.acquisition && (n == 0 || self.always) && !self.state.is_empty() {
                    tx.test_server_abort(self.state).await?;
                }
                Ok(())
            })
        }
    }
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL"]
    async fn ordered_owner_preserves_server_abort_classes() {
        for acquisition in [false, true] {
            for state in ["40001", "40P01", "55P03", "57014"] {
                let mut f = Fixture::open().await;
                sqlx::query("UPDATE users SET status='APPROVED',authentication_revoked_at=NULL,must_change_password=false WHERE id=$1").bind(id(1)).execute(&mut f.admin).await.unwrap();
                if acquisition {
                    transaction::install_acquisition_abort(&mut f.admin, state).await;
                }
                let previews = Arc::new(AtomicUsize::new(0));
                let executions = Arc::new(AtomicUsize::new(0));
                let result = transaction::run_market_command(
                    f.pool.clone(),
                    Arc::new(tokio::sync::Semaphore::new(1)),
                    f.access(id(1)).await,
                    Command {
                        preview: previews.clone(),
                        executions: executions.clone(),
                        state,
                        acquisition,
                        always: false,
                        drift: false,
                    },
                    AdmissionClass::Cleanup,
                )
                .await;
                let retry = matches!(state, "40001" | "40P01");
                assert_eq!(result.is_ok(), retry, "{state}/{acquisition}");
                assert_eq!(
                    previews.load(Ordering::SeqCst),
                    if retry {
                        if acquisition { 3 } else { 4 }
                    } else {
                        if acquisition { 1 } else { 2 }
                    }
                );
                let effects: i64 =
                    sqlx::query_scalar("SELECT value::bigint FROM lock_effects WHERE id=1")
                        .fetch_one(&mut f.admin)
                        .await
                        .unwrap();
                assert_eq!(effects, if retry { 1 } else { 0 });
                f.finish().await;
            }
        }
    }
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL"]
    async fn ordered_owner_bounds_abort_and_rebuilds_changed_preview() {
        let mut f = Fixture::open().await;
        sqlx::query("UPDATE users SET status='APPROVED',authentication_revoked_at=NULL,must_change_password=false WHERE id=$1").bind(id(1)).execute(&mut f.admin).await.unwrap();
        for (state, drift, expected) in [
            ("40001", false, false),
            ("40P01", false, false),
            ("", true, true),
        ] {
            let previews = Arc::new(AtomicUsize::new(0));
            let executions = Arc::new(AtomicUsize::new(0));
            let result = transaction::run_market_command(
                f.pool.clone(),
                Arc::new(tokio::sync::Semaphore::new(1)),
                f.access(id(1)).await,
                Command {
                    preview: previews.clone(),
                    executions: executions.clone(),
                    state,
                    acquisition: false,
                    always: true,
                    drift,
                },
                AdmissionClass::Cleanup,
            )
            .await;
            assert_eq!(result.is_ok(), expected);
            assert_eq!(previews.load(Ordering::SeqCst), 4);
            assert_eq!(executions.load(Ordering::SeqCst), if drift { 1 } else { 2 });
        }
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn assisted_context_uses_locked_customer_authority() {
    let mut f = Fixture::open().await;
    for (change, expected) in [
        ("valid", true),
        ("member_rejected", false),
        ("member_absent", false),
        ("wrong_org", false),
        ("unknown_provenance", false),
        ("revoked_capability", false),
        ("expired_context", false),
    ] {
        admission::test_seed_assisted(&mut f.admin, change).await;
        let mut conn = f.pool.acquire().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        let parties = PartyLockPreview::new(
            [id(10), id(20)],
            [id(1), id(2), id(4)],
            id(1),
            serde_json::Value::Null,
        );
        let mut plan = OperationLockPlan::new(MarketOperation::Create);
        plan.parties(&parties).unwrap();
        plan.resources(LockClass::Capability, [id(60), id(61)])
            .unwrap();
        plan.resources(LockClass::Context, [id(62)]).unwrap();
        let locked = plan.acquire(&mut tx).await.unwrap();
        let actor = admission::authenticate(
            &mut tx,
            &locked,
            f.access(id(1)).await,
            AdmissionClass::Execution,
        )
        .await
        .unwrap();
        let result = admission::execution_context(
            &mut tx,
            &locked,
            actor,
            id(10),
            Some(id(62)),
            Utc::now(),
            true,
        )
        .await;
        assert_eq!(result.is_ok(), expected, "{change}");
        if let Ok(ctx) = result {
            assert_eq!(ctx.effective_organization_id(), id(10));
            assert_eq!(ctx.support_context_id(), Some(id(62)));
        }
        tx.rollback().await.unwrap();
    }
    f.finish().await;
}


#[cfg(feature = "contract-tests")]
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn continuing_python_protocols_run_against_ordered_rust() {
    let binary=std::env::current_exe().unwrap();
    let status=std::process::Command::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../venv/bin/python"))
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["rs/read/tests/fixtures/phase8_writer_protocols.py"])
        .env("RUST_LOCK_PROTOCOL_BINARY",binary).status().unwrap();
    assert!(status.success(),"each continuing writer must execute its database protocol");
}

#[cfg(feature = "contract-tests")]
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn ordered_protocol_contender() {
    let Ok(url)=std::env::var("RUST_PROTOCOL_DATABASE_URL") else {return;};
    let parsed=url::Url::parse(&url).unwrap();assert_eq!(parsed.host_str(),Some("127.0.0.1"));assert!(parsed.path().ends_with("_test"));
    let mut conn=PgConnection::connect(&url).await.unwrap();
    let (organizations,users,orders,slices)=transaction::protocol_preview(&mut conn).await;
    let actor=users.first().copied().unwrap_or_else(||id(1));
    if users.is_empty() {println!("PROTOCOL_EMPTY");return;}
    let parties=PartyLockPreview::new(organizations,users,actor,serde_json::Value::Null);
    let mut plan=OperationLockPlan::new(MarketOperation::Cancel);plan.parties(&parties).unwrap();plan.slices(slices).unwrap();plan.orders(orders).unwrap();
    let mut tx=conn.begin().await.unwrap();
    match plan.acquire(&mut tx).await {
        Ok(_)=>println!("PROTOCOL_SERIAL"),
        Err(error)=>{assert!(matches!(error.code,MarketErrorCode::MarketBusy|MarketErrorCode::MarketTimeout|MarketErrorCode::TransactionRetryExhausted));println!("PROTOCOL_BOUNDED");}
    }
    tx.rollback().await.unwrap();
}


#[cfg(feature = "contract-tests")]
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn ordered_cancellation_retains_admission_until_completion() {
    let mut f=Fixture::open().await;
    sqlx::query("UPDATE users SET status='APPROVED',authentication_revoked_at=NULL,must_change_password=false WHERE id=$1").bind(id(1)).execute(&mut f.admin).await.unwrap();
    let permit=std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let entered=std::sync::Arc::new(tokio::sync::Notify::new());let release=std::sync::Arc::new(tokio::sync::Notify::new());
    let task=tokio::spawn(transaction::run_paused_market_probe(f.pool.clone(),permit.clone(),f.access(id(1)).await,entered.clone(),release.clone()));
    entered.notified().await;task.abort();
    assert_eq!(permit.available_permits(),0);
    release.notify_one();
    let acquired=tokio::time::timeout(std::time::Duration::from_secs(5),permit.acquire()).await.unwrap().unwrap();drop(acquired);
    let value:i32=sqlx::query_scalar("SELECT value FROM lock_effects WHERE id=1").fetch_one(&mut f.admin).await.unwrap();assert_eq!(value,1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn ordinary_context_requires_current_locked_execution() {
    let mut f=Fixture::open().await;
    sqlx::query("UPDATE users SET organization_id=$1,status='APPROVED',email_verified=true,must_change_password=false,authentication_revoked_at=NULL,kyc_status='APPROVED',kyc_organization_id=$1 WHERE id=$2").bind(id(10)).bind(id(1)).execute(&mut f.admin).await.unwrap();
    let mut conn=f.pool.acquire().await.unwrap();let mut tx=conn.begin().await.unwrap();let mut plan=OperationLockPlan::new(MarketOperation::Create);plan.parties(&preview(id(1))).unwrap();let locked=plan.acquire(&mut tx).await.unwrap();
    let actor=admission::authenticate(&mut tx,&locked,f.access(id(1)).await,AdmissionClass::Execution).await.unwrap();
    let ctx=admission::execution_context(&mut tx,&locked,actor,id(10),None,Utc::now(),true).await.unwrap();assert_eq!(ctx.actor().user_id(),id(1));assert_eq!(ctx.effective_organization_id(),id(10));assert!(ctx.support().is_none());
    tx.rollback().await.unwrap();drop(conn);f.finish().await;
}
