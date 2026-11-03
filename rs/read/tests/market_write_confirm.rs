use serde_json::json;
use uuid::Uuid;
use verdaxis_read::market_lifecycle::{LifecycleAction, lifecycle_hash};

#[test]
fn lifecycle_receipts_bind_the_operation_and_trade() {
    let trade = Uuid::from_u128(1);
    let confirm = lifecycle_hash(LifecycleAction::Confirm, trade).unwrap();
    assert_eq!(confirm.len(), 64);
    assert_eq!(
        confirm,
        lifecycle_hash(LifecycleAction::Confirm, trade).unwrap()
    );
    assert_ne!(
        confirm,
        lifecycle_hash(LifecycleAction::Decline, trade).unwrap()
    );
    assert_ne!(
        confirm,
        lifecycle_hash(LifecycleAction::Confirm, Uuid::from_u128(2)).unwrap()
    );
}

#[test]
fn recorded_viewer_keeps_pending_supplier_details_separate_from_handoff() {
    use verdaxis_read::market_trades::project_trade;
    let buyer = Uuid::from_u128(1);
    let seller = Uuid::from_u128(2);
    let row = json!({"buyer_id":buyer,"seller_id":seller,"status":"DECLINED","is_anonymous":true,"commission_fee_per_mt_usd":"2","commission_plan":"FREE"});
    let projected = project_trade(row.clone(), Some(buyer), false).unwrap();
    assert!(projected["seller_id"].is_null());
    assert!(projected["commission_fee_per_mt_usd"].is_null());
    let projected = project_trade(row, Some(seller), false).unwrap();
    assert_eq!(projected["seller_id"], seller.to_string());
    assert_eq!(projected["commission_fee_per_mt_usd"], "2");
}
