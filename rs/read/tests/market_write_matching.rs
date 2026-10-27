use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::market_matching::{
    CompatibleTerms, CreationMethod, DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID,
    DemoOrderPair, ExecutionTerms, MatchBound, MatchPlan, OrderRow, OrderStatus, OwnerAdmission,
    PostOnlyAssessment, PostOnlyOrganization, Provenance, Side, assess_post_only, compatible_terms,
    fame_terms_compatible, lng_terms_compatible, order_is_execution_qualified,
    orders_execution_compatible, plan_match, provenance_pair_executes,
};
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
        created_by_actor_user_id: Some(Uuid::from_u128(if side == Side::Bid {
            1001
        } else {
            2001
        })),
        creation_method: CreationMethod::SelfService,
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
        terms: ExecutionTerms {
            certification_scheme: Some("ISCC EU".to_owned()),
            certification_declared: true,
            ..ExecutionTerms::default()
        },
        owner_admission: OwnerAdmission::Admitted,
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

// ---------------------------------------------------------------------------
// Execution eligibility: provenance, the exact demo pair, qualification,
// typed compatibility, owner decisions and the bound.
// ---------------------------------------------------------------------------

const B30: Uuid = Uuid::from_u128(0xc4cecebc_3d2d_5840_8021_57a9a11bc673);
const UCOME_B100: Uuid = Uuid::from_u128(0xe561e43f_d9b2_598e_981c_f1d28d515ddc);
const BIO_LNG: Uuid = Uuid::from_u128(0x8f6f34a5_2927_54a6_8564_cca40ae7cb8a);
const ROTTERDAM: Uuid = Uuid::from_u128(0x1379d36c_1ca9_55b7_9c0d_5235a0ba1f36);
const SINGAPORE: Uuid = Uuid::from_u128(0x73835e92_820e_584b_8280_bb61c63aa28e);
const OTHER_DEMO_ORG: Uuid = Uuid::from_u128(0x4da7b285_34ee_5443_9406_f96b4ed1a251);

fn filled(plan: &MatchPlan) -> Vec<Uuid> {
    plan.fills().iter().map(|f| f.resting_before.id).collect()
}
fn fills(candidates: &[OrderRow], aggressor: &OrderRow) -> Vec<Uuid> {
    filled(&plan_match(candidates, aggressor, MatchBound::new(now())).unwrap())
}
fn ids(raw: &[u128]) -> Vec<Uuid> {
    raw.iter().copied().map(Uuid::from_u128).collect()
}
fn demo_order(id: u128, side: Side, organization: Uuid, price: &str) -> OrderRow {
    let mut row = order(id, side, price, "1", id as i64);
    row.organization_id = organization;
    row.provenance = "DEMO".to_owned();
    row.owner_user_id = None;
    row.created_by_actor_user_id = None;
    row.creation_method = CreationMethod::System;
    row.owner_admission = OwnerAdmission::Unverified;
    row.expires_at = Some(now() + chrono::Duration::days(1));
    row
}
/// A complete, qualified B30 supplier listing (fixed-standard product).
fn b30(id: u128, side: Side, price: &str) -> OrderRow {
    let mut row = order(id, side, price, "1", id as i64);
    row.product_id = B30;
    row.delivery_point_id = Some(ROTTERDAM);
    row.terms = ExecutionTerms {
        certification_scheme: Some("ISCC EU".to_owned()),
        certification_declared: true,
        specification_standard: Some("ISO 8217:2024 RF 380".to_owned()),
        msds_available: true,
        carbon_intensity_gco2_mj: Some(d("10")),
        carbon_intensity_method: Some("LIFECYCLE".to_owned()),
        feedstock: Some("residue".to_owned()),
        origin: Some("NL".to_owned()),
        ..ExecutionTerms::default()
    };
    row
}
fn fame_bid() -> serde_json::Value {
    json!({
        "side": "BID", "neat_fame": true, "standard": "EN_14214",
        "standard_edition": "2012+A2:2019", "sustainability_scheme": "ISCC_EU"
    })
}
fn fame_ask() -> serde_json::Value {
    json!({
        "side": "ASK", "neat_fame": true, "uco_mass_pct": 100, "standard": "EN_14214",
        "standard_edition": "2012+A2:2019", "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "certificate", "certificate_holder": "supplier",
        "evidence_status": "PENDING", "document_references": [],
        "certificate_valid_until": "2027-01-01", "evidence_due": "BEFORE_LOADING",
        "cfpp_c": -5
    })
}
fn lng_bid() -> serde_json::Value {
    json!({
        "side": "BID", "accepted_schemes": ["ISCC_EU"],
        "accepted_delivery_methods": ["SHIP_TO_SHIP"]
    })
}
fn lng_ask() -> serde_json::Value {
    json!({
        "side": "ASK", "sustainability_scheme": "ISCC_EU",
        "scheme_certificate_number": "SELLER-123", "chain_of_custody": "MASS_BALANCE",
        "union_database_registered": true, "bio_share_pct": "100", "feedstock": "MANURE",
        "production_country": "NL", "wtt_ghg_gco2e_mj": "-20.25",
        "specification_standard": "ISO 23306:2020", "min_methane_number": "80",
        "delivery_method": "SHIP_TO_SHIP", "seller_is_physical_supplier": false,
        "physical_supplier_name": "Private bunkering partner", "pos_timing": "WITH_DELIVERY"
    })
}
fn with(mut terms: serde_json::Value, key: &str, value: serde_json::Value) -> serde_json::Value {
    terms[key] = value;
    terms
}
fn without(mut terms: serde_json::Value, key: &str) -> serde_json::Value {
    terms.as_object_mut().unwrap().remove(key);
    terms
}
fn typed(id: u128, side: Side, product: Uuid, point: Uuid, terms: serde_json::Value) -> OrderRow {
    let mut row = order(
        id,
        side,
        if side == Side::Bid { "110" } else { "100" },
        "1",
        id as i64,
    );
    row.product_id = product;
    row.delivery_point_id = Some(point);
    row.terms.msds_available = true;
    if product == UCOME_B100 {
        row.terms.fame_terms = Some(terms);
    } else {
        row.terms.lng_terms = Some(terms);
    }
    row
}
fn today() -> NaiveDate {
    MatchBound::new(now()).today()
}

#[test]
fn real_never_crosses_demo_or_unknown() {
    let labels = ["REAL", "DEMO", "UNKNOWN", "TEST", "CANARY", "real", "LIVE"];
    for aggressor_label in labels {
        for resting_label in labels {
            let mut aggressor = order(1, Side::Bid, "110", "1", 9);
            let mut resting = order(2, Side::Ask, "100", "1", 1);
            aggressor.provenance = aggressor_label.to_owned();
            resting.provenance = resting_label.to_owned();
            let expected = aggressor_label == "REAL" && resting_label == "REAL";
            assert_eq!(
                provenance_pair_executes(&aggressor, &resting),
                expected,
                "{aggressor_label}/{resting_label}"
            );
            assert_eq!(
                !fills(&[resting], &aggressor).is_empty(),
                expected,
                "{aggressor_label}/{resting_label}"
            );
        }
    }
    // An unrecognized label is never REAL.
    assert_eq!(Provenance::coerce("REAL"), Provenance::Real);
    assert_eq!(Provenance::coerce("real"), Provenance::Unknown);
    assert_eq!(Provenance::coerce("LIVE"), Provenance::Unknown);
    // REAL never crosses the demo activity pair, even when the pair is supplied.
    let bid = order(1, Side::Bid, "110", "1", 9);
    let paired = demo_order(2, Side::Ask, DEMO_ACTIVITY_SELLER_ORG_ID, "100");
    let pair = DemoOrderPair::new(bid.id, paired.id).unwrap();
    assert!(!provenance_pair_executes(&bid, &paired));
    let bound = MatchBound::new(now()).with_demo_pair(pair);
    assert!(
        plan_match(&[paired], &bid, bound)
            .unwrap()
            .fills()
            .is_empty()
    );
}

#[test]
fn demo_requires_the_exact_two_order_ids() {
    let bid = demo_order(1, Side::Bid, DEMO_ACTIVITY_BUYER_ORG_ID, "110");
    let paired = demo_order(2, Side::Ask, DEMO_ACTIVITY_SELLER_ORG_ID, "100");
    // Better priced orders of the same seller and of another demo organization.
    let unpaired = demo_order(3, Side::Ask, DEMO_ACTIVITY_SELLER_ORG_ID, "99");
    let foreign = demo_order(4, Side::Ask, OTHER_DEMO_ORG, "98");
    let book = [foreign.clone(), unpaired.clone(), paired.clone()];
    let plan_with = |pair: Option<DemoOrderPair>, aggressor: &OrderRow, rows: &[OrderRow]| {
        let bound = match pair {
            Some(pair) => MatchBound::new(now()).with_demo_pair(pair),
            None => MatchBound::new(now()),
        };
        filled(&plan_match(rows, aggressor, bound).unwrap())
    };
    // The allowlisted pair crosses only its paired order.
    let pair = DemoOrderPair::new(bid.id, paired.id).unwrap();
    assert_eq!(plan_with(Some(pair), &bid, &book), vec![paired.id]);
    assert!(provenance_pair_executes(&bid, &paired));
    // No pair, or a pair without the aggressor, executes nothing.
    assert!(plan_with(None, &bid, &book).is_empty());
    let elsewhere = DemoOrderPair::new(paired.id, unpaired.id).unwrap();
    assert!(plan_with(Some(elsewhere), &bid, &book).is_empty());
    // The same demo organizations with another pair never reach the paired order.
    let other = DemoOrderPair::new(bid.id, unpaired.id).unwrap();
    assert!(plan_with(Some(other), &bid, std::slice::from_ref(&paired)).is_empty());
    // One order twice is not a pair.
    assert!(DemoOrderPair::new(bid.id, bid.id).is_none());
    // The paired seller may aggress against its paired buyer.
    let resting_bid = demo_order(5, Side::Bid, DEMO_ACTIVITY_BUYER_ORG_ID, "110");
    let ask = demo_order(6, Side::Ask, DEMO_ACTIVITY_SELLER_ORG_ID, "100");
    let pair = DemoOrderPair::new(resting_bid.id, ask.id).unwrap();
    assert_eq!(
        plan_with(Some(pair), &ask, std::slice::from_ref(&resting_bid)),
        vec![resting_bid.id]
    );
    // Reversed sides: the buyer organization selling, the seller organization buying.
    let reversed_bid = demo_order(7, Side::Bid, DEMO_ACTIVITY_SELLER_ORG_ID, "110");
    let reversed_ask = demo_order(8, Side::Ask, DEMO_ACTIVITY_BUYER_ORG_ID, "100");
    let pair = DemoOrderPair::new(reversed_bid.id, reversed_ask.id).unwrap();
    assert!(
        plan_with(
            Some(pair),
            &reversed_bid,
            std::slice::from_ref(&reversed_ask)
        )
        .is_empty()
    );
    assert!(
        plan_with(
            Some(pair),
            &reversed_ask,
            std::slice::from_ref(&reversed_bid)
        )
        .is_empty()
    );
    // A paired order of the wrong organization never crosses.
    let pair = DemoOrderPair::new(bid.id, foreign.id).unwrap();
    assert!(plan_with(Some(pair), &bid, &book).is_empty());
}

#[test]
fn owner_decisions_gate_each_fill() {
    let aggressor = order(1, Side::Bid, "110", "3", 9);
    let mut refused = order(2, Side::Ask, "97", "1", 1);
    refused.owner_admission = OwnerAdmission::Refused;
    let mut unverified = order(3, Side::Ask, "98", "1", 2);
    unverified.owner_admission = OwnerAdmission::Unverified;
    let mut ownerless = order(4, Side::Ask, "99", "1", 3);
    ownerless.owner_user_id = None;
    let admitted = order(5, Side::Ask, "100", "1", 4);
    let book = [refused, unverified, ownerless, admitted];
    assert_eq!(fills(&book, &aggressor), ids(&[5]));
    for decision in [OwnerAdmission::Refused, OwnerAdmission::Unverified] {
        let mut aggressor = aggressor.clone();
        aggressor.owner_admission = decision;
        assert!(fills(&book, &aggressor).is_empty(), "{decision:?}");
    }
}

#[test]
fn bound_counts_the_previewed_set_before_eligibility() {
    let book = |first: fn(&mut OrderRow)| {
        let mut rows: Vec<_> = (2..=102)
            .map(|id| order(id, Side::Ask, "100", "1", id as i64))
            .collect();
        first(&mut rows[0]);
        rows
    };
    let aggressor = order(1, Side::Bid, "110", "100", 200);
    // A row skipped after the cap still counts: 99 fills leave a remainder.
    for skipped in [
        (|row: &mut OrderRow| row.terms.off_spec = true) as fn(&mut OrderRow),
        |row| row.owner_admission = OwnerAdmission::Refused,
        |row| row.terms.certification_scheme = Some("REDcert-EU".to_owned()),
    ] {
        let error = plan_match(&book(skipped), &aggressor, MatchBound::new(now())).unwrap_err();
        assert_eq!(error.code, MarketErrorCode::MatchFanoutExceeded);
    }
    // An ownerless row is outside the previewed set and does not count.
    let plan = plan_match(
        &book(|row| row.owner_user_id = None),
        &aggressor,
        MatchBound::new(now()),
    )
    .unwrap();
    assert_eq!(plan.fills().len(), 100);
    // An aggressor whose owner is refused fills nothing, and the bound still applies.
    let mut refused = order(1, Side::Bid, "110", "1", 200);
    refused.owner_admission = OwnerAdmission::Refused;
    let error = plan_match(&book(|_| {}), &refused, MatchBound::new(now())).unwrap_err();
    assert_eq!(error.code, MarketErrorCode::MatchFanoutExceeded);
    // An aggressor that cannot execute plans nothing before any bound.
    let mut unqualified = order(1, Side::Bid, "110", "100", 200);
    unqualified.terms.off_spec = true;
    let plan = plan_match(&book(|_| {}), &unqualified, MatchBound::new(now())).unwrap();
    assert!(plan.fills().is_empty());
}

#[test]
fn self_trade_is_refused_for_the_economic_organization() {
    let aggressor = order(1, Side::Bid, "110", "1", 9);
    // Same organization, a different user.
    let mut colleague = order(2, Side::Ask, "100", "1", 1);
    colleague.organization_id = aggressor.organization_id;
    colleague.owner_user_id = Some(Uuid::from_u128(1002));
    // Same organization, the same user.
    let mut own = order(3, Side::Ask, "100", "1", 2);
    own.organization_id = aggressor.organization_id;
    own.owner_user_id = aggressor.owner_user_id;
    assert!(fills(&[colleague, own], &aggressor).is_empty());
    // An assisted order acts for its customer organization, not its administrator.
    let admin = Some(Uuid::from_u128(7000));
    let mut assisted = order(4, Side::Bid, "110", "1", 10);
    assisted.owner_user_id = admin;
    assisted.created_by_actor_user_id = admin;
    assisted.creation_method = CreationMethod::MarketSupport;
    let mut customer_member = order(5, Side::Ask, "100", "1", 3);
    customer_member.organization_id = assisted.organization_id;
    assert!(fills(&[customer_member], &assisted).is_empty());
    let mut other_customer = order(6, Side::Ask, "100", "1", 4);
    other_customer.owner_user_id = admin;
    other_customer.created_by_actor_user_id = admin;
    other_customer.creation_method = CreationMethod::MarketSupport;
    assert_eq!(fills(&[other_customer], &assisted), ids(&[6]));
}

#[test]
fn qualification_follows_the_catalog_profile() {
    let bound = MatchBound::new(now());
    assert!(order_is_execution_qualified(
        &b30(1, Side::Ask, "100"),
        bound
    ));
    // Each fixed-standard supplier scalar is required on the ASK side.
    let missing: [fn(&mut ExecutionTerms); 9] = [
        |t| t.off_spec = true,
        |t| t.certification_scheme = Some("  ".to_owned()),
        |t| t.certification_declared = false,
        |t| t.specification_standard = Some("ISO 8217:2024 DFA".to_owned()),
        |t| t.carbon_intensity_method = Some(" ".to_owned()),
        |t| t.msds_available = false,
        |t| t.carbon_intensity_gco2_mj = None,
        |t| t.feedstock = None,
        |t| t.origin = Some("\t".to_owned()),
    ];
    for (index, change) in missing.iter().enumerate() {
        let mut row = b30(1, Side::Ask, "100");
        change(&mut row.terms);
        assert!(!order_is_execution_qualified(&row, bound), "change {index}");
    }
    // Zero carbon intensity is a declaration, not an absence.
    let mut zero = b30(1, Side::Ask, "100");
    zero.terms.carbon_intensity_gco2_mj = Some(Decimal::ZERO);
    assert!(order_is_execution_qualified(&zero, bound));
    // A requirement needs none of the supplier scalars, but is never off-spec.
    let mut bid = b30(2, Side::Bid, "110");
    bid.terms = ExecutionTerms::default();
    assert!(order_is_execution_qualified(&bid, bound));
    bid.terms.off_spec = true;
    assert!(!order_is_execution_qualified(&bid, bound));
    // The planner skips an unqualified resting row and keeps priority.
    let aggressor = b30(1, Side::Bid, "110");
    let mut unqualified = b30(2, Side::Ask, "99");
    unqualified.terms.carbon_intensity_gco2_mj = None;
    let qualified = b30(3, Side::Ask, "100");
    assert_eq!(fills(&[unqualified, qualified], &aggressor), ids(&[3]));
}

#[test]
fn certificate_currency_uses_the_singapore_date_of_the_command_clock() {
    let ask = |terms: serde_json::Value| {
        let mut row = typed(2, Side::Ask, UCOME_B100, SINGAPORE, terms);
        row.terms.certification_scheme = Some("ISCC_EU".to_owned());
        row
    };
    let current = ask(with(
        fame_ask(),
        "certificate_valid_until",
        json!("2026-10-25"),
    ));
    // 15:59:59Z is still 25 October in Singapore; 16:00Z is already 26 October.
    let before: DateTime<Utc> = "2026-10-25T15:59:59Z".parse().unwrap();
    let after: DateTime<Utc> = "2026-10-25T16:00:00Z".parse().unwrap();
    assert!(order_is_execution_qualified(
        &current,
        MatchBound::new(before)
    ));
    assert!(!order_is_execution_qualified(
        &current,
        MatchBound::new(after)
    ));
    // A quality test dated 26 October is in the future only before Singapore midnight.
    let tested = ask(with(
        fame_ask(),
        "quality_evidence",
        json!({"status": "PENDING", "tested_on": "2026-10-26"}),
    ));
    assert!(!order_is_execution_qualified(
        &tested,
        MatchBound::new(before)
    ));
    assert!(order_is_execution_qualified(
        &tested,
        MatchBound::new(after)
    ));
    // The same boundary decides compatibility.
    let bid = typed(1, Side::Bid, UCOME_B100, SINGAPORE, fame_bid());
    assert!(orders_execution_compatible(
        &bid,
        &current,
        MatchBound::new(before)
    ));
    assert!(!orders_execution_compatible(
        &bid,
        &current,
        MatchBound::new(after)
    ));
}

#[test]
fn fame_requirements_each_hold_independently() {
    let today = today();
    assert!(fame_terms_compatible(&fame_bid(), &fame_ask(), today));
    let incompatible = [
        (
            with(fame_bid(), "standard", json!("ASTM_D6751")),
            fame_ask(),
        ),
        (
            with(fame_bid(), "standard_edition", json!("2019")),
            fame_ask(),
        ),
        (
            with(fame_bid(), "sustainability_scheme", json!("REDCERT_EU")),
            fame_ask(),
        ),
        (
            with(fame_bid(), "evidence_due", json!("BEFORE_DELIVERY")),
            fame_ask(),
        ),
        (
            with(fame_bid(), "en_climate_class", json!("Class C")),
            fame_ask(),
        ),
        (with(fame_bid(), "max_cfpp_c", json!(-10)), fame_ask()),
        (with(fame_bid(), "max_cloud_point_c", json!(0)), fame_ask()),
        (
            with(
                with(
                    with(
                        with(fame_bid(), "max_ci_gco2e_mj", json!(20)),
                        "ci_methodology",
                        json!("RED"),
                    ),
                    "ci_boundary",
                    json!("WTW"),
                ),
                "ci_basis",
                json!("ACTUAL"),
            ),
            fame_ask(),
        ),
        (
            with(fame_bid(), "require_quality_evidence", json!(true)),
            fame_ask(),
        ),
        (
            with(fame_bid(), "require_sustainability_evidence", json!(true)),
            fame_ask(),
        ),
        (
            fame_bid(),
            with(
                with(fame_ask(), "evidence_due", json!("BEFORE_DELIVERY")),
                "sustainability_evidence",
                json!({"status": "PENDING", "document_type": "POS", "due": "BEFORE_DELIVERY"}),
            ),
        ),
        // Invalid terms on either side, or an expired certificate, fail closed.
        (without(fame_bid(), "standard"), fame_ask()),
        (with(fame_bid(), "side", json!("ASK")), fame_ask()),
        (fame_bid(), with(fame_ask(), "uco_mass_pct", json!(90))),
        (
            fame_bid(),
            with(fame_ask(), "certificate_valid_until", json!("2026-10-24")),
        ),
    ];
    for (index, (bid, ask)) in incompatible.iter().enumerate() {
        assert!(!fame_terms_compatible(bid, ask, today), "case {index}");
    }
    // A limit of zero is a limit: an absent declaration fails and zero passes.
    let zero_limit = with(fame_bid(), "max_cfpp_c", json!(0));
    assert!(!fame_terms_compatible(
        &zero_limit,
        &without(fame_ask(), "cfpp_c"),
        today
    ));
    assert!(fame_terms_compatible(
        &zero_limit,
        &with(fame_ask(), "cfpp_c", json!(0)),
        today
    ));
    assert!(!fame_terms_compatible(
        &zero_limit,
        &with(fame_ask(), "cfpp_c", json!("0.01")),
        today
    ));
    // A requested grade must be the declared grade; no request accepts any.
    let astm = |terms: serde_json::Value, grade: &str| {
        with(
            with(terms, "standard", json!("ASTM_D6751")),
            "astm_grade",
            json!(grade),
        )
    };
    assert!(fame_terms_compatible(
        &astm(fame_bid(), "1-B S15"),
        &astm(fame_ask(), "1-B S15"),
        today
    ));
    assert!(!fame_terms_compatible(
        &astm(fame_bid(), "2-B S15"),
        &astm(fame_ask(), "1-B S15"),
        today
    ));
    // Comparable carbon intensity: the same method, boundary and basis, within the limit.
    let ci_bid = with(
        with(
            with(
                with(fame_bid(), "max_ci_gco2e_mj", json!(20)),
                "ci_methodology",
                json!("RED"),
            ),
            "ci_boundary",
            json!("WTW"),
        ),
        "ci_basis",
        json!("ACTUAL"),
    );
    let ci_ask = with(
        with(
            with(
                with(fame_ask(), "ci_gco2e_mj", json!("19.5")),
                "ci_methodology",
                json!("RED"),
            ),
            "ci_boundary",
            json!("WTW"),
        ),
        "ci_basis",
        json!("ACTUAL"),
    );
    assert!(fame_terms_compatible(&ci_bid, &ci_ask, today));
    assert!(!fame_terms_compatible(
        &ci_bid,
        &with(ci_ask, "ci_basis", json!("DEFAULT")),
        today
    ));
    // Available evidence satisfies the evidence requirements.
    let evidence_ask = with(
        with(
            with(fame_ask(), "batch_reference", json!("B-1")),
            "quality_evidence",
            json!({"status": "AVAILABLE", "reference": "Q-1", "batch_reference": "B-1",
                   "laboratory": "Lab", "tested_on": "2026-10-01"}),
        ),
        "sustainability_evidence",
        json!({"status": "AVAILABLE", "document_type": "POS", "reference": "P-1",
               "due": "BEFORE_LOADING"}),
    );
    let evidence_bid = with(
        with(fame_bid(), "require_quality_evidence", json!(true)),
        "require_sustainability_evidence",
        json!(true),
    );
    assert!(fame_terms_compatible(&evidence_bid, &evidence_ask, today));
}

#[test]
fn lng_requirements_each_hold_independently() {
    assert!(lng_terms_compatible(&lng_bid(), &lng_ask()));
    let incompatible = [
        // 1. scheme
        (
            with(lng_bid(), "accepted_schemes", json!(["REDCERT_EU"])),
            lng_ask(),
        ),
        // 2. custody
        (
            with(
                lng_bid(),
                "accepted_custody",
                json!(["PHYSICAL_SEGREGATION"]),
            ),
            lng_ask(),
        ),
        // 3. union database (required by default)
        (
            lng_bid(),
            with(lng_ask(), "union_database_registered", json!(false)),
        ),
        // 4. minimum bio share
        (
            with(lng_bid(), "min_bio_share_pct", json!("100")),
            with(lng_ask(), "bio_share_pct", json!("99.99")),
        ),
        // 5. maximum well-to-tank emissions
        (
            with(lng_bid(), "max_wtt_ghg_gco2e_mj", json!("-21")),
            lng_ask(),
        ),
        // 6. delivery method
        (
            with(
                lng_bid(),
                "accepted_delivery_methods",
                json!(["TRUCK_TO_SHIP"]),
            ),
            lng_ask(),
        ),
        // 7. minimum methane number
        (
            with(lng_bid(), "min_methane_number", json!("81")),
            lng_ask(),
        ),
        // 8. resellers
        (with(lng_bid(), "accept_resellers", json!(false)), lng_ask()),
        // Invalid declarations fail closed.
        (without(lng_bid(), "accepted_schemes"), lng_ask()),
        (
            lng_bid(),
            with(lng_ask(), "production_country", json!("ZZ")),
        ),
        (lng_bid(), with(lng_ask(), "side", json!("BID"))),
    ];
    for (index, (bid, ask)) in incompatible.iter().enumerate() {
        assert!(!lng_terms_compatible(bid, ask), "case {index}");
    }
    // Each rule's passing side.
    assert!(lng_terms_compatible(
        &with(lng_bid(), "require_union_database", json!(false)),
        &with(lng_ask(), "union_database_registered", json!(false))
    ));
    assert!(lng_terms_compatible(
        &with(lng_bid(), "accept_resellers", json!(false)),
        &with(lng_ask(), "seller_is_physical_supplier", json!(true))
    ));
    // Zero is a limit, not an absence.
    assert!(lng_terms_compatible(
        &with(lng_bid(), "max_wtt_ghg_gco2e_mj", json!("0")),
        &lng_ask()
    ));
    assert!(!lng_terms_compatible(
        &with(lng_bid(), "max_wtt_ghg_gco2e_mj", json!("0")),
        &with(lng_ask(), "wtt_ghg_gco2e_mj", json!("0.01"))
    ));
    assert!(lng_terms_compatible(
        &with(lng_bid(), "min_methane_number", json!("0")),
        &lng_ask()
    ));
    assert!(!lng_terms_compatible(
        &with(lng_bid(), "min_methane_number", json!("0")),
        &without(lng_ask(), "min_methane_number")
    ));
}

#[test]
fn typed_compatibility_decides_the_fill_and_returns_the_terms() {
    // B100 at Singapore: the typed requirement decides.
    let bid = typed(1, Side::Bid, UCOME_B100, SINGAPORE, fame_bid());
    let mut ask = typed(2, Side::Ask, UCOME_B100, SINGAPORE, fame_ask());
    ask.terms.certification_scheme = Some("ISCC_EU".to_owned());
    assert_eq!(fills(std::slice::from_ref(&ask), &bid), ids(&[2]));
    match compatible_terms(&bid, &ask, MatchBound::new(now())) {
        Some(CompatibleTerms::Fame { bid, ask }) => {
            assert_eq!(bid.standard_edition, "2012+A2:2019");
            assert_eq!(ask.cfpp_c, Some(d("-5")));
        }
        other => panic!("expected B100 terms, got {other:?}"),
    }
    let strict = typed(
        3,
        Side::Bid,
        UCOME_B100,
        SINGAPORE,
        with(fame_bid(), "max_cfpp_c", json!(-10)),
    );
    assert!(fills(std::slice::from_ref(&ask), &strict).is_empty());
    // Bio-LNG at Rotterdam: the eight rules decide.
    let bid = typed(4, Side::Bid, BIO_LNG, ROTTERDAM, lng_bid());
    let mut ask = typed(5, Side::Ask, BIO_LNG, ROTTERDAM, lng_ask());
    ask.terms.certification_scheme = Some("ISCC EU".to_owned());
    assert_eq!(fills(std::slice::from_ref(&ask), &bid), ids(&[5]));
    let strict = typed(
        6,
        Side::Bid,
        BIO_LNG,
        ROTTERDAM,
        with(lng_bid(), "accept_resellers", json!(false)),
    );
    assert!(fills(std::slice::from_ref(&ask), &strict).is_empty());
    // An invalid stored requirement never executes.
    let broken = typed(7, Side::Bid, BIO_LNG, ROTTERDAM, json!({"side": "BID"}));
    assert!(fills(std::slice::from_ref(&ask), &broken).is_empty());
}

#[test]
fn scalar_certifications_follow_the_accepted_set() {
    let bound = MatchBound::new(now());
    let ask = order(2, Side::Ask, "100", "1", 1);
    let bid = |certifications: &[&str], scheme: Option<&str>| {
        let mut row = order(1, Side::Bid, "110", "1", 9);
        row.terms.certifications = certifications
            .iter()
            .map(|value| value.to_string())
            .collect();
        row.terms.certification_scheme = scheme.map(str::to_owned);
        row
    };
    assert!(orders_execution_compatible(
        &bid(&["iscc eu", " "], None),
        &ask,
        bound
    ));
    assert!(!orders_execution_compatible(
        &bid(&["REDcert-EU"], Some("ISCC EU")),
        &ask,
        bound
    ));
    assert!(orders_execution_compatible(
        &bid(&[], Some(" iscc eu ")),
        &ask,
        bound
    ));
    assert!(!orders_execution_compatible(
        &bid(&[], Some("REDcert-EU")),
        &ask,
        bound
    ));
    assert!(orders_execution_compatible(&bid(&[], None), &ask, bound));
    assert!(orders_execution_compatible(&bid(&[" "], None), &ask, bound));
}

fn customer() -> PostOnlyOrganization {
    PostOnlyOrganization {
        id: Uuid::from_u128(1000),
        approved: true,
    }
}

#[test]
fn post_only_reports_the_first_executable_crossing_price() {
    let bound = MatchBound::new(now());
    let candidate = order(1, Side::Bid, "110", "1", 9);
    // Best price first: an ineligible owner, then an incompatible row, then the first executable.
    let mut ineligible = order(2, Side::Ask, "97", "1", 1);
    ineligible.owner_admission = OwnerAdmission::Refused;
    let mut incompatible = order(3, Side::Ask, "98", "1", 2);
    incompatible.terms.certification_scheme = Some("REDcert-EU".to_owned());
    let mut candidate_with_set = candidate.clone();
    candidate_with_set.terms.certifications = vec!["ISCC EU".to_owned()];
    let executable = order(4, Side::Ask, "99", "1", 3);
    let later = order(5, Side::Ask, "99", "1", 4);
    let book = [later, executable, incompatible, ineligible];
    assert_eq!(
        assess_post_only(&candidate_with_set, customer(), &book, bound),
        PostOnlyAssessment::crosses(d("99"))
    );
    // Rows that do not cross, or that cannot execute, never block.
    let mut demo = order(6, Side::Ask, "99", "1", 5);
    demo.provenance = "DEMO".to_owned();
    let mut ownerless = order(7, Side::Ask, "99", "1", 6);
    ownerless.owner_user_id = None;
    let above = order(8, Side::Ask, "111", "1", 7);
    let mut unqualified = order(9, Side::Ask, "99", "1", 8);
    unqualified.terms.off_spec = true;
    let quiet = [
        book[2].clone(),
        book[3].clone(),
        demo,
        ownerless,
        above,
        unqualified,
    ];
    assert_eq!(
        assess_post_only(&candidate_with_set, customer(), &quiet, bound),
        PostOnlyAssessment::rests()
    );
    // An ASK candidate crosses the highest executable BID.
    let ask = order(10, Side::Ask, "100", "1", 9);
    let low = order(11, Side::Bid, "101", "1", 1);
    let high = order(12, Side::Bid, "103", "1", 2);
    assert_eq!(
        assess_post_only(
            &ask,
            PostOnlyOrganization {
                id: ask.organization_id,
                approved: true
            },
            &[low, high],
            bound
        ),
        PostOnlyAssessment::crosses(d("103"))
    );
}

#[test]
fn post_only_preconditions_are_indeterminate() {
    let bound = MatchBound::new(now());
    let candidate = order(1, Side::Bid, "110", "1", 9);
    let book = [order(2, Side::Ask, "100", "1", 1)];
    let indeterminate = PostOnlyAssessment::indeterminate();
    assert!(indeterminate.would_cross && indeterminate.indeterminate);
    let mut empty = candidate.clone();
    empty.remaining_quantity_mt = Decimal::ZERO;
    empty.status = OrderStatus::Filled;
    let mut unqualified = candidate.clone();
    unqualified.terms.off_spec = true;
    let mut unknown = candidate.clone();
    unknown.provenance = "UNKNOWN".to_owned();
    for row in [empty, unqualified, unknown] {
        assert_eq!(
            assess_post_only(&row, customer(), &book, bound),
            indeterminate
        );
    }
    let other = PostOnlyOrganization {
        id: Uuid::from_u128(9000),
        approved: true,
    };
    assert_eq!(
        assess_post_only(&candidate, other, &book, bound),
        indeterminate
    );
    let pending = PostOnlyOrganization {
        id: Uuid::from_u128(1000),
        approved: false,
    };
    assert_eq!(
        assess_post_only(&candidate, pending, &book, bound),
        indeterminate
    );
}

#[test]
fn post_only_full_set_of_one_hundred_and_one_is_indeterminate() {
    let bound = MatchBound::new(now());
    let candidate = order(1, Side::Bid, "110", "1", 9);
    let mut book: Vec<_> = (2..=102)
        .map(|id| {
            let mut row = order(id, Side::Ask, "100", "1", id as i64);
            row.owner_admission = OwnerAdmission::Refused;
            row
        })
        .collect();
    assert_eq!(
        assess_post_only(&candidate, customer(), &book, bound),
        PostOnlyAssessment::indeterminate()
    );
    book.pop();
    assert_eq!(
        assess_post_only(&candidate, customer(), &book, bound),
        PostOnlyAssessment::rests()
    );
}
