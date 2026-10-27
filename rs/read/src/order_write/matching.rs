//! Bounded match planning and application inside the ordered caller transaction.
//!
//! The planner decides execution eligibility as well as fills: typed provenance
//! with the exact demo pair, execution qualification, typed term compatibility
//! and the owner decisions the port establishes under the held party locks.
//! Trade completion and route admission are supplied by the command owner.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::Map;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::authority::ActorRole;
use super::authority::admission::{PartyRef, require_execution};
use super::locks::{LockClass, Locked, OperationLockPlan, SliceKey};
use super::{MarketErrorCode, MarketWriteError, OrderQuantity, OrderRemainder, Price};
use crate::market::terms::{
    QualificationInput, normalize_certification_scheme, order_is_execution_qualified_at,
    validate_fame_schema, validate_fame_terms, validate_lng_terms,
};
use crate::market::whitespace::is_python_whitespace;
use crate::{Json, MarketPolicy};

pub use super::post_only::{PostOnlyAssessment, PostOnlyOrganization, assess_post_only};

pub const MAX_CROSSING_ORDERS_PER_MATCH: usize = 100;

/// The demo activity organizations. DEMO liquidity executes only between them.
pub const DEMO_ACTIVITY_BUYER_ORG_ID: Uuid =
    Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111);
pub const DEMO_ACTIVITY_SELLER_ORG_ID: Uuid =
    Uuid::from_u128(0xd1e43e55_3fb0_4b5e_9f0b_93aa10050222);

const FAME_PROFILE: &str = "fame_v1";
const LNG_PROFILE: &str = "lng_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}
impl Side {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "BID",
            Self::Ask => "ASK",
        }
    }
    pub(super) fn opposite(self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Expired,
}
impl OrderStatus {
    fn live(self) -> bool {
        matches!(self, Self::Open | Self::PartiallyFilled)
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::PartiallyFilled => "PARTIALLY_FILLED",
            Self::Filled => "FILLED",
            Self::Cancelled => "CANCELLED",
            Self::Expired => "EXPIRED",
        }
    }
}

/// Stored market provenance. Coercion fails closed: an unrecognized label is
/// UNKNOWN, never REAL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    Real,
    Demo,
    Unknown,
    Test,
    Canary,
}
impl Provenance {
    pub fn coerce(label: &str) -> Self {
        match label {
            "REAL" => Self::Real,
            "DEMO" => Self::Demo,
            "TEST" => Self::Test,
            "CANARY" => Self::Canary,
            _ => Self::Unknown,
        }
    }
}

/// How an order was created. Assisted rows (`MarketSupport`) are owned by the
/// administrator who entered them for a customer organization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CreationMethod {
    SelfService,
    MarketSupport,
    System,
    LegacyUnknown,
}
impl CreationMethod {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "SELF_SERVICE" => Some(Self::SelfService),
            "MARKET_SUPPORT" => Some(Self::MarketSupport),
            "SYSTEM" => Some(Self::System),
            "LEGACY_UNKNOWN" => Some(Self::LegacyUnknown),
            _ => None,
        }
    }
}

/// The owner decision the matching port establishes under the held party
/// locks. A decoded row starts `Unverified`; only `Admitted` owners execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerAdmission {
    Unverified,
    Admitted,
    Refused,
}

/// The stored declarations that qualification and compatibility read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutionTerms {
    pub off_spec: bool,
    pub msds_available: bool,
    pub certification_declared: bool,
    pub certification_scheme: Option<String>,
    pub certifications: Vec<String>,
    pub fame_terms: Option<Json>,
    pub lng_terms: Option<Json>,
    pub specification_standard: Option<String>,
    pub carbon_intensity_method: Option<String>,
    pub carbon_intensity_gco2_mj: Option<Decimal>,
    pub feedstock: Option<String>,
    pub origin: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub created_by_actor_user_id: Option<Uuid>,
    pub creation_method: CreationMethod,
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub side: Side,
    pub provenance: String,
    pub organization_approved: bool,
    pub price_per_mt_usd: Decimal,
    pub quantity_mt: Decimal,
    pub remaining_quantity_mt: Decimal,
    pub acceptance_ordinal: i64,
    pub version: i32,
    pub status: OrderStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub inventory_item_id: Option<Uuid>,
    pub terms: ExecutionTerms,
    pub owner_admission: OwnerAdmission,
}
impl OrderRow {
    pub(super) fn slice(&self) -> SliceKey {
        SliceKey {
            product_id: self.product_id,
            delivery_point_id: self.delivery_point_id,
            availability_window: self.availability_window.clone(),
        }
    }
    /// The coerced provenance used by every execution decision.
    pub fn execution_provenance(&self) -> Provenance {
        Provenance::coerce(&self.provenance)
    }
    fn validate(&self) -> Result<(), MarketWriteError> {
        OrderQuantity::checked(self.quantity_mt).map_err(|_| inconsistent())?;
        OrderRemainder::checked(self.remaining_quantity_mt).map_err(|_| inconsistent())?;
        Price::checked(self.price_per_mt_usd).map_err(|_| inconsistent())?;
        if self.remaining_quantity_mt > self.quantity_mt
            || self.version < 1
            || self.acceptance_ordinal < 1
            || super::normalize_slice_window(&self.availability_window).is_err()
        {
            return Err(inconsistent());
        }
        Ok(())
    }
    pub(super) fn live_at(&self, now: DateTime<Utc>) -> bool {
        self.status.live()
            && self.remaining_quantity_mt > Decimal::ZERO
            && self.expires_at.is_none_or(|expiry| expiry > now)
    }
    /// The stored row without the port's owner decision, for drift checks.
    fn stored_state(&self) -> Self {
        Self {
            owner_admission: OwnerAdmission::Unverified,
            ..self.clone()
        }
    }
}

/// The exact two demo orders an internal demo activity caller allows to cross.
/// No request type carries one; order routes never supply a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoOrderPair {
    first: Uuid,
    second: Uuid,
}
impl DemoOrderPair {
    /// Two distinct order identifiers, or none.
    pub fn new(first: Uuid, second: Uuid) -> Option<Self> {
        (first != second).then_some(Self { first, second })
    }
    fn counterparty_of(&self, id: Uuid) -> Option<Uuid> {
        if id == self.first {
            Some(self.second)
        } else if id == self.second {
            Some(self.first)
        } else {
            None
        }
    }
}

/// The committed, attested catalog the serving binary also loads. A missing
/// policy executes nothing.
static COMMITTED_POLICY: LazyLock<Option<MarketPolicy>> =
    LazyLock::new(|| crate::runtime::policy::committed_policy().ok());

/// The fixed transaction cap, the command's single clock sample and the
/// execution inputs the command owner supplies.
#[derive(Clone, Copy)]
pub struct MatchBound<'a> {
    now: DateTime<Utc>,
    policy: Option<&'a MarketPolicy>,
    kyc_gate_enabled: bool,
    demo_pair: Option<DemoOrderPair>,
    #[cfg(feature = "contract-tests")]
    preview_barrier: Option<&'a contract::MatchPreviewBarrier>,
}
impl MatchBound<'static> {
    /// The committed catalog policy, the KYC execution gate on (its fail-safe
    /// default) and no demo pair.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now,
            policy: COMMITTED_POLICY.as_ref(),
            kyc_gate_enabled: true,
            demo_pair: None,
            #[cfg(feature = "contract-tests")]
            preview_barrier: None,
        }
    }
}
impl MatchBound<'_> {
    /// The validated runtime catalog policy.
    pub fn with_policy<'b>(self, policy: &'b MarketPolicy) -> MatchBound<'b>
    where
        Self: 'b,
    {
        MatchBound {
            now: self.now,
            policy: Some(policy),
            kyc_gate_enabled: self.kyc_gate_enabled,
            demo_pair: self.demo_pair,
            #[cfg(feature = "contract-tests")]
            preview_barrier: self.preview_barrier,
        }
    }
    /// The runtime `KYC_EXECUTION_GATE_ENABLED` switch.
    pub fn with_kyc_gate(mut self, enabled: bool) -> Self {
        self.kyc_gate_enabled = enabled;
        self
    }
    /// The demo activity pair; internal callers only.
    pub fn with_demo_pair(mut self, pair: DemoOrderPair) -> Self {
        self.demo_pair = Some(pair);
        self
    }
    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }
    /// The Singapore calendar date of the command clock, used for
    /// certificate currency.
    pub fn today(&self) -> NaiveDate {
        self.now
            .with_timezone(&chrono_tz::Asia::Singapore)
            .date_naive()
    }
    fn kyc_gate_enabled(&self) -> bool {
        self.kyc_gate_enabled
    }
}
impl std::fmt::Debug for MatchBound<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("MatchBound")
            .field("now", &self.now)
            .field("policy", &self.policy.map(|policy| &policy.source_sha))
            .field("kyc_gate_enabled", &self.kyc_gate_enabled)
            .field("demo_pair", &self.demo_pair)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Fill {
    pub terms: CompatibleTerms,
    pub resting_before: OrderRow,
    pub resting_after: OrderRow,
    pub aggressor_after: OrderRow,
    pub quantity_mt: Decimal,
    pub price_per_mt_usd: Decimal,
}
#[derive(Clone, Debug)]
pub struct MatchPlan {
    aggressor_before: OrderRow,
    aggressor_after: OrderRow,
    fills: Vec<Fill>,
    now: DateTime<Utc>,
    acquired_orders: Vec<Uuid>,
    acquired_inventory: Vec<Uuid>,
}
impl MatchPlan {
    fn empty(aggressor: &OrderRow, now: DateTime<Utc>) -> Self {
        Self {
            aggressor_before: aggressor.clone(),
            aggressor_after: aggressor.clone(),
            fills: Vec::new(),
            now,
            acquired_orders: Vec::new(),
            acquired_inventory: Vec::new(),
        }
    }
    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }
    pub(super) fn aggressor_before(&self) -> &OrderRow { &self.aggressor_before }
    pub(super) fn clock(&self) -> DateTime<Utc> { self.now }
    pub(super) fn reserved_consumption(&self) -> Result<BTreeMap<Uuid,Decimal>,MarketWriteError> {
        let mut consumption=BTreeMap::<Uuid,Decimal>::new();
        for fill in &self.fills {
            for item in [self.aggressor_before.inventory_item_id,fill.resting_before.inventory_item_id].into_iter().flatten() {
                let amount=consumption.entry(item).or_default();
                *amount=amount.checked_add(fill.quantity_mt).ok_or_else(inconsistent)?;
            }
        }
        Ok(consumption)
    }
    pub(super) fn locked_order_ids(&self) -> Vec<Uuid> {
        self.acquired_orders.clone()
    }
    pub(super) fn inventory_ids(&self) -> Vec<Uuid> {
        self.acquired_inventory.clone()
    }
    pub fn aggressor_after(&self) -> &OrderRow {
        &self.aggressor_after
    }
}
fn inconsistent() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::InventoryInconsistent)
}
pub(super) fn changed() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::MarketSliceChanged)
}

// ---------------------------------------------------------------------------
// Provenance and the exact demo pair.
// ---------------------------------------------------------------------------

/// REAL crosses REAL. DEMO crosses DEMO only between the two demo activity
/// organizations. UNKNOWN, TEST and CANARY never execute, and no mixed pair
/// does.
pub fn provenance_pair_executes(left: &OrderRow, right: &OrderRow) -> bool {
    match (left.execution_provenance(), right.execution_provenance()) {
        (Provenance::Real, Provenance::Real) => true,
        (Provenance::Demo, Provenance::Demo) => {
            BTreeSet::from([left.organization_id, right.organization_id])
                == BTreeSet::from([DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID])
        }
        _ => false,
    }
}

/// How an admitted aggressor may execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AggressorAdmission {
    /// REAL liquidity: both owners are revalidated under the held locks.
    Real,
    /// The exact demo pair: synthetic orders have no owner to revalidate.
    Demo {
        counterparty: Uuid,
        counterparty_organization: Uuid,
    },
}

/// The exact order-pair rule: a DEMO aggressor crosses only its paired order,
/// held by the paired organization.
fn demo_counterparty_allows(
    counterparty: Uuid,
    counterparty_organization: Uuid,
    row: &OrderRow,
) -> bool {
    row.id == counterparty && row.organization_id == counterparty_organization
}

fn demo_admission(aggressor: &OrderRow, pair: DemoOrderPair) -> Option<AggressorAdmission> {
    let (expected, counterparty_organization) = match aggressor.side {
        Side::Bid => (DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID),
        Side::Ask => (DEMO_ACTIVITY_SELLER_ORG_ID, DEMO_ACTIVITY_BUYER_ORG_ID),
    };
    if aggressor.organization_id != expected {
        return None;
    }
    Some(AggressorAdmission::Demo {
        counterparty: pair.counterparty_of(aggressor.id)?,
        counterparty_organization,
    })
}

/// An aggressor that cannot execute plans nothing and acquires no candidate.
fn aggressor_admission(aggressor: &OrderRow, bound: &MatchBound<'_>) -> Option<AggressorAdmission> {
    if !aggressor.live_at(bound.now) || !aggressor.organization_approved {
        return None;
    }
    let admission = match aggressor.execution_provenance() {
        Provenance::Real => AggressorAdmission::Real,
        Provenance::Demo => demo_admission(aggressor, bound.demo_pair?)?,
        Provenance::Unknown | Provenance::Test | Provenance::Canary => return None,
    };
    order_is_execution_qualified(aggressor, *bound).then_some(admission)
}

// ---------------------------------------------------------------------------
// Execution qualification and typed compatibility.
// ---------------------------------------------------------------------------

fn qualified(order: &OrderRow, policy: &MarketPolicy, today: NaiveDate) -> bool {
    let delivery_point = order.delivery_point_id.map(|id| id.to_string());
    let carbon_intensity = order
        .terms
        .carbon_intensity_gco2_mj
        .map(|value| value.to_string());
    let input = QualificationInput {
        product_id: Some(order.product_id),
        market_product: None,
        side: order.side.as_str(),
        delivery_point_id: delivery_point.as_deref(),
        availability_window: &order.availability_window,
        off_spec: order.terms.off_spec,
        msds_available: order.terms.msds_available,
        certification_scheme: order.terms.certification_scheme.as_deref(),
        certification_declared: order.terms.certification_declared,
        fame_terms: order.terms.fame_terms.as_ref(),
        lng_terms: order.terms.lng_terms.as_ref(),
        specification_standard: order.terms.specification_standard.as_deref(),
        carbon_intensity_method: order.terms.carbon_intensity_method.as_deref(),
        carbon_intensity_gco2_mj: carbon_intensity.as_deref(),
        feedstock: order.terms.feedstock.as_deref(),
        origin: order.terms.origin.as_deref(),
    };
    order_is_execution_qualified_at(policy, &input, today)
}

/// Whether an order satisfies its catalog profile's execution qualification on
/// the bound's Singapore date. Without a catalog policy nothing qualifies.
pub fn order_is_execution_qualified(order: &OrderRow, bound: MatchBound<'_>) -> bool {
    bound
        .policy
        .is_some_and(|policy| qualified(order, policy, bound.today()))
}

/// The validated typed terms a compatible pair carries. Trade snapshots reuse
/// these values instead of parsing the declarations again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompatibleTerms {
    Scalar,
    Fame {
        bid: Box<FameBidTerms>,
        ask: Box<FameAskTerms>,
    },
    Lng {
        bid: Box<LngBidTerms>,
        ask: Box<LngAskTerms>,
    },
}

/// Both orders qualified and their terms compatible, failing closed on invalid
/// declarations.
pub fn compatible_terms(
    left: &OrderRow,
    right: &OrderRow,
    bound: MatchBound<'_>,
) -> Option<CompatibleTerms> {
    let policy = bound.policy?;
    let today = bound.today();
    if !qualified(left, policy, today) || !qualified(right, policy, today) {
        return None;
    }
    let (bid, ask) = if left.side == Side::Bid {
        (left, right)
    } else {
        (right, left)
    };
    let profile = |order: &OrderRow| {
        policy
            .products
            .iter()
            .find(|product| product.id == order.product_id)
            .map(|product| product.qualification_profile.as_str())
    };
    let has_profile = |name: &str| profile(bid) == Some(name) || profile(ask) == Some(name);
    if has_profile(FAME_PROFILE) {
        if bid.product_id != ask.product_id {
            return None;
        }
        let bid_terms = FameBidTerms::parse(bid.terms.fame_terms.as_ref()?)?;
        let ask_terms = FameAskTerms::parse(ask.terms.fame_terms.as_ref()?, today)?;
        return fame_pair_compatible(&bid_terms, &ask_terms).then_some(CompatibleTerms::Fame {
            bid: Box::new(bid_terms),
            ask: Box::new(ask_terms),
        });
    }
    if has_profile(LNG_PROFILE) {
        if bid.product_id != ask.product_id {
            return None;
        }
        let bid_terms = LngBidTerms::parse(bid.terms.lng_terms.as_ref()?)?;
        let ask_terms = LngAskTerms::parse(ask.terms.lng_terms.as_ref()?)?;
        return lng_pair_compatible(&bid_terms, &ask_terms).then_some(CompatibleTerms::Lng {
            bid: Box::new(bid_terms),
            ask: Box::new(ask_terms),
        });
    }
    scalar_compatible(bid, ask).then_some(CompatibleTerms::Scalar)
}

/// Both orders qualified and compatible.
pub fn orders_execution_compatible(
    left: &OrderRow,
    right: &OrderRow,
    bound: MatchBound<'_>,
) -> bool {
    compatible_terms(left, right, bound).is_some()
}

/// The certification rule for products without a typed profile.
fn scalar_compatible(bid: &OrderRow, ask: &OrderRow) -> bool {
    let Some(ask_scheme) =
        normalize_certification_scheme(ask.terms.certification_scheme.as_deref())
    else {
        return true;
    };
    let accepted: Vec<String> = bid
        .terms
        .certifications
        .iter()
        .filter_map(|scheme| normalize_certification_scheme(Some(scheme)))
        .collect();
    if !accepted.is_empty() {
        return accepted.contains(&ask_scheme);
    }
    normalize_certification_scheme(bid.terms.certification_scheme.as_deref())
        .is_none_or(|scheme| scheme == ask_scheme)
}

// Declarations are read only after the shared schema rule in `market::terms`
// accepted them. Strings are trimmed as the schema's `str_strip_whitespace`
// does; numbers are exact decimals. Anything unreadable fails closed.

fn declared_text(object: &Map<String, Json>, key: &str) -> Result<Option<String>, ()> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => Ok(Some(value.trim_matches(is_python_whitespace).to_owned())),
        Some(_) => Err(()),
    }
}
fn required_text(object: &Map<String, Json>, key: &str) -> Result<String, ()> {
    declared_text(object, key)?.ok_or(())
}
fn declared_texts(object: &Map<String, Json>, key: &str) -> Result<Option<Vec<String>>, ()> {
    match object.get(key) {
        None => Ok(None),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|value| value.trim_matches(is_python_whitespace).to_owned())
                    .ok_or(())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(()),
    }
}
fn exact_decimal(text: &str) -> Result<Decimal, ()> {
    let text = text.trim();
    Decimal::from_str_exact(text)
        .or_else(|_| Decimal::from_scientific(text))
        .map_err(|_| ())
}
fn declared_number(object: &Map<String, Json>, key: &str) -> Result<Option<Decimal>, ()> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => exact_decimal(value).map(Some),
        Some(Json::Number(value)) => exact_decimal(&value.to_string()).map(Some),
        Some(_) => Err(()),
    }
}
fn required_number(object: &Map<String, Json>, key: &str) -> Result<Decimal, ()> {
    declared_number(object, key)?.ok_or(())
}
fn declared_flag(object: &Map<String, Json>, key: &str, default: bool) -> Result<bool, ()> {
    match object.get(key) {
        None => Ok(default),
        Some(Json::Bool(value)) => Ok(*value),
        Some(_) => Err(()),
    }
}
fn required_flag(object: &Map<String, Json>, key: &str) -> Result<bool, ()> {
    match object.get(key) {
        Some(Json::Bool(value)) => Ok(*value),
        _ => Err(()),
    }
}
/// A declared upper limit holds only when the declared value is present and
/// does not exceed it. Zero is a limit, not an absence.
fn within_limit(limit: Option<Decimal>, actual: Option<Decimal>) -> bool {
    limit.is_none_or(|limit| actual.is_some_and(|actual| actual <= limit))
}

/// A validated B100 requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FameBidTerms {
    pub standard: String,
    pub standard_edition: String,
    pub astm_grade: Option<String>,
    pub en_climate_class: Option<String>,
    pub max_cfpp_c: Option<Decimal>,
    pub max_cloud_point_c: Option<Decimal>,
    pub max_ci_gco2e_mj: Option<Decimal>,
    pub ci_methodology: Option<String>,
    pub ci_boundary: Option<String>,
    pub ci_basis: Option<String>,
    pub sustainability_scheme: String,
    pub require_quality_evidence: bool,
    pub require_sustainability_evidence: bool,
    pub evidence_due: String,
    /// The validated stored declaration.
    pub declaration: Json,
}
impl FameBidTerms {
    pub fn parse(terms: &Json) -> Option<Self> {
        validate_fame_schema("BID", Some(terms)).ok()?;
        Self::read(terms.as_object()?, terms).ok()
    }
    fn read(object: &Map<String, Json>, terms: &Json) -> Result<Self, ()> {
        Ok(Self {
            standard: required_text(object, "standard")?,
            standard_edition: required_text(object, "standard_edition")?,
            astm_grade: declared_text(object, "astm_grade")?,
            en_climate_class: declared_text(object, "en_climate_class")?,
            max_cfpp_c: declared_number(object, "max_cfpp_c")?,
            max_cloud_point_c: declared_number(object, "max_cloud_point_c")?,
            max_ci_gco2e_mj: declared_number(object, "max_ci_gco2e_mj")?,
            ci_methodology: declared_text(object, "ci_methodology")?,
            ci_boundary: declared_text(object, "ci_boundary")?,
            ci_basis: declared_text(object, "ci_basis")?,
            sustainability_scheme: required_text(object, "sustainability_scheme")?,
            require_quality_evidence: declared_flag(object, "require_quality_evidence", false)?,
            require_sustainability_evidence: declared_flag(
                object,
                "require_sustainability_evidence",
                false,
            )?,
            evidence_due: declared_text(object, "evidence_due")?
                .unwrap_or_else(|| "BEFORE_LOADING".to_owned()),
            declaration: terms.clone(),
        })
    }
}

/// The milestone and status of declared B100 sustainability evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FameSustainabilityEvidence {
    pub status: String,
    pub due: String,
}

/// A validated B100 declaration with a current operator certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FameAskTerms {
    pub standard: String,
    pub standard_edition: String,
    pub astm_grade: Option<String>,
    pub en_climate_class: Option<String>,
    pub cfpp_c: Option<Decimal>,
    pub cloud_point_c: Option<Decimal>,
    pub ci_gco2e_mj: Option<Decimal>,
    pub ci_methodology: Option<String>,
    pub ci_boundary: Option<String>,
    pub ci_basis: Option<String>,
    pub sustainability_scheme: String,
    pub evidence_due: String,
    pub certificate_valid_until: NaiveDate,
    pub quality_evidence_status: Option<String>,
    pub sustainability_evidence: Option<FameSustainabilityEvidence>,
    /// The validated stored declaration.
    pub declaration: Json,
}
impl FameAskTerms {
    /// `today` is the Singapore date of the command clock.
    pub fn parse(terms: &Json, today: NaiveDate) -> Option<Self> {
        validate_fame_terms("ASK", Some(terms), today).ok()?;
        Self::read(terms.as_object()?, terms).ok()
    }
    fn read(object: &Map<String, Json>, terms: &Json) -> Result<Self, ()> {
        let nested = |key: &str| match object.get(key) {
            None | Some(Json::Null) => Ok(None),
            Some(Json::Object(value)) => Ok(Some(value)),
            Some(_) => Err(()),
        };
        let sustainability = nested("sustainability_evidence")?
            .map(|evidence| {
                Ok::<_, ()>(FameSustainabilityEvidence {
                    status: required_text(evidence, "status")?,
                    due: required_text(evidence, "due")?,
                })
            })
            .transpose()?;
        let quality_status = nested("quality_evidence")?
            .map(|evidence| required_text(evidence, "status"))
            .transpose()?;
        let certificate = required_text(object, "certificate_valid_until")?;
        Ok(Self {
            standard: required_text(object, "standard")?,
            standard_edition: required_text(object, "standard_edition")?,
            astm_grade: declared_text(object, "astm_grade")?,
            en_climate_class: declared_text(object, "en_climate_class")?,
            cfpp_c: declared_number(object, "cfpp_c")?,
            cloud_point_c: declared_number(object, "cloud_point_c")?,
            ci_gco2e_mj: declared_number(object, "ci_gco2e_mj")?,
            ci_methodology: declared_text(object, "ci_methodology")?,
            ci_boundary: declared_text(object, "ci_boundary")?,
            ci_basis: declared_text(object, "ci_basis")?,
            sustainability_scheme: required_text(object, "sustainability_scheme")?,
            evidence_due: required_text(object, "evidence_due")?,
            certificate_valid_until: NaiveDate::parse_from_str(&certificate, "%Y-%m-%d")
                .map_err(|_| ())?,
            quality_evidence_status: quality_status,
            sustainability_evidence: sustainability,
            declaration: terms.clone(),
        })
    }
}

fn fame_pair_compatible(bid: &FameBidTerms, ask: &FameAskTerms) -> bool {
    let requested = |requested: &Option<String>, declared: &Option<String>| {
        requested
            .as_ref()
            .is_none_or(|requested| declared.as_ref() == Some(requested))
    };
    (
        &bid.standard,
        &bid.standard_edition,
        &bid.sustainability_scheme,
    ) == (
        &ask.standard,
        &ask.standard_edition,
        &ask.sustainability_scheme,
    ) && bid.evidence_due == ask.evidence_due
        && requested(&bid.astm_grade, &ask.astm_grade)
        && requested(&bid.en_climate_class, &ask.en_climate_class)
        && within_limit(bid.max_cfpp_c, ask.cfpp_c)
        && within_limit(bid.max_cloud_point_c, ask.cloud_point_c)
        && within_limit(bid.max_ci_gco2e_mj, ask.ci_gco2e_mj)
        && (bid.max_ci_gco2e_mj.is_none()
            || (&bid.ci_methodology, &bid.ci_boundary, &bid.ci_basis)
                == (&ask.ci_methodology, &ask.ci_boundary, &ask.ci_basis))
        && (!bid.require_quality_evidence
            || ask.quality_evidence_status.as_deref() == Some("AVAILABLE"))
        && (!bid.require_sustainability_evidence
            || ask
                .sustainability_evidence
                .as_ref()
                .is_some_and(|evidence| evidence.status == "AVAILABLE"))
        && ask
            .sustainability_evidence
            .as_ref()
            .is_none_or(|evidence| evidence.due == bid.evidence_due)
}

/// Each typed B100 requirement against a current declaration. Invalid terms
/// on either side are incompatible.
pub fn fame_terms_compatible(bid: &Json, ask: &Json, today: NaiveDate) -> bool {
    match (FameBidTerms::parse(bid), FameAskTerms::parse(ask, today)) {
        (Some(bid), Some(ask)) => fame_pair_compatible(&bid, &ask),
        _ => false,
    }
}

/// A validated `lng_v1` requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LngBidTerms {
    pub accepted_schemes: Vec<String>,
    pub accepted_custody: Vec<String>,
    pub require_union_database: bool,
    pub min_bio_share_pct: Option<Decimal>,
    pub max_wtt_ghg_gco2e_mj: Option<Decimal>,
    pub accepted_delivery_methods: Vec<String>,
    pub min_methane_number: Option<Decimal>,
    pub accept_resellers: bool,
    /// The validated stored declaration.
    pub declaration: Json,
}
impl LngBidTerms {
    pub fn parse(terms: &Json) -> Option<Self> {
        validate_lng_terms("BID", Some(terms)).ok()?;
        Self::read(terms.as_object()?, terms).ok()
    }
    fn read(object: &Map<String, Json>, terms: &Json) -> Result<Self, ()> {
        Ok(Self {
            accepted_schemes: declared_texts(object, "accepted_schemes")?.ok_or(())?,
            accepted_custody: declared_texts(object, "accepted_custody")?.unwrap_or_else(|| {
                vec!["MASS_BALANCE".to_owned(), "PHYSICAL_SEGREGATION".to_owned()]
            }),
            require_union_database: declared_flag(object, "require_union_database", true)?,
            min_bio_share_pct: declared_number(object, "min_bio_share_pct")?,
            max_wtt_ghg_gco2e_mj: declared_number(object, "max_wtt_ghg_gco2e_mj")?,
            accepted_delivery_methods: declared_texts(object, "accepted_delivery_methods")?
                .ok_or(())?,
            min_methane_number: declared_number(object, "min_methane_number")?,
            accept_resellers: declared_flag(object, "accept_resellers", true)?,
            declaration: terms.clone(),
        })
    }
}

/// A validated `lng_v1` declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LngAskTerms {
    pub sustainability_scheme: String,
    pub chain_of_custody: String,
    pub union_database_registered: bool,
    pub bio_share_pct: Decimal,
    pub wtt_ghg_gco2e_mj: Decimal,
    pub min_methane_number: Option<Decimal>,
    pub delivery_method: String,
    pub seller_is_physical_supplier: bool,
    /// The validated stored declaration.
    pub declaration: Json,
}
impl LngAskTerms {
    pub fn parse(terms: &Json) -> Option<Self> {
        validate_lng_terms("ASK", Some(terms)).ok()?;
        Self::read(terms.as_object()?, terms).ok()
    }
    fn read(object: &Map<String, Json>, terms: &Json) -> Result<Self, ()> {
        Ok(Self {
            sustainability_scheme: required_text(object, "sustainability_scheme")?,
            chain_of_custody: required_text(object, "chain_of_custody")?,
            union_database_registered: required_flag(object, "union_database_registered")?,
            bio_share_pct: required_number(object, "bio_share_pct")?,
            wtt_ghg_gco2e_mj: required_number(object, "wtt_ghg_gco2e_mj")?,
            min_methane_number: declared_number(object, "min_methane_number")?,
            delivery_method: required_text(object, "delivery_method")?,
            seller_is_physical_supplier: required_flag(object, "seller_is_physical_supplier")?,
            declaration: terms.clone(),
        })
    }
}

/// The eight `lng_v1` compatibility rules.
fn lng_pair_compatible(bid: &LngBidTerms, ask: &LngAskTerms) -> bool {
    bid.accepted_schemes.contains(&ask.sustainability_scheme)
        && bid.accepted_custody.contains(&ask.chain_of_custody)
        && (ask.union_database_registered || !bid.require_union_database)
        && bid
            .min_bio_share_pct
            .is_none_or(|minimum| ask.bio_share_pct >= minimum)
        && bid
            .max_wtt_ghg_gco2e_mj
            .is_none_or(|maximum| ask.wtt_ghg_gco2e_mj <= maximum)
        && bid.accepted_delivery_methods.contains(&ask.delivery_method)
        && bid.min_methane_number.is_none_or(|minimum| {
            ask.min_methane_number
                .is_some_and(|declared| declared >= minimum)
        })
        && (bid.accept_resellers || ask.seller_is_physical_supplier)
}

/// All eight `lng_v1` rules. Invalid terms on either side are incompatible.
pub fn lng_terms_compatible(bid: &Json, ask: &Json) -> bool {
    match (LngBidTerms::parse(bid), LngAskTerms::parse(ask)) {
        (Some(bid), Some(ask)) => lng_pair_compatible(&bid, &ask),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The pure planner.
// ---------------------------------------------------------------------------

/// The core crossing rule: exact slice, opposite side, other organization,
/// an executable provenance pair, an approved resting organization, a live
/// remainder and a crossing price.
fn crosses(row: &OrderRow, aggressor: &OrderRow, now: DateTime<Utc>) -> bool {
    row.id != aggressor.id
        && row.side == aggressor.side.opposite()
        && row.product_id == aggressor.product_id
        && row.delivery_point_id == aggressor.delivery_point_id
        && row.availability_window == aggressor.availability_window
        && row.organization_id != aggressor.organization_id
        && provenance_pair_executes(aggressor, row)
        && row.organization_approved
        && row.live_at(now)
        && match aggressor.side {
            Side::Bid => row.price_per_mt_usd <= aggressor.price_per_mt_usd,
            Side::Ask => row.price_per_mt_usd >= aggressor.price_per_mt_usd,
        }
}

/// The set the bound counts, as the preview selects it: crossing rows with an
/// owner (REAL), or the exact paired order (DEMO).
fn counts_toward_bound(
    row: &OrderRow,
    aggressor: &OrderRow,
    admission: &AggressorAdmission,
    now: DateTime<Utc>,
) -> bool {
    crosses(row, aggressor, now)
        && match *admission {
            AggressorAdmission::Real => row.owner_user_id.is_some(),
            AggressorAdmission::Demo {
                counterparty,
                counterparty_organization,
            } => demo_counterparty_allows(counterparty, counterparty_organization, row),
        }
}

/// Typed compatibility and owner revalidation apply after the cap; a row that
/// fails them is skipped, never an error.
fn executable(
    row: &OrderRow,
    aggressor: &OrderRow,
    admission: &AggressorAdmission,
    bound: MatchBound<'_>,
) -> bool {
    orders_execution_compatible(aggressor, row, bound)
        && match admission {
            AggressorAdmission::Real => {
                aggressor.owner_admission == OwnerAdmission::Admitted
                    && row.owner_admission == OwnerAdmission::Admitted
            }
            AggressorAdmission::Demo { .. } => true,
        }
}

pub(super) fn priority(left: &OrderRow, right: &OrderRow, side: Side) -> std::cmp::Ordering {
    let price = match side {
        Side::Bid => left.price_per_mt_usd.cmp(&right.price_per_mt_usd),
        Side::Ask => right.price_per_mt_usd.cmp(&left.price_per_mt_usd),
    };
    price
        .then(left.acceptance_ordinal.cmp(&right.acceptance_ordinal))
        .then(left.id.cmp(&right.id))
}
fn decrement(row: &mut OrderRow, quantity: Decimal) -> Result<(), MarketWriteError> {
    row.remaining_quantity_mt = row
        .remaining_quantity_mt
        .checked_sub(quantity)
        .ok_or_else(inconsistent)?;
    OrderRemainder::checked(row.remaining_quantity_mt).map_err(|_| inconsistent())?;
    row.version = row.version.checked_add(1).ok_or_else(inconsistent)?;
    row.status = if row.remaining_quantity_mt == Decimal::ZERO {
        OrderStatus::Filled
    } else {
        OrderStatus::PartiallyFilled
    };
    Ok(())
}

/// The planner never mutates input and never returns a partially accepted prefix.
///
/// The bound counts the previewed set; execution eligibility skips rows only
/// within the first hundred, and a remaining aggressor beyond them refuses the
/// whole plan.
pub fn plan_match(
    candidates: &[OrderRow],
    aggressor: &OrderRow,
    bound: MatchBound<'_>,
) -> Result<MatchPlan, MarketWriteError> {
    plan_match_with_overflow(candidates, aggressor, bound, false)
}

fn plan_match_with_overflow(
    candidates: &[OrderRow],
    aggressor: &OrderRow,
    bound: MatchBound<'_>,
    preview_exceeded: bool,
) -> Result<MatchPlan, MarketWriteError> {
    aggressor.validate()?;
    let mut plan = MatchPlan::empty(aggressor, bound.now);
    let Some(admission) = aggressor_admission(aggressor, &bound) else {
        return Ok(plan);
    };
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for row in candidates {
        row.validate()?;
        if !seen.insert(row.id) {
            return Err(inconsistent());
        }
        if counts_toward_bound(row, aggressor, &admission, bound.now) {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| priority(a, b, aggressor.side));
    let exceeded = preview_exceeded || rows.len() > MAX_CROSSING_ORDERS_PER_MATCH;
    for row in rows.into_iter().take(MAX_CROSSING_ORDERS_PER_MATCH) {
        if plan.aggressor_after.remaining_quantity_mt == Decimal::ZERO {
            break;
        }
        if !executable(row, aggressor, &admission, bound) {
            continue;
        }
        let quantity = row
            .remaining_quantity_mt
            .min(plan.aggressor_after.remaining_quantity_mt);
        let mut after = row.clone();
        decrement(&mut after, quantity)?;
        decrement(&mut plan.aggressor_after, quantity)?;
        let terms = compatible_terms(aggressor,row,bound).ok_or_else(inconsistent)?;
        plan.fills.push(Fill {
            terms,
            resting_before: row.clone(),
            resting_after: after,
            aggressor_after: plan.aggressor_after.clone(),
            quantity_mt: quantity,
            price_per_mt_usd: row.price_per_mt_usd,
        });
    }
    if exceeded && plan.aggressor_after.remaining_quantity_mt > Decimal::ZERO {
        return Err(MarketWriteError::new(MarketErrorCode::MatchFanoutExceeded));
    }
    Ok(plan)
}

#[derive(Clone, Debug)]
pub struct MatchedTrade {
    pub id: Uuid,
    pub bid_order_id: Uuid,
    pub ask_order_id: Uuid,
    pub buyer_id: Uuid,
    pub seller_id: Uuid,
    pub quantity_mt: Decimal,
    pub price_per_mt_usd: Decimal,
    pub confirmed_at: DateTime<Utc>,
}
#[derive(Clone, Debug)]
pub struct ChangedOrder {
    pub before: OrderRow,
    pub after: OrderRow,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedSlice {
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub side: String,
}
#[derive(Clone, Debug)]
pub struct MatchOutcome {
    pub trades: Vec<MatchedTrade>,
    pub changed_orders: Vec<ChangedOrder>,
    pub changed_slices: BTreeSet<ChangedSlice>,
}

// ---------------------------------------------------------------------------
// Party revalidation under the held locks.
// ---------------------------------------------------------------------------

fn economic_role(text: &str) -> Option<ActorRole> {
    match text {
        "BUYER" => Some(ActorRole::Buyer),
        "SUPPLIER" => Some(ActorRole::Supplier),
        _ => None,
    }
}

/// One party through the shared execution rule. Ineligibility is a skip; a
/// retryable lock or serialization failure propagates.
async fn party_admission(
    conn: &mut PgConnection,
    locked: &Locked,
    party: &PartyRef,
    bound: &MatchBound<'_>,
) -> Result<OwnerAdmission, MarketWriteError> {
    match require_execution(conn, locked, party, bound.now, bound.kyc_gate_enabled()).await {
        Ok(()) => Ok(OwnerAdmission::Admitted),
        Err(error) if error.is_retryable() => Err(error),
        Err(error)
            if matches!(
                error.code,
                MarketErrorCode::ExecutionNotAvailable | MarketErrorCode::KycRequired
            ) =>
        {
            Ok(OwnerAdmission::Refused)
        }
        Err(error) => Err(error),
    }
}

/// An assisted row executes for its customer organization: the recorded
/// administrator owns and created it, the organization is approved and REAL,
/// and one customer member locked in this transaction is execution-eligible.
/// A member outside the locked set cannot authorize.
async fn assisted_owner_admission(
    conn: &mut PgConnection,
    locked: &Locked,
    row: &OrderRow,
    owner: Uuid,
    bound: &MatchBound<'_>,
) -> Result<OwnerAdmission, MarketWriteError> {
    if row.created_by_actor_user_id != Some(owner) {
        return Ok(OwnerAdmission::Refused);
    }
    let actor =
        sqlx::query("SELECT role::text AS role,status::text AS status FROM users WHERE id=$1")
            .bind(owner)
            .fetch_optional(&mut *conn)
            .await?;
    if !actor.is_some_and(|actor| {
        actor.get::<String, _>("role") == "ADMIN" && actor.get::<String, _>("status") == "APPROVED"
    }) {
        return Ok(OwnerAdmission::Refused);
    }
    let organization = sqlx::query(
        "SELECT verification_status::text AS status,provenance::text AS provenance FROM organizations WHERE id=$1",
    )
    .bind(row.organization_id)
    .fetch_optional(&mut *conn)
    .await?;
    if !organization.is_some_and(|organization| {
        organization.get::<String, _>("status") == "APPROVED"
            && organization.get::<String, _>("provenance") == "REAL"
    }) {
        return Ok(OwnerAdmission::Refused);
    }
    let members = sqlx::query(
        "SELECT id,role::text AS role FROM users WHERE organization_id=$1 AND role::text IN ('BUYER','SUPPLIER') ORDER BY id",
    )
    .bind(row.organization_id)
    .fetch_all(&mut *conn)
    .await?;
    for member in members {
        let user_id: Uuid = member.get("id");
        if !locked.contains(LockClass::User, user_id) {
            continue;
        }
        let Some(role) = economic_role(&member.get::<String, _>("role")) else {
            continue;
        };
        let party = PartyRef {
            user_id,
            organization_id: row.organization_id,
            role,
        };
        if party_admission(conn, locked, &party, bound).await? == OwnerAdmission::Admitted {
            return Ok(OwnerAdmission::Admitted);
        }
    }
    Ok(OwnerAdmission::Refused)
}

/// Revalidate one row's owner under the held locks. A row without an owner
/// fails closed.
async fn revalidate_owner(
    conn: &mut PgConnection,
    locked: &Locked,
    row: &OrderRow,
    bound: &MatchBound<'_>,
) -> Result<OwnerAdmission, MarketWriteError> {
    let Some(owner) = row.owner_user_id else {
        return Ok(OwnerAdmission::Refused);
    };
    if row.creation_method == CreationMethod::MarketSupport {
        return assisted_owner_admission(conn, locked, row, owner, bound).await;
    }
    let role: Option<String> = sqlx::query_scalar("SELECT role::text FROM users WHERE id=$1")
        .bind(owner)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(role) = role.as_deref().and_then(economic_role) else {
        return Ok(OwnerAdmission::Refused);
    };
    let party = PartyRef {
        user_id: owner,
        organization_id: row.organization_id,
        role,
    };
    party_admission(conn, locked, &party, bound).await
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OwnerKey {
    creation_method: CreationMethod,
    owner: Option<Uuid>,
    creator: Option<Uuid>,
    organization: Uuid,
}

/// Owner decisions of one locked attempt, each party evaluated once.
#[derive(Default)]
pub(super) struct OwnerRevalidation {
    decided: BTreeMap<OwnerKey, OwnerAdmission>,
}
impl OwnerRevalidation {
    pub(super) async fn revalidate(
        &mut self,
        conn: &mut PgConnection,
        locked: &Locked,
        row: &OrderRow,
        bound: &MatchBound<'_>,
    ) -> Result<OwnerAdmission, MarketWriteError> {
        let key = OwnerKey {
            creation_method: row.creation_method,
            owner: row.owner_user_id,
            creator: row.created_by_actor_user_id,
            organization: row.organization_id,
        };
        if let Some(decision) = self.decided.get(&key) {
            return Ok(*decision);
        }
        let decision = revalidate_owner(conn, locked, row, bound).await?;
        self.decided.insert(key, decision);
        Ok(decision)
    }
}

/// Every previewed party must already be held by the prefix; no party is
/// acquired after the slice.
pub(super) fn require_parties_locked<'r>(
    locked: &Locked,
    rows: impl IntoIterator<Item = &'r OrderRow>,
) -> Result<(), MarketWriteError> {
    for row in rows {
        if !locked.contains(LockClass::Organization, row.organization_id)
            || row
                .owner_user_id
                .is_some_and(|id| !locked.contains(LockClass::User, id))
        {
            return Err(changed());
        }
    }
    Ok(())
}

/// Candidate rows are acquired first in the economic phase, before any
/// declared order, inventory or PIN resource.
pub(super) fn candidate_acquisition_closed(plan: &OperationLockPlan, locked: &Locked) -> bool {
    locked.has_acquired_economic_resources()
        || plan.trace().iter().any(|(class, _)| {
            matches!(
                class,
                LockClass::Order | LockClass::Inventory | LockClass::Pin
            )
        })
}

/// The complete party set a crossing assessment may revalidate: organizations,
/// concrete owners and, for an assisted row, its organization's BUYER and
/// SUPPLIER members. A command previews these before its prefix.
#[allow(dead_code)]
pub(super) struct DiscoveredParties {
    pub(super) organizations: Vec<Uuid>,
    pub(super) users: Vec<Uuid>,
    pub(super) fingerprint: Json,
}
pub(super) async fn discover_parties<'r>(
    conn: &mut PgConnection,
    rows: impl IntoIterator<Item = &'r OrderRow>,
    organizations: impl IntoIterator<Item = Uuid>,
) -> Result<DiscoveredParties, MarketWriteError> {
    let rows: Vec<&OrderRow> = rows.into_iter().collect();
    let assisted: Vec<Uuid> = rows
        .iter()
        .filter(|row| row.creation_method == CreationMethod::MarketSupport)
        .map(|row| row.organization_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let members: Vec<Uuid> = if assisted.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            "SELECT id FROM users WHERE organization_id=ANY($1) AND role::text IN ('BUYER','SUPPLIER') ORDER BY id",
        )
        .bind(&assisted)
        .fetch_all(&mut *conn)
        .await?
    };
    let organizations: BTreeSet<Uuid> = organizations
        .into_iter()
        .chain(rows.iter().map(|row| row.organization_id))
        .collect();
    let users: BTreeSet<Uuid> = rows
        .iter()
        .filter_map(|row| row.owner_user_id)
        .chain(members.iter().copied())
        .collect();
    Ok(DiscoveredParties {
        organizations: organizations.into_iter().collect(),
        users: users.into_iter().collect(),
        fingerprint: serde_json::json!({
            "rows": rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            "members": members,
        }),
    })
}

/// Read-only discovery for a match: the aggressor and its previewed candidates.
/// Order entry composes this into its command preview.
#[allow(dead_code)]
pub(super) async fn match_parties(
    conn: &mut PgConnection,
    aggressor: &OrderRow,
    bound: MatchBound<'_>,
) -> Result<DiscoveredParties, MarketWriteError> {
    let candidates = match aggressor_admission(aggressor, &bound) {
        Some(admission) => {
            crossing_rows(
                conn,
                &CrossingQuery::matching(aggressor, &admission),
                &bound,
            )
            .await?
        }
        None => Vec::new(),
    };
    discover_parties(conn, candidates.iter().chain([aggressor]), []).await
}

// ---------------------------------------------------------------------------
// The matching port on the sealed transaction.
// ---------------------------------------------------------------------------

/// Borrows the sealed caller transaction; no connection or commit escapes.
#[allow(dead_code)]
pub(super) struct MatchingPort<'a> {
    conn: &'a mut PgConnection,
    plan: &'a OperationLockPlan,
    locked: &'a Locked,
}
#[allow(dead_code)]
impl<'a> MatchingPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        plan: &'a OperationLockPlan,
        locked: &'a Locked,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self { conn, plan, locked })
    }
    pub(super) async fn plan_match(
        &mut self,
        aggressor: &OrderRow,
        bound: MatchBound<'_>,
    ) -> Result<MatchPlan, MarketWriteError> {
        self.locked.require_economic_phase()?;
        self.locked.require_effect_slice(&aggressor.slice())?;
        if candidate_acquisition_closed(self.plan, self.locked) {
            return Err(changed());
        }
        let mut current = load_order(self.conn, aggressor.id).await?;
        if current.stored_state() != aggressor.stored_state() {
            return Err(changed());
        }
        let Some(admission) = aggressor_admission(&current, &bound) else {
            return Ok(MatchPlan::empty(&current, bound.now));
        };
        let query = CrossingQuery::matching(&current, &admission);
        let candidates = crossing_rows(self.conn, &query, &bound).await?;
        require_parties_locked(self.locked, candidates.iter().chain([&current]))?;
        let ids: Vec<_> = candidates
            .iter()
            .take(MAX_CROSSING_ORDERS_PER_MATCH)
            .map(|row| row.id)
            .collect();
        let preview_exceeded = candidates.len() > MAX_CROSSING_ORDERS_PER_MATCH;
        #[cfg(feature = "contract-tests")]
        if let Some(barrier) = bound.preview_barrier {
            barrier.reached.notify_one();
            barrier.resume.notified().await;
        }
        let mut rows = crossing_rows(self.conn, &query.locking(&ids, false), &bound).await?;
        sqlx::query("SELECT id FROM orderbook_orders WHERE id=$1 FOR UPDATE")
            .bind(current.id)
            .fetch_one(&mut *self.conn)
            .await?;
        if load_order(self.conn, current.id).await?.stored_state() != current.stored_state() {
            return Err(changed());
        }
        if admission == AggressorAdmission::Real {
            let mut owners = OwnerRevalidation::default();
            current.owner_admission = owners
                .revalidate(self.conn, self.locked, &current, &bound)
                .await?;
            for row in &mut rows {
                row.owner_admission = owners
                    .revalidate(self.conn, self.locked, row, &bound)
                    .await?;
            }
        }
        // Overflow belongs to the preview, even when locked re-selection drops
        // rows. The look-ahead identity is never executed or locked.
        let mut result = plan_match_with_overflow(&rows, &current, bound, preview_exceeded)?;
        result.acquired_inventory = rows
            .iter()
            .take(MAX_CROSSING_ORDERS_PER_MATCH)
            .filter_map(|row| row.inventory_item_id)
            .chain(current.inventory_item_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        result.acquired_orders = ids.into_iter().chain([current.id]).collect();
        Ok(result)
    }
    pub(super) async fn apply_plan(
        &mut self,
        plan: &MatchPlan,
    ) -> Result<MatchOutcome, MarketWriteError> {
        self.locked.require_economic_phase()?;
        apply_plan(self.conn, plan).await
    }
    pub(super) async fn match_order(
        &mut self,
        aggressor: &OrderRow,
        bound: MatchBound<'_>,
    ) -> Result<MatchOutcome, MarketWriteError> {
        let plan = self.plan_match(aggressor, bound).await?;
        self.apply_plan(&plan).await
    }
}

const ORDER_COLUMNS: &str = "o.id,o.organization_id,o.owner_user_id,o.created_by_actor_user_id,o.creation_method::text AS creation_method,o.product_id,o.delivery_point_id,o.availability_window,o.side::text AS side,o.provenance::text AS provenance,(g.verification_status::text='APPROVED') AS organization_approved,o.price_per_mt_usd,o.quantity_mt,o.remaining_quantity_mt,o.acceptance_ordinal,o.version,o.status::text AS status,o.expires_at,o.created_at,o.inventory_item_id,o.off_spec,o.msds_available,o.certification_declared,o.certification_scheme,o.certifications::text AS certifications,o.fame_terms::text AS fame_terms,o.lng_terms::text AS lng_terms,o.specification_standard,o.carbon_intensity_method,o.carbon_intensity_gco2_mj,o.feedstock,o.origin";

/// A stored JSON declaration; SQL NULL and JSON `null` are both absent.
fn stored_json(text: Option<String>) -> Result<Option<Json>, MarketWriteError> {
    match text {
        None => Ok(None),
        Some(text) => match serde_json::from_str::<Json>(&text).map_err(|_| inconsistent())? {
            Json::Null => Ok(None),
            value => Ok(Some(value)),
        },
    }
}
fn stored_certifications(text: Option<String>) -> Result<Vec<String>, MarketWriteError> {
    match stored_json(text)? {
        None => Ok(Vec::new()),
        Some(Json::Array(items)) => items
            .into_iter()
            .map(|item| match item {
                Json::String(value) => Ok(value),
                _ => Err(inconsistent()),
            })
            .collect(),
        Some(_) => Err(inconsistent()),
    }
}
fn decode(row: sqlx::postgres::PgRow) -> Result<OrderRow, MarketWriteError> {
    let side: String = row.try_get("side")?;
    let status: String = row.try_get("status")?;
    let creation_method: String = row.try_get("creation_method")?;
    let flag = |name: &str| -> Result<bool, MarketWriteError> {
        Ok(row.try_get::<Option<bool>, _>(name)?.unwrap_or(false))
    };
    Ok(OrderRow {
        id: row.try_get("id")?,
        organization_id: row.try_get("organization_id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        created_by_actor_user_id: row.try_get("created_by_actor_user_id")?,
        creation_method: CreationMethod::parse(&creation_method).ok_or_else(inconsistent)?,
        product_id: row.try_get("product_id")?,
        delivery_point_id: row.try_get("delivery_point_id")?,
        availability_window: row.try_get("availability_window")?,
        side: match side.as_str() {
            "BID" => Side::Bid,
            "ASK" => Side::Ask,
            _ => return Err(inconsistent()),
        },
        provenance: row.try_get("provenance")?,
        organization_approved: row.try_get("organization_approved")?,
        price_per_mt_usd: row.try_get("price_per_mt_usd")?,
        quantity_mt: row.try_get("quantity_mt")?,
        remaining_quantity_mt: row.try_get("remaining_quantity_mt")?,
        acceptance_ordinal: row.try_get("acceptance_ordinal")?,
        version: row.try_get("version")?,
        status: match status.as_str() {
            "OPEN" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "FILLED" => OrderStatus::Filled,
            "CANCELLED" => OrderStatus::Cancelled,
            "EXPIRED" => OrderStatus::Expired,
            _ => return Err(inconsistent()),
        },
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        inventory_item_id: row.try_get("inventory_item_id")?,
        terms: ExecutionTerms {
            off_spec: flag("off_spec")?,
            msds_available: flag("msds_available")?,
            certification_declared: flag("certification_declared")?,
            certification_scheme: row.try_get("certification_scheme")?,
            certifications: stored_certifications(row.try_get("certifications")?)?,
            fame_terms: stored_json(row.try_get("fame_terms")?)?,
            lng_terms: stored_json(row.try_get("lng_terms")?)?,
            specification_standard: row.try_get("specification_standard")?,
            carbon_intensity_method: row.try_get("carbon_intensity_method")?,
            carbon_intensity_gco2_mj: row.try_get("carbon_intensity_gco2_mj")?,
            feedstock: row.try_get("feedstock")?,
            origin: row.try_get("origin")?,
        },
        owner_admission: OwnerAdmission::Unverified,
    })
}
pub(super) async fn load_order(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<OrderRow, MarketWriteError> {
    let sql = format!(
        "SELECT {ORDER_COLUMNS} FROM orderbook_orders o JOIN organizations g ON g.id=o.organization_id WHERE o.id=$1"
    );
    decode(
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(conn)
            .await?
            .ok_or_else(changed)?,
    )
}

/// The owner-admission membership test of `rs/read/sql/current_order_rule.sql`
/// over the shared eligibility sets: assisted rows use the administrator rule.
/// Callers also require an owner, so the ownerless branch is omitted.
const OWNER_ADMISSION: &str = "(CASE WHEN o.creation_method='MARKET_SUPPORT' THEN (o.owner_user_id,o.created_by_actor_user_id,o.organization_id) IN (SELECT owner_user_id,actor_user_id,organization_id FROM eligible_assisted) ELSE (o.owner_user_id,o.organization_id) IN (SELECT user_id,organization_id FROM eligible_owners) END)";

/// One bounded crossing-set statement. The preview orders by price,
/// acceptance ordinal and UUID; a locking re-selection reapplies every
/// predicate to the previewed identities.
#[derive(Clone, Copy)]
pub(super) struct CrossingQuery<'q> {
    product_id: Uuid,
    delivery_point_id: Option<Uuid>,
    availability_window: &'q str,
    resting_side: Side,
    excluded_organization: Uuid,
    provenance: &'q str,
    limit_price: Decimal,
    demo_counterparty: Option<Uuid>,
    ids: Option<&'q [Uuid]>,
    by_identity: bool,
    lock: bool,
}
impl<'q> CrossingQuery<'q> {
    fn matching(aggressor: &'q OrderRow, admission: &AggressorAdmission) -> Self {
        Self {
            product_id: aggressor.product_id,
            delivery_point_id: aggressor.delivery_point_id,
            availability_window: &aggressor.availability_window,
            resting_side: aggressor.side.opposite(),
            excluded_organization: aggressor.organization_id,
            provenance: &aggressor.provenance,
            limit_price: aggressor.price_per_mt_usd,
            demo_counterparty: match admission {
                AggressorAdmission::Real => None,
                AggressorAdmission::Demo { counterparty, .. } => Some(*counterparty),
            },
            ids: None,
            by_identity: false,
            lock: false,
        }
    }
    /// The REAL crossing set an assisted order would meet.
    pub(super) fn post_only(candidate: &'q OrderRow) -> Self {
        Self {
            product_id: candidate.product_id,
            delivery_point_id: candidate.delivery_point_id,
            availability_window: &candidate.availability_window,
            resting_side: candidate.side.opposite(),
            excluded_organization: candidate.organization_id,
            provenance: "REAL",
            limit_price: candidate.price_per_mt_usd,
            demo_counterparty: None,
            ids: None,
            by_identity: false,
            lock: false,
        }
    }
    /// Lock exactly `ids`, reapplying every predicate, in priority order or,
    /// for a non-matching assessment, ascending UUID.
    pub(super) fn locking(&self, ids: &'q [Uuid], by_identity: bool) -> Self {
        Self {
            ids: Some(ids),
            by_identity,
            lock: true,
            ..*self
        }
    }
}

pub(super) async fn crossing_rows(
    conn: &mut PgConnection,
    query: &CrossingQuery<'_>,
    bound: &MatchBound<'_>,
) -> Result<Vec<OrderRow>, MarketWriteError> {
    let (comparison, direction) = match query.resting_side {
        Side::Ask => ("<=", "ASC"),
        Side::Bid => (">=", "DESC"),
    };
    let order = if query.by_identity {
        "o.id".to_owned()
    } else {
        format!("o.price_per_mt_usd {direction},o.acceptance_ordinal,o.id")
    };
    let lock = if query.lock { " FOR UPDATE OF o" } else { "" };
    let sql = format!(
        "WITH anchor AS (SELECT 1) {sets} SELECT {ORDER_COLUMNS} FROM orderbook_orders o JOIN organizations g ON g.id=o.organization_id WHERE o.product_id=$1 AND o.delivery_point_id IS NOT DISTINCT FROM $3 AND o.availability_window=$4 AND o.side::text=$5 AND o.organization_id<>$6 AND o.provenance::text=$7 AND g.verification_status::text='APPROVED' AND o.status::text IN ('OPEN','PARTIALLY_FILLED') AND o.remaining_quantity_mt>0 AND (o.expires_at IS NULL OR o.expires_at>$2) AND o.price_per_mt_usd {comparison} $8 AND ($9::uuid[] IS NULL OR o.id=ANY($9)) AND CASE WHEN $10::uuid IS NOT NULL THEN o.id=$10 ELSE o.owner_user_id IS NOT NULL AND {OWNER_ADMISSION} END ORDER BY {order} LIMIT 101{lock}",
        sets = crate::market::visibility::eligibility_sets_sql("$11", "$2"),
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(query.product_id)
        .bind(bound.now)
        .bind(query.delivery_point_id)
        .bind(query.availability_window)
        .bind(query.resting_side.as_str())
        .bind(query.excluded_organization)
        .bind(query.provenance)
        .bind(query.limit_price)
        .bind(query.ids)
        .bind(query.demo_counterparty)
        .bind(bound.kyc_gate_enabled())
        .fetch_all(conn)
        .await?
        .into_iter()
        .map(decode)
        .collect()
}

async fn update_order(
    conn: &mut PgConnection,
    before: &OrderRow,
    after: &OrderRow,
    at: DateTime<Utc>,
) -> Result<(), MarketWriteError> {
    let result = sqlx::query("UPDATE orderbook_orders SET remaining_quantity_mt=$2,status=$3,version=$4,updated_at=$6 WHERE id=$1 AND version=$5")
        .bind(after.id).bind(after.remaining_quantity_mt).bind(after.status.as_str()).bind(after.version).bind(before.version).bind(at).execute(conn).await?;
    if result.rows_affected() != 1 {
        return Err(changed());
    }
    Ok(())
}
fn catalog_products() -> Result<serde_json::Value, MarketWriteError> {
    let policy: serde_json::Value =
        serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json"))
            .map_err(|_| inconsistent())?;
    Ok(policy["products"].clone())
}

async fn apply_plan(
    conn: &mut PgConnection,
    plan: &MatchPlan,
) -> Result<MatchOutcome, MarketWriteError> {
    apply_plan_with_fees(conn,plan,None).await
}

pub(super) async fn apply_plan_with_fees(
    conn: &mut PgConnection,
    plan: &MatchPlan,
    fee_config: Option<&crate::subscriptions::FeeConfig>,
) -> Result<MatchOutcome, MarketWriteError> {
    let mut fees=BTreeMap::new();
    if let Some(config)=fee_config {
        for fill in &plan.fills {
            let seller=if plan.aggressor_before.side==Side::Ask { plan.aggressor_before.organization_id } else {fill.resting_before.organization_id};
            if let std::collections::btree_map::Entry::Vacant(entry)=fees.entry(seller) {
                let fee=crate::subscriptions::resolve_seller_fee(conn,seller,plan.now,config).await.map_err(|error|match error {
                    crate::subscriptions::FeeError::Database(error)=>MarketWriteError::from(error),
                    _=>MarketWriteError::new(MarketErrorCode::TradePartiesInvalid),
                })?;
                entry.insert(fee);
            }
        }
    }
    let mut outcome = MatchOutcome {
        trades: Vec::new(),
        changed_orders: Vec::new(),
        changed_slices: BTreeSet::new(),
    };
    let mut aggressor = plan.aggressor_before.clone();
    for fill in &plan.fills {
        let resting = &fill.resting_before;
        let (buyer, seller) = if aggressor.side == Side::Bid {
            (&aggressor, resting)
        } else {
            (resting, &aggressor)
        };
        let trade = MatchedTrade {
            id: Uuid::new_v4(),
            bid_order_id: buyer.id,
            ask_order_id: seller.id,
            buyer_id: buyer.organization_id,
            seller_id: seller.organization_id,
            quantity_mt: fill.quantity_mt,
            price_per_mt_usd: fill.price_per_mt_usd,
            confirmed_at: plan.now,
        };
        let fee=fees.get(&seller.organization_id);
        let (fame,lng)=match &fill.terms {
            CompatibleTerms::Scalar => (None,None),
            CompatibleTerms::Fame {bid,ask} => (Some(serde_json::json!({"bid":bid.declaration,"ask":ask.declaration})),None),
            CompatibleTerms::Lng {bid,ask} => (None,Some(serde_json::json!({"bid":bid.declaration,"ask":ask.declaration}))),
        };
        let inserted = sqlx::query("INSERT INTO trades (id,bid_order_id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,buyer_provenance,seller_provenance,initiated_by,quantity_mt,price_per_mt_usd,status,confirmed_at,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version,commission_rate_pct,is_anonymous,commission_plan,commission_fee_per_mt_usd,fame_terms_snapshot,lng_terms_snapshot) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'CONFIRMED',$14,o.product_id,p.name,p.fuel_type,p.fuel_grade,catalog.code,o.delivery_point_id,dp.name,dp.region,o.availability_window,1,0,true,$17,$18,$19,$20 FROM orderbook_orders o JOIN products p ON p.id=o.product_id JOIN jsonb_to_recordset($16::jsonb) AS catalog(id uuid,code text) ON catalog.id=p.id JOIN delivery_points dp ON dp.id=o.delivery_point_id WHERE o.id=$15")
            .bind(trade.id).bind(trade.bid_order_id).bind(trade.ask_order_id).bind(trade.buyer_id).bind(trade.seller_id).bind(buyer.owner_user_id).bind(seller.owner_user_id).bind(aggressor.organization_id).bind(&buyer.provenance).bind(&seller.provenance).bind(if aggressor.side==Side::Bid { "BUYER" } else { "SELLER" }).bind(trade.quantity_mt).bind(trade.price_per_mt_usd).bind(plan.now).bind(aggressor.id).bind(catalog_products()?).bind(fee.map(|f|f.plan.as_str())).bind(fee.map(|f|f.seller_fee_per_mt_usd)).bind(fame).bind(lng).execute(&mut *conn).await?;
        if inserted.rows_affected() != 1 {
            return Err(inconsistent());
        }

        if fee_config.is_some() {
            super::confirmed_effect::on_trade_confirmed(conn,trade.id,trade.buyer_id,trade.seller_id,plan.now).await?;
        }
        update_order(conn, resting, &fill.resting_after, plan.now).await?;
        update_order(conn, &aggressor, &fill.aggressor_after, plan.now).await?;
        aggressor = fill.aggressor_after.clone();
        outcome.changed_orders.push(ChangedOrder {
            before: resting.clone(),
            after: fill.resting_after.clone(),
        });
        outcome.trades.push(trade);
    }
    if !plan.fills.is_empty() {
        outcome.changed_orders.push(ChangedOrder {
            before: plan.aggressor_before.clone(),
            after: aggressor,
        });
    }
    for change in &outcome.changed_orders {
        let row = &change.after;
        outcome.changed_slices.insert(ChangedSlice {
            product_id: row.product_id,
            delivery_point_id: row.delivery_point_id,
            availability_window: row.availability_window.clone(),
            side: row.side.as_str().to_owned(),
        });
    }
    Ok(outcome)
}

#[cfg(feature = "contract-tests")]
pub mod contract {
    use super::super::locks::{MarketOperation, PartyLockPreview};
    use super::*;

    #[derive(Debug)]
    pub struct PlannedMatch {
        pub plan: MatchPlan,
    }
    #[derive(Debug)]
    pub struct SealedMatchResult {
        pub outcome: MatchOutcome,
        pub captured_orders: Vec<Uuid>,
        pub order_lock_order: Vec<Uuid>,
        pub inventory_after_orders: bool,
    }
    /// Execution inputs a sealed contract match supplies to its bound.
    #[derive(Clone, Copy, Debug)]
    pub struct SealedMatchOptions {
        pub kyc_gate_enabled: bool,
        pub demo_pair: Option<DemoOrderPair>,
        /// The locked actor when the aggressor has no owner.
        pub actor: Option<Uuid>,
    }
    impl Default for SealedMatchOptions {
        fn default() -> Self {
            Self {
                kyc_gate_enabled: true,
                demo_pair: None,
                actor: None,
            }
        }
    }
    fn bound_for(at: DateTime<Utc>, options: &SealedMatchOptions) -> MatchBound<'static> {
        let bound = MatchBound::new(at).with_kyc_gate(options.kyc_gate_enabled);
        match options.demo_pair {
            Some(pair) => bound.with_demo_pair(pair),
            None => bound,
        }
    }
    #[derive(Default)]
    pub struct MatchPreviewBarrier {
        pub(super) reached: tokio::sync::Notify,
        pub(super) resume: tokio::sync::Notify,
    }
    impl MatchPreviewBarrier {
        pub async fn previewed(&self) {
            self.reached.notified().await;
        }
        pub fn resume(&self) {
            self.resume.notify_one();
        }
    }
    struct SealedMatch {
        barrier: Option<std::sync::Arc<MatchPreviewBarrier>>,
        aggressor: Uuid,
        at: DateTime<Utc>,
        options: SealedMatchOptions,
    }
    impl super::super::transaction::MarketCommand for SealedMatch {
        type Output = SealedMatchResult;
        fn preview<'a>(
            &'a self,
            conn: &'a mut PgConnection,
            actor: Uuid,
        ) -> super::super::transaction::BoxFuture<
            'a,
            Result<
                super::super::transaction::MarketPreview,
                super::super::transaction::MarketAttemptError,
            >,
        > {
            Box::pin(async move {
                let aggressor = load_order(conn, self.aggressor).await?;
                let discovered =
                    match_parties(conn, &aggressor, bound_for(self.at, &self.options)).await?;
                let parties = PartyLockPreview::new(
                    discovered.organizations,
                    discovered.users,
                    actor,
                    discovered.fingerprint,
                );
                let mut locks = OperationLockPlan::new(MarketOperation::Amend);
                locks.parties(&parties)?.slices([aggressor.slice()])?;
                Ok(super::super::transaction::MarketPreview { parties, locks })
            })
        }
        fn execute<'a>(
            &'a self,
            tx: &'a mut super::super::transaction::LockedMarketTransaction<'_>,
            _actor: &'a super::super::AuthenticatedActor,
        ) -> super::super::transaction::BoxFuture<
            'a,
            Result<Self::Output, super::super::transaction::MarketAttemptError>,
        > {
            Box::pin(async move {
                let aggressor = tx.matching_order(self.aggressor).await?;
                let mut bound = bound_for(self.at, &self.options);
                bound.preview_barrier = self.barrier.as_deref();
                let plan = tx.prepare_match(&aggressor, bound).await?;
                let ids = plan.locked_order_ids();
                let policy: crate::MarketPolicy = serde_json::from_str(include_str!(
                    "../../../../deploy/rust/market-policy.json"
                ))
                .map_err(|_| inconsistent())?;
                let config: crate::Config = serde_json::from_value(serde_json::json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).map_err(|_| inconsistent())?;
                let _before = tx
                    .effects()?
                    .capture_before(&policy, &config, &ids, self.at)
                    .await?;
                tx.stock_ledger()?.verify_invariants().await?;
                let outcome = tx.apply_match(&plan).await?;
                for fill in plan.fills() {
                    for item in [
                        fill.resting_before.inventory_item_id,
                        plan.aggressor_before.inventory_item_id,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        tx.stock_ledger()?
                            .consume_reserved(
                                item,
                                super::super::StockQuantity::checked(fill.quantity_mt)?,
                            )
                            .await?;
                    }
                }
                tx.stock_ledger()?.verify_invariants().await?;
                let trace = tx.matching_trace();
                let first_inventory = trace
                    .iter()
                    .position(|(class, _)| *class == LockClass::Inventory)
                    .unwrap_or(trace.len());
                let inventory_after_orders = trace[first_inventory..]
                    .iter()
                    .all(|(class, _)| *class != LockClass::Order);
                Ok(SealedMatchResult {
                    outcome,
                    captured_orders: ids,
                    order_lock_order: trace
                        .iter()
                        .filter(|(class, _)| *class == LockClass::Order)
                        .map(|(_, id)| *id)
                        .collect(),
                    inventory_after_orders,
                })
            })
        }
    }
    pub async fn run_sealed_match(
        pool: &sqlx::PgPool,
        aggressor: Uuid,
        at: DateTime<Utc>,
    ) -> Result<SealedMatchResult, MarketWriteError> {
        run_sealed_match_with(pool, aggressor, at, SealedMatchOptions::default()).await
    }
    pub async fn run_sealed_match_with(
        pool: &sqlx::PgPool,
        aggressor: Uuid,
        at: DateTime<Utc>,
        options: SealedMatchOptions,
    ) -> Result<SealedMatchResult, MarketWriteError> {
        run_sealed_match_inner(pool, aggressor, at, options, None).await
    }
    pub async fn run_sealed_match_paused(
        pool: sqlx::PgPool,
        aggressor: Uuid,
        at: DateTime<Utc>,
        barrier: std::sync::Arc<MatchPreviewBarrier>,
    ) -> Result<SealedMatchResult, MarketWriteError> {
        run_sealed_match_inner(
            &pool,
            aggressor,
            at,
            SealedMatchOptions::default(),
            Some(barrier),
        )
        .await
    }
    async fn run_sealed_match_inner(
        pool: &sqlx::PgPool,
        aggressor: Uuid,
        at: DateTime<Utc>,
        options: SealedMatchOptions,
        barrier: Option<std::sync::Arc<MatchPreviewBarrier>>,
    ) -> Result<SealedMatchResult, MarketWriteError> {
        let mut discovery = pool.acquire().await?;
        let row = load_order(&mut discovery, aggressor).await?;
        drop(discovery);
        let subject = match options.actor {
            Some(actor) => actor,
            None => row.owner_user_id.ok_or_else(changed)?,
        };
        super::super::transaction::run_market_command(
            pool.clone(),
            std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            crate::auth::VerifiedAccess {
                subject,
                issued_at: Some(Utc::now()),
                issuance_out_of_range: false,
            },
            SealedMatch {
                barrier,
                aggressor,
                at,
                options,
            },
            super::super::AdmissionClass::Cleanup,
        )
        .await
    }
    /// Configurable enclosing command for completion qualification; not a serving route.
    #[derive(Clone,Debug)]
    pub struct CompletionOptions {
        pub fee_config:crate::subscriptions::FeeConfig,
        pub receipt_key:Option<String>,
        pub request_id:Option<String>,
        pub demo_pair:Option<DemoOrderPair>,
        pub actor:Option<Uuid>,
        pub fail_after_effects:bool,
    }
    impl Default for CompletionOptions {
        fn default()->Self { Self {fee_config:crate::subscriptions::FeeConfig::default(),receipt_key:None,request_id:None,demo_pair:None,actor:None,fail_after_effects:false} }
    }
    #[derive(Default)]
    pub struct CompletionBarrier { entered:tokio::sync::Notify,release:tokio::sync::Notify }
    impl CompletionBarrier {
        pub async fn staged(&self) {self.entered.notified().await;}
        pub fn resume(&self) {self.release.notify_one();}
    }
    #[derive(Debug)]
    pub struct CompletionResult {pub trade_ids:Vec<Uuid>,pub replayed:bool}
    struct CompletedMatch {aggressor:Uuid,at:DateTime<Utc>,options:CompletionOptions,barrier:Option<std::sync::Arc<CompletionBarrier>>}
    impl super::super::transaction::MarketCommand for CompletedMatch {
        type Output=CompletionResult;
        fn preview<'a>(&'a self,conn:&'a mut PgConnection,actor:Uuid)->super::super::transaction::BoxFuture<'a,Result<super::super::transaction::MarketPreview,super::super::transaction::MarketAttemptError>> {
            Box::pin(async move {
                let order=load_order(conn,self.aggressor).await?;
                let mut bound=MatchBound::new(self.at);
                if let Some(pair)=self.options.demo_pair {bound=bound.with_demo_pair(pair);}
                let discovered=match_parties(conn,&order,bound).await?;
                let parties=PartyLockPreview::new(discovered.organizations,discovered.users,actor,discovered.fingerprint);
                let mut locks=OperationLockPlan::new(MarketOperation::Amend);
                locks.parties(&parties)?;
                if let Some(key)=&self.options.receipt_key {locks.idempotency(actor,super::super::Operation::OrderAmend.as_str(),key)?;}
                locks.slices([order.slice()])?;
                Ok(super::super::transaction::MarketPreview {parties,locks})
            })
        }
        fn replay<'a>(&'a self,tx:&'a mut super::super::transaction::LockedMarketTransaction<'_>,actor:&'a super::super::AuthenticatedActor)->super::super::transaction::BoxFuture<'a,Result<Option<Self::Output>,super::super::transaction::MarketAttemptError>> {
            Box::pin(async move {
                let Some(key)=&self.options.receipt_key else {return Ok(None)};
                let order=tx.matching_order(self.aggressor).await?;
                let context=tx.match_receipt_context(actor,order.organization_id,self.at).await?;
                let receipt=tx.replay_receipt(&context,super::super::Operation::OrderAmend,self.aggressor,key,"completion-v1").await?;
                receipt.map(|receipt| {
                    let trade_ids=serde_json::from_value(receipt.response_body).map_err(|_|inconsistent())?;
                    Ok(CompletionResult {trade_ids,replayed:true})
                }).transpose()
            })
        }
        fn execute<'a>(&'a self,tx:&'a mut super::super::transaction::LockedMarketTransaction<'_>,actor:&'a super::super::AuthenticatedActor)->super::super::transaction::BoxFuture<'a,Result<Self::Output,super::super::transaction::MarketAttemptError>> {
            Box::pin(async move {
                let order=tx.matching_order(self.aggressor).await?;
                let mut bound=MatchBound::new(self.at);
                if let Some(pair)=self.options.demo_pair {bound=bound.with_demo_pair(pair);}
                let plan=tx.prepare_match(&order,bound).await?;
                let policy:crate::MarketPolicy=serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json")).map_err(|_|inconsistent())?;
                let config:crate::Config=serde_json::from_value(serde_json::json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).map_err(|_|inconsistent())?;
                let before=tx.effects()?.capture_before(&policy,&config,&plan.locked_order_ids(),self.at).await?;
                let outcome=tx.complete_match(&plan,&self.options.fee_config).await?;
                let trade_ids:Vec<Uuid>=outcome.trades.iter().map(|trade|trade.id).collect();
                let mut effects=before.stage();
                let request=axum::http::Request::builder().header("x-request-id",self.options.request_id.as_deref().unwrap_or("completion")).body(()).map_err(|_|inconsistent())?;
                let (mut parts,_)=request.into_parts();
                parts.extensions.insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127,0,0,1],12345))));
                let (ip,request_id)=crate::runtime::http::audit_identity(&parts);
                let audit=super::super::effects::AuditContext::new(Some(ip),Some(request_id))?;
                tx.effects()?.matched(&mut effects,&outcome,&order,actor.user_id(),audit).await?;
                if let Some(key)=&self.options.receipt_key {
                    let context=tx.match_receipt_context(actor,order.organization_id,self.at).await?;
                    tx.record_receipt(&context,&super::super::ReceiptRecord {id:Uuid::new_v4(),actor_user_id:actor.user_id(),effective_organization_id:order.organization_id,support_context_id:None,operation:super::super::Operation::OrderAmend,idempotency_key:key.clone(),request_hash:"completion-v1".into(),resource_id:order.id,response_status:200,response_body:serde_json::json!(trade_ids)}).await?;
                }
                if !outcome.trades.is_empty() {tx.effects()?.finish(&policy,&config,effects).await?;}
                if self.options.fail_after_effects {return Err(inconsistent().into());}
                if let Some(barrier)=&self.barrier {barrier.entered.notify_one();barrier.release.notified().await;}
                Ok(CompletionResult {trade_ids,replayed:false})
            })
        }
    }
    pub async fn run_completed_match(pool:&sqlx::PgPool,aggressor:Uuid,at:DateTime<Utc>,options:CompletionOptions)->Result<CompletionResult,MarketWriteError> {
        completed(pool,aggressor,at,options,None).await
    }
    pub async fn run_completed_match_paused(pool:&sqlx::PgPool,aggressor:Uuid,at:DateTime<Utc>,options:CompletionOptions,barrier:std::sync::Arc<CompletionBarrier>)->Result<CompletionResult,MarketWriteError> {
        completed(pool,aggressor,at,options,Some(barrier)).await
    }
    async fn completed(pool:&sqlx::PgPool,aggressor:Uuid,at:DateTime<Utc>,options:CompletionOptions,barrier:Option<std::sync::Arc<CompletionBarrier>>)->Result<CompletionResult,MarketWriteError> {
        let mut conn=pool.acquire().await?;
        let row=load_order(&mut conn,aggressor).await?;drop(conn);
        let subject=options.actor.or(row.owner_user_id).ok_or_else(changed)?;
        super::super::transaction::run_market_command(pool.clone(),std::sync::Arc::new(tokio::sync::Semaphore::new(1)),crate::auth::VerifiedAccess {subject,issued_at:Some(Utc::now()),issuance_out_of_range:false},CompletedMatch {aggressor,at,options,barrier},super::super::AdmissionClass::Cleanup).await
    }
    pub async fn probe_order_lock(conn: &mut PgConnection, id: Uuid) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(id)
            .fetch_one(conn)
            .await
    }
    pub async fn preview(
        conn: &mut PgConnection,
        aggressor: Uuid,
        bound: MatchBound<'_>,
    ) -> Result<Vec<OrderRow>, MarketWriteError> {
        let aggressor = load_order(conn, aggressor).await?;
        match aggressor_admission(&aggressor, &bound) {
            Some(admission) => {
                crossing_rows(
                    conn,
                    &CrossingQuery::matching(&aggressor, &admission),
                    &bound,
                )
                .await
            }
            None => Ok(Vec::new()),
        }
    }
    /// The read-only party discovery of a command, kept apart from its lock
    /// acquisition so tests can change state between the two.
    pub struct PreviewedParties {
        aggressor: OrderRow,
        parties: PartyLockPreview,
    }
    impl PreviewedParties {
        pub fn users(&self) -> &[Uuid] {
            self.parties.users()
        }
    }
    pub async fn preview_parties(
        conn: &mut PgConnection,
        aggressor: Uuid,
        actor: Uuid,
        bound: MatchBound<'_>,
    ) -> Result<PreviewedParties, MarketWriteError> {
        let aggressor = load_order(conn, aggressor).await?;
        let discovered = match_parties(conn, &aggressor, bound).await?;
        let parties = PartyLockPreview::new(
            discovered.organizations,
            discovered.users,
            actor,
            discovered.fingerprint,
        );
        Ok(PreviewedParties { aggressor, parties })
    }
    /// Acquire exactly the previewed prefix, then plan through the port.
    pub async fn lock_previewed_and_plan(
        conn: &mut PgConnection,
        previewed: &PreviewedParties,
        bound: MatchBound<'_>,
    ) -> Result<PlannedMatch, MarketWriteError> {
        let mut locks = OperationLockPlan::new(MarketOperation::Amend);
        locks
            .parties(&previewed.parties)?
            .slices([previewed.aggressor.slice()])?;
        let held = locks.acquire(conn).await?;
        let plan = MatchingPort::new(conn, &locks, &held)?
            .plan_match(&previewed.aggressor, bound)
            .await?;
        Ok(PlannedMatch { plan })
    }
    pub async fn lock_and_plan_as(
        conn: &mut PgConnection,
        aggressor: Uuid,
        actor: Uuid,
        bound: MatchBound<'_>,
    ) -> Result<PlannedMatch, MarketWriteError> {
        let previewed = preview_parties(conn, aggressor, actor, bound).await?;
        lock_previewed_and_plan(conn, &previewed, bound).await
    }
    pub async fn lock_and_plan(
        conn: &mut PgConnection,
        aggressor: Uuid,
        bound: MatchBound<'_>,
    ) -> Result<PlannedMatch, MarketWriteError> {
        let actor = load_order(conn, aggressor)
            .await?
            .owner_user_id
            .ok_or_else(changed)?;
        lock_and_plan_as(conn, aggressor, actor, bound).await
    }

    pub async fn apply(
        conn: &mut PgConnection,
        planned: PlannedMatch,
    ) -> Result<MatchOutcome, MarketWriteError> {
        apply_plan(conn, &planned.plan).await
    }
}
