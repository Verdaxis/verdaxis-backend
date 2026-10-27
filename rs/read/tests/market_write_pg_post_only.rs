#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;
use verdaxis_read::market_contract::post_only::{SealedPostOnly, run_sealed_post_only};
use verdaxis_read::market_matching::{
    CreationMethod, ExecutionTerms, OrderRow, OrderStatus, OwnerAdmission, PostOnlyAssessment, Side,
};

const CUSTOMER: Uuid = Uuid::from_u128(0x1000);
const ADMIN: Uuid = Uuid::from_u128(0x7000);

fn now() -> DateTime<Utc> {
    "2026-10-25T12:00:00Z".parse().unwrap()
}
fn d(raw: &str) -> Decimal {
    Decimal::from_str_exact(raw).unwrap()
}

struct Fixture {
    admin: PgConnection,
    observer: PgConnection,
    pool: sqlx::PgPool,
    resting: Vec<Uuid>,
    product: Uuid,
    point: Uuid,
}
impl Fixture {
    /// `count` resting ASKs of 1 MT at 100 from another organization, plus the
    /// administrator who enters the assisted order.
    async fn new(count: i32) -> Self {
        let admin_url = std::env::var("RUST_MATCHING_ADMIN_DATABASE_URL").unwrap();
        let runtime_url = std::env::var("RUST_MATCHING_DATABASE_URL").unwrap();
        let admin_target = url::Url::parse(&admin_url).unwrap();
        let runtime_target = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
        assert!(admin_target.path().ends_with("_test"));
        assert_eq!(admin_target.path(), runtime_target.path());
        assert_ne!(admin_target.username(), runtime_target.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/market_write_matching.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT matching_fixture_seed($1,$2,$3)")
            .bind(Uuid::from_u128(1))
            .bind(count)
            .bind(d("1"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT matching_fixture_admin($1)")
            .bind(ADMIN)
            .execute(&mut admin)
            .await
            .unwrap();
        let resting = sqlx::query_scalar(
            "SELECT id FROM orderbook_orders WHERE side='ASK' ORDER BY acceptance_ordinal,id",
        )
        .fetch_all(&mut admin)
        .await
        .unwrap();
        let slice = sqlx::query(
            "SELECT product_id,delivery_point_id FROM orderbook_orders WHERE side='BID'",
        )
        .fetch_one(&mut admin)
        .await
        .unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&runtime_url)
            .await
            .unwrap();
        Self {
            admin,
            observer,
            pool,
            resting,
            product: slice.get("product_id"),
            point: slice.get("delivery_point_id"),
        }
    }
    async fn price(&mut self, order: Uuid, price: &str) {
        sqlx::query("SELECT matching_fixture_price($1,$2)")
            .bind(order)
            .bind(d(price))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn order(&mut self, order: Uuid, change: &str) {
        sqlx::query("SELECT matching_fixture_order($1,$2)")
            .bind(order)
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    /// An unsaved assisted BID at `price` for the customer organization.
    fn candidate(&self, price: &str) -> OrderRow {
        OrderRow {
            id: Uuid::from_u128(0xa1),
            organization_id: CUSTOMER,
            owner_user_id: Some(ADMIN),
            created_by_actor_user_id: Some(ADMIN),
            creation_method: CreationMethod::MarketSupport,
            product_id: self.product,
            delivery_point_id: Some(self.point),
            availability_window: "SPOT".to_owned(),
            side: Side::Bid,
            provenance: "REAL".to_owned(),
            organization_approved: true,
            price_per_mt_usd: d(price),
            quantity_mt: d("1"),
            remaining_quantity_mt: d("1"),
            acceptance_ordinal: 1,
            version: 1,
            status: OrderStatus::Open,
            expires_at: None,
            created_at: now(),
            inventory_item_id: None,
            terms: ExecutionTerms {
                certification_scheme: Some("ISCC EU".to_owned()),
                certifications: vec!["ISCC EU".to_owned()],
                certification_declared: true,
                ..ExecutionTerms::default()
            },
            owner_admission: OwnerAdmission::Unverified,
        }
    }
    async fn assess(&mut self, candidate: OrderRow) -> SealedPostOnly {
        run_sealed_post_only(&self.pool, candidate, CUSTOMER, ADMIN, now(), true)
            .await
            .unwrap()
    }
    /// The assessment writes nothing: no trade, order change, authorization or effect.
    async fn unchanged(&mut self) {
        let writes: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM trades)+(SELECT count(*) FROM orderbook_orders WHERE version<>1)+(SELECT count(*) FROM market_support_authorizations)+(SELECT count(*) FROM audit_logs)+(SELECT count(*) FROM notifications)+(SELECT count(*) FROM market_event_outbox)",
        )
        .fetch_one(&mut self.observer)
        .await
        .unwrap();
        assert_eq!(writes, 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn post_only_refuses_an_eligible_crossing_row() {
    let mut f = Fixture::new(3).await;
    let [first, second, third] = [f.resting[0], f.resting[1], f.resting[2]];
    // Priority order is second (98), third (99), first (100); the best-priced
    // row's certification is not accepted by the assisted requirement.
    f.price(second, "98").await;
    f.price(third, "99").await;
    f.order(second, "scheme_redcert").await;
    let assessed = f.assess(f.candidate("110")).await;
    assert_eq!(assessed.assessment, PostOnlyAssessment::crosses(d("99")));
    assert!(assessed.assessment.would_cross && !assessed.assessment.indeterminate);
    // Candidate rows are locked ascending UUID, not in matching priority.
    let mut ascending = vec![first, second, third];
    ascending.sort();
    assert_eq!(assessed.order_lock_order, ascending);
    f.unchanged().await;
    // A limit below every executable price rests.
    let assessed = f.assess(f.candidate("98.5")).await;
    assert_eq!(assessed.assessment, PostOnlyAssessment::rests());
    f.unchanged().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn post_only_ignores_an_ineligible_crossing_row() {
    let mut f = Fixture::new(4).await;
    let [incompatible, unqualified, ownerless, unadmitted] =
        [f.resting[0], f.resting[1], f.resting[2], f.resting[3]];
    f.order(incompatible, "scheme_redcert").await;
    f.order(unqualified, "off_spec").await;
    f.order(ownerless, "ownerless").await;
    // An assisted row whose administrator is no longer approved.
    let administrator = Uuid::from_u128(0x7001);
    sqlx::query("SELECT matching_fixture_assist($1,$2)")
        .bind(unadmitted)
        .bind(administrator)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT matching_fixture_party($1,'pending')")
        .bind(administrator)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let assessed = f.assess(f.candidate("110")).await;
    assert_eq!(assessed.assessment, PostOnlyAssessment::rests());
    // The incompatible and unqualified rows were assessed under lock; the
    // ownerless and unadmitted rows are outside the crossing set.
    let mut assessed_rows = vec![incompatible, unqualified];
    assessed_rows.sort();
    assert_eq!(assessed.order_lock_order, assessed_rows);
    f.unchanged().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn post_only_indeterminate_over_one_hundred() {
    let mut f = Fixture::new(101).await;
    let assessed = f.assess(f.candidate("110")).await;
    assert_eq!(assessed.assessment, PostOnlyAssessment::indeterminate());
    assert!(
        assessed.order_lock_order.is_empty(),
        "an undecidable set locks no candidate row"
    );
    f.unchanged().await;
    // One hundred crossing rows are decided.
    let mut f = Fixture::new(100).await;
    let assessed = f.assess(f.candidate("110")).await;
    assert_eq!(assessed.assessment, PostOnlyAssessment::crosses(d("100")));
    assert_eq!(assessed.order_lock_order.len(), 100);
    f.unchanged().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn post_only_reads_the_customer_organization_under_its_lock() {
    let mut f = Fixture::new(1).await;
    sqlx::query("UPDATE organizations SET verification_status='PENDING' WHERE id=$1")
        .bind(CUSTOMER)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let assessed = f.assess(f.candidate("110")).await;
    assert_eq!(assessed.assessment, PostOnlyAssessment::indeterminate());
    assert!(assessed.order_lock_order.is_empty());
    f.unchanged().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn sealed_post_only_closes_later_candidate_acquisition() {
    use verdaxis_read::MarketErrorCode;
    use verdaxis_read::market_contract::post_only::run_sealed_post_only_followed_by;

    let mut f = Fixture::new(3).await;
    for match_next in [false, true] {
        let result = run_sealed_post_only_followed_by(
            &f.pool,
            f.candidate("110"),
            CUSTOMER,
            ADMIN,
            now(),
            match_next,
        )
        .await
        .unwrap();
        assert_eq!(result.assessment, PostOnlyAssessment::crosses(d("100")));
        assert_eq!(
            result.follow_up_error,
            Some(MarketErrorCode::MarketSliceChanged)
        );
        let mut expected = f.resting.clone();
        expected.sort();
        assert_eq!(
            result.order_lock_order, expected,
            "a refused follow-up acquires no additional candidate rows"
        );
        f.unchanged().await;
    }
}
