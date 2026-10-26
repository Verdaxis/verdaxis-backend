use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;
use verdaxis_read::market_matching::{MatchBound, OrderRow, OrderStatus, Side, plan_match};
use verdaxis_read::{MarketErrorCode, MarketWriteError};

fn d(raw: &str) -> Decimal {
    Decimal::from_str_exact(raw).unwrap()
}
fn now() -> DateTime<Utc> {
    "2026-10-25T12:00:00Z".parse().unwrap()
}
fn order(id: u128, side: Side, price: &str, quantity: &str, ordinal: i64) -> OrderRow {
    OrderRow {
        id: Uuid::from_u128(id),
        organization_id: Uuid::from_u128(if side == Side::Bid { 1000 } else { 2000 }),
        owner_user_id: Some(Uuid::from_u128(if side == Side::Bid { 1001 } else { 2001 })),
        product_id: Uuid::from_u128(3000),
        delivery_point_id: Some(Uuid::from_u128(4000)),
        availability_window: "SPOT".to_owned(),
        side,
        provenance: "REAL".to_owned(),
        organization_approved: true,
        price_per_mt_usd: d(price),
        quantity_mt: d(quantity),
        remaining_quantity_mt: d(quantity),
        acceptance_ordinal: ordinal,
        version: 1,
        status: OrderStatus::Open,
        expires_at: None,
        // Creation time deliberately opposes acceptance priority.
        created_at: now() - chrono::Duration::seconds(ordinal),
        inventory_item_id: None,
    }
}

#[test]
fn matching_priority_is_price_then_acceptance_then_uuid() {
    let aggressor = order(1, Side::Bid, "120", "7", 99);
    let candidates = vec![
        order(40, Side::Ask, "100", "2", 2),
        order(20, Side::Ask, "100", "2", 1),
        order(10, Side::Ask, "101", "10", 1),
        order(30, Side::Ask, "99", "2", 9),
        order(19, Side::Ask, "100", "2", 1),
    ];
    let plan = plan_match(&candidates, &aggressor, MatchBound::new(now())).unwrap();
    let fills = plan.fills();
    assert_eq!(
        fills
            .iter()
            .map(|f| f.resting_before.id)
            .collect::<Vec<_>>(),
        [30, 19, 20, 40].map(Uuid::from_u128)
    );
    assert_eq!(
        fills.iter().map(|f| f.price_per_mt_usd).collect::<Vec<_>>(),
        vec![d("99"), d("100"), d("100"), d("100")]
    );
    assert_eq!(
        fills
            .iter()
            .map(|f| f.aggressor_after.remaining_quantity_mt)
            .collect::<Vec<_>>(),
        vec![d("5"), d("3"), d("1"), d("0")]
    );
    assert_eq!(fills[3].resting_after.remaining_quantity_mt, d("1"));
    assert_eq!(plan.aggressor_after().version, 5);
}

#[test]
fn resting_price_becomes_the_trade_price() {
    for (side, resting_side, limit, resting_price) in [
        (Side::Bid, Side::Ask, "110", "99.95"),
        (Side::Ask, Side::Bid, "90", "101.05"),
    ] {
        let aggressor = order(1, side, limit, "1.25", 2);
        let mut resting = order(2, resting_side, resting_price, "2", 1);
        resting.organization_id = Uuid::from_u128(9000);
        let plan = plan_match(&[resting], &aggressor, MatchBound::new(now())).unwrap();
        assert_eq!(plan.fills()[0].price_per_mt_usd, d(resting_price));
        assert_eq!(plan.fills()[0].quantity_mt, d("1.25"));
    }
}

#[test]
fn partial_fill_updates_both_remainders_and_versions() {
    let aggressor = order(1, Side::Bid, "110", "3.25", 3);
    let candidates = [
        order(2, Side::Ask, "100", "2", 1),
        order(3, Side::Ask, "101", "4", 2),
    ];
    let plan = plan_match(&candidates, &aggressor, MatchBound::new(now())).unwrap();
    assert_eq!(plan.fills()[0].resting_after.status, OrderStatus::Filled);
    assert_eq!(
        plan.fills()[0].aggressor_after.status,
        OrderStatus::PartiallyFilled
    );
    assert_eq!(
        plan.fills()[1].resting_after.remaining_quantity_mt,
        d("2.75")
    );
    assert_eq!(plan.fills()[1].resting_after.version, 2);
    assert_eq!(plan.aggressor_after().version, 3);
    assert_eq!(plan.aggressor_after().status, OrderStatus::Filled);
    assert_eq!(plan.aggressor_after().remaining_quantity_mt, Decimal::ZERO);
}

#[test]
fn fanout_over_one_hundred_rejects_the_transaction() {
    let candidates: Vec<_> = (2..=102)
        .map(|id| order(id, Side::Ask, "100", "1", id as i64))
        .collect();
    let aggressor = order(1, Side::Bid, "110", "101", 200);
    let error = plan_match(&candidates, &aggressor, MatchBound::new(now())).unwrap_err();
    assert_eq!(error.code, MarketErrorCode::MatchFanoutExceeded);
    assert_eq!(error.status.as_u16(), 409);
    assert_eq!(aggressor.remaining_quantity_mt, d("101"));
    assert!(
        candidates
            .iter()
            .all(|o| o.version == 1 && o.remaining_quantity_mt == d("1"))
    );
}

#[test]
fn exactly_one_hundred_that_finish_the_aggressor_is_allowed() {
    let candidates: Vec<_> = (2..=102)
        .map(|id| order(id, Side::Ask, "100", "1", id as i64))
        .collect();
    let plan = plan_match(
        &candidates,
        &order(1, Side::Bid, "110", "100", 200),
        MatchBound::new(now()),
    )
    .unwrap();
    assert_eq!(plan.fills().len(), 100);
    assert_eq!(plan.aggressor_after().remaining_quantity_mt, Decimal::ZERO);
    assert_eq!(plan.aggressor_after().version, 101);
}

#[test]
fn expired_and_terminal_rows_are_skipped() {
    let aggressor = order(1, Side::Bid, "110", "10", 200);
    let mut expired = order(2, Side::Ask, "90", "1", 1);
    expired.expires_at = Some(now());
    let mut cancelled = order(3, Side::Ask, "91", "1", 2);
    cancelled.status = OrderStatus::Cancelled;
    let mut filled = order(4, Side::Ask, "92", "1", 3);
    filled.status = OrderStatus::Filled;
    filled.remaining_quantity_mt = Decimal::ZERO;
    let live = order(5, Side::Ask, "100", "1", 4);
    let plan = plan_match(
        &[expired, cancelled, filled, live],
        &aggressor,
        MatchBound::new(now()),
    )
    .unwrap();
    assert_eq!(plan.fills().len(), 1);
    assert_eq!(plan.fills()[0].resting_before.id, Uuid::from_u128(5));
    let mut expired_aggressor = aggressor.clone();
    expired_aggressor.expires_at = Some(now());
    assert!(
        plan_match(
            &[order(6, Side::Ask, "100", "1", 5)],
            &expired_aggressor,
            MatchBound::new(now())
        )
        .unwrap()
        .fills()
        .is_empty()
    );
}

#[test]
fn exact_slice_side_crossing_and_organization_filters_apply() {
    let aggressor = order(1, Side::Bid, "110", "10", 200);
    let mut candidates = vec![order(2, Side::Ask, "100", "1", 1); 8];
    for (index, row) in candidates.iter_mut().enumerate() {
        row.id = Uuid::from_u128(index as u128 + 2);
    }
    candidates[0].product_id = Uuid::new_v4();
    candidates[1].delivery_point_id = None;
    candidates[2].availability_window = "2026-11".to_owned();
    candidates[3].side = Side::Bid;
    candidates[4].price_per_mt_usd = d("111");
    candidates[5].organization_id = aggressor.organization_id;
    candidates[6].provenance = "DEMO".to_owned();
    candidates[7].organization_approved = false;
    assert!(
        plan_match(&candidates, &aggressor, MatchBound::new(now()))
            .unwrap()
            .fills()
            .is_empty()
    );
}

#[test]
fn stored_economic_and_version_bounds_are_checked() {
    let aggressor = order(1, Side::Bid, "110", "10", 200);
    for invalid in ["-1", "0.001", "100001"] {
        let mut resting = order(2, Side::Ask, "100", "1", 1);
        resting.remaining_quantity_mt = d(invalid);
        assert_eq!(
            plan_match(&[resting], &aggressor, MatchBound::new(now()))
                .unwrap_err()
                .code,
            MarketErrorCode::InventoryInconsistent
        );
    }
    let mut resting = order(2, Side::Ask, "100", "1", 1);
    resting.version = i32::MAX;
    assert_eq!(
        plan_match(&[resting], &aggressor, MatchBound::new(now()))
            .unwrap_err()
            .code,
        MarketErrorCode::InventoryInconsistent
    );
}

#[test]
fn match_fanout_status_and_code_are_stable() {
    let error = MarketWriteError::new(MarketErrorCode::MatchFanoutExceeded);
    assert_eq!(error.status.as_u16(), 409);
    assert_eq!(error.code.as_str(), "MATCH_FANOUT_EXCEEDED");
}
