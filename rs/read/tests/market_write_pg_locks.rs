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
