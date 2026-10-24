//! Marketplace tab composition and the shared analytics response primitives.
//!
//! Phase 4 Task 19 owns three product-analytics tabs: acquisition (P06),
//! marketplace (P09) and reliability (P11). They share one query vocabulary,
//! one suppression rule, one provenance rule and one meta/coverage shape, so
//! those live here. `liquidity`, `commercial`, `acquisition` and `reliability`
//! compose tabs; only this module touches market rows.
//!
//! Rules carried over from the Python business reference
//! (`app/services/product_analytics.py`) and frozen by CF4_03:
//!
//! - Periods are half-open UTC intervals `[start, end)`; the comparison period
//!   is `[start - (end - start), start)` and its arithmetic is checked (§CF4_03
//!   `QueryInput`).
//! - Live, demo and unknown provenance are separate sections and are never
//!   summed. Reference coverage is a different source again: it is coverage,
//!   never activity.
//! - A cell backed by fewer than three entities (or events, when entity counts
//!   are unavailable) is suppressed: `count = null`, `suppressed = true`. Zero
//!   is a genuine zero.
//! - Unavailable and not-requested provider components carry `null`, never a
//!   fabricated zero or empty collection.
//!
//! Scope note: the CF4_03 query/suppression ABI (`analytics::policy::
//! validate_query` and `analytics::policy::suppress`) belongs to Task 15, not
//! this lane. This module implements the same rules locally as `parse_query`
//! and `suppress_cell` because Task 15's source is not integrated yet; the
//! names are deliberately different so the lane publishes no competing ABI.
//! Integration binds the real policy and deletes the local copies before
//! acceptance.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::market::{policy as catalog, windows};
use crate::{ApiError, Json, MarketPolicy};

/// Small-cell threshold: one or two underlying entities stay private.
pub const SUPPRESSION_THRESHOLD: i64 = 3;

/// Bounds mirrored from the Python schemas (`MAX_SERIES_POINTS`,
/// `_MAX_RANKED_ROWS`, `_MAX_MATRIX_CELLS`, reliability audit sample).
pub const MAX_SERIES_POINTS: usize = 366;
pub const MAX_RANKED_ROWS: usize = 20;
pub const MAX_MATRIX_CELLS: usize = 512;
pub const MAX_AUDIT_ROWS: usize = 20;

// ---------------------------------------------------------------------------
// Decimal wire helpers
// ---------------------------------------------------------------------------

/// Decimal fields keep Python's string spelling on the wire: the frontend
/// applies `Number()` itself (`client-contract.json` decimal policy).
pub mod wire {
    use rust_decimal::Decimal;
    use serde::Serializer;

    pub fn decimal<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub fn decimal_option<S: Serializer>(
        value: &Option<Decimal>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_some(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    All,
    Buyer,
    Supplier,
}

impl Audience {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ALL" => Some(Self::All),
            "BUYER" => Some(Self::Buyer),
            "SUPPLIER" => Some(Self::Supplier),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Live,
    Demo,
    Reference,
    All,
}

impl Activity {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "LIVE" => Some(Self::Live),
            "DEMO" => Some(Self::Demo),
            "REFERENCE" => Some(Self::Reference),
            "ALL" => Some(Self::All),
            _ => None,
        }
    }

    /// Whether this request asks for order/trade aggregates at all.
    pub fn wants_activity(self) -> bool {
        matches!(self, Self::Live | Self::Demo | Self::All)
    }

    pub fn wants_live(self) -> bool {
        matches!(self, Self::Live | Self::All)
    }

    pub fn wants_demo(self) -> bool {
        matches!(self, Self::Demo | Self::All)
    }

    pub fn wants_unknown(self) -> bool {
        matches!(self, Self::All)
    }

    pub fn wants_reference(self) -> bool {
        matches!(self, Self::Reference | Self::All)
    }
}

/// A validated analytics period. Construction is the parser's job: every
/// field here has already passed the CF4_03 checks, including the checked
/// previous-window subtraction.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalyticsQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub compare: bool,
    pub audience: Audience,
    pub activity: Activity,
    pub product_id: Option<Uuid>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: Option<String>,
}

impl AnalyticsQuery {
    /// The comparison window, or `None` when comparison is off. Callers build a
    /// query through the parser, which refuses an underflowing subtraction.
    pub fn previous(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        if !self.compare {
            return None;
        }
        let elapsed = self.end - self.start;
        Some((self.start.checked_sub_signed(elapsed)?, self.start))
    }

    pub fn filters(&self) -> Filters {
        Filters {
            product_id: self.product_id,
            delivery_point_id: self.delivery_point_id,
            availability_window: self.availability_window.clone(),
        }
    }
}

/// The canonical marketplace slice filters. Trades and commissions qualify
/// through a linked order, never through guessed legacy columns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub product_id: Option<Uuid>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: Option<String>,
}

impl Filters {
    pub fn active(&self) -> bool {
        self.product_id.is_some()
            || self.delivery_point_id.is_some()
            || self.availability_window.is_some()
    }
}

/// Reference activity is accepted only by the marketplace tab.
pub fn refuse_reference(query: &AnalyticsQuery) -> Result<(), ApiError> {
    if query.activity == Activity::Reference {
        return Err(invalid_query(
            "REFERENCE activity is accepted only by the marketplace tab",
        ));
    }
    Ok(())
}

pub fn invalid_query(message: &'static str) -> ApiError {
    crate::business::error::refusal(
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "ANALYTICS_QUERY_INVALID",
        message,
    )
}

pub fn invalid_property(message: &'static str) -> ApiError {
    crate::business::error::refusal(
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "ANALYTICS_PROPERTY_INVALID",
        message,
    )
}

// ---------------------------------------------------------------------------
// Meta, coverage and data quality
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Available,
    Partial,
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Diagnostic {
    Disabled,
    Configuration,
    Authentication,
    Timeout,
    Upstream,
    MalformedResponse,
    InsufficientCoverage,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceCoverage {
    pub coverage_start: Option<DateTime<Utc>>,
    pub coverage_end: Option<DateTime<Utc>>,
    pub observed_at: Option<DateTime<Utc>>,
    pub status: SourceStatus,
    pub diagnostic: Option<Diagnostic>,
}

impl SourceCoverage {
    pub fn not_applicable() -> Self {
        Self {
            coverage_start: None,
            coverage_end: None,
            observed_at: None,
            status: SourceStatus::NotApplicable,
            diagnostic: None,
        }
    }

    pub fn available(observed_at: DateTime<Utc>) -> Self {
        Self {
            coverage_start: None,
            coverage_end: None,
            observed_at: Some(observed_at),
            status: SourceStatus::Available,
            diagnostic: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Coverage {
    pub authoritative: SourceCoverage,
    pub behavioral: SourceCoverage,
    pub login_history: SourceCoverage,
    pub status_history: SourceCoverage,
    pub reference: SourceCoverage,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DataQuality {
    pub legacy_timestamp_fallback_count: i64,
    pub missing_paid_at_count: i64,
    pub missing_commission_payment_date_count: i64,
    pub suppressed_cell_count: i64,
    pub cohort_complete: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Meta {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub previous_start: Option<DateTime<Utc>>,
    pub previous_end: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub coverage: Coverage,
    pub data_quality: DataQuality,
}

/// Per-request suppression accounting. Every suppressed cell increments the
/// count that `meta.data_quality.suppressed_cell_count` publishes.
#[derive(Clone, Debug)]
pub struct Quality {
    pub legacy_timestamp_fallback_count: i64,
    pub missing_paid_at_count: i64,
    pub missing_commission_payment_date_count: i64,
    pub suppressed_cell_count: i64,
    pub cohort_complete: bool,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            legacy_timestamp_fallback_count: 0,
            missing_paid_at_count: 0,
            missing_commission_payment_date_count: 0,
            suppressed_cell_count: 0,
            cohort_complete: true,
        }
    }
}

impl Quality {
    pub fn data_quality(&self) -> DataQuality {
        DataQuality {
            legacy_timestamp_fallback_count: self.legacy_timestamp_fallback_count,
            missing_paid_at_count: self.missing_paid_at_count,
            missing_commission_payment_date_count: self.missing_commission_payment_date_count,
            suppressed_cell_count: self.suppressed_cell_count,
            cohort_complete: self.cohort_complete,
        }
    }
}

/// Suppression decision, shared with Task 15's exported `suppress`.
///
/// `0 < basis < 3` suppresses; a genuine zero stays visible; an unknown count
/// stays unknown and is never suppressed. `entities` falls back to the event
/// count when no distinct-entity count exists.
pub fn suppress_cell(count: Option<i64>, entities: Option<i64>, quality: &mut Quality) -> (Option<i64>, bool) {
    let Some(count) = count else {
        return (None, false);
    };
    let basis = entities.unwrap_or(count);
    if basis > 0 && basis < SUPPRESSION_THRESHOLD {
        quality.suppressed_cell_count += 1;
        (None, true)
    } else {
        (Some(count), false)
    }
}

pub fn cell(
    key: impl Into<String>,
    count: Option<i64>,
    entities: Option<i64>,
    quality: &mut Quality,
) -> AggregateCell {
    let (count, suppressed) = suppress_cell(count, entities, quality);
    AggregateCell {
        key: key.into(),
        count,
        suppressed,
    }
}

pub fn metric(
    value: Option<i64>,
    previous: Option<i64>,
    segmented: bool,
    quality: &mut Quality,
) -> MetricValue {
    if !segmented {
        return MetricValue {
            value,
            previous,
            suppressed: false,
        };
    }
    let (value, suppressed_value) = suppress_cell(value, None, quality);
    let (previous, suppressed_previous) = suppress_cell(previous, None, quality);
    MetricValue {
        value,
        previous,
        suppressed: suppressed_value || suppressed_previous,
    }
}

// ---------------------------------------------------------------------------
// Response primitives
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct AggregateCell {
    pub key: String,
    pub count: Option<i64>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricValue {
    pub value: Option<i64>,
    pub previous: Option<i64>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DecimalMetricValue {
    #[serde(serialize_with = "wire::decimal_option")]
    pub value: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub previous: Option<Decimal>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RatioValue {
    pub numerator: Option<i64>,
    pub denominator: Option<i64>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub rate_pct: Option<Decimal>,
    pub suppressed: bool,
    pub cohort_complete: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SeriesPoint {
    pub date: NaiveDate,
    pub value: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RankedRow {
    pub key: String,
    pub label: String,
    pub count: Option<i64>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub share_pct: Option<Decimal>,
    pub suppressed: bool,
}

// ---------------------------------------------------------------------------
// Arithmetic helpers (Python `_money`, `_quantity`, `_rate_pct`, `_median_hours`)
// ---------------------------------------------------------------------------

/// Quantize to two fractional places and keep that scale on the wire, exactly
/// like Python's `Decimal.quantize(Decimal("0.01"))`.
pub fn money(value: Decimal) -> Decimal {
    let mut value = value.round_dp(2);
    value.rescale(2);
    value
}

/// Quantize to one fractional place (`Decimal("0.1")`).
pub fn one_decimal(value: Decimal) -> Decimal {
    let mut value = value.round_dp(1);
    value.rescale(1);
    value
}

/// `ROUND_HALF_UP` percentage, `None` for a non-positive denominator.
pub fn rate_pct(numerator: i64, denominator: i64) -> Option<Decimal> {
    if denominator <= 0 {
        return None;
    }
    let mut value = (Decimal::from(numerator) / Decimal::from(denominator) * Decimal::from(100))
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    value.rescale(2);
    Some(value)
}

/// Share of a total, `None` when the total is zero.
pub fn share_pct(count: i64, total: i64) -> Option<Decimal> {
    rate_pct(count, total)
}

/// Half-open date bounds for date-only facts projected to 00:00:00Z. A midday
/// start excludes that date; a midday end includes it.
pub fn date_window(
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<(NaiveDate, NaiveDate)> {
    let (start, end) = (start?, end?);
    let start_date = start.date_naive();
    let lower = if start == start_date.and_hms_opt(0, 0, 0)?.and_utc() {
        start_date
    } else {
        start_date.succ_opt()?
    };
    let end_date = end.date_naive();
    let upper = if end == end_date.and_hms_opt(0, 0, 0)?.and_utc() {
        end_date
    } else {
        end_date.succ_opt()?
    };
    Some((lower, upper))
}

fn seconds(delta: TimeDelta) -> Decimal {
    Decimal::from(delta.num_microseconds().unwrap_or_default()) / Decimal::from(1_000_000)
}

/// Median in hours, one fractional place; `None` for an empty sample.
pub fn median_hours(deltas: &[TimeDelta]) -> Option<Decimal> {
    if deltas.is_empty() {
        return None;
    }
    let mut values: Vec<Decimal> = deltas.iter().copied().map(seconds).collect();
    values.sort();
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / Decimal::TWO
    } else {
        values[middle]
    };
    Some(one_decimal(median / Decimal::from(3600)))
}

/// One dense UTC point. A day with no recorded rows is a genuine zero when the
/// source answered for the whole period; a one- or two-event day is suppressed.
fn series_point(date: NaiveDate, values: &BTreeMap<NaiveDate, i64>, quality: &mut Quality) -> SeriesPoint {
    let raw = values.get(&date).copied().unwrap_or(0);
    let (value, _) = suppress_cell(Some(raw), Some(raw), quality);
    SeriesPoint { date, value }
}

/// Dense UTC date series across a half-open period (service variant, which
/// includes a period end that is not midnight).
pub fn dense_series(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    values: &BTreeMap<NaiveDate, i64>,
    quality: &mut Quality,
) -> Vec<SeriesPoint> {
    let mut points = Vec::new();
    let mut cursor = start.date_naive();
    let end_date = end.date_naive();
    let midnight = end.time() == chrono::NaiveTime::MIN;
    while cursor < end_date || (cursor == end_date && !midnight) {
        points.push(series_point(cursor, values, quality));
        let Some(next) = cursor.succ_opt() else { break };
        cursor = next;
        if points.len() >= MAX_SERIES_POINTS {
            break;
        }
    }
    points
}

/// Daily series for the day-bounded provider windows: `cursor < end.date()`.
pub fn daily_series(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    values: &BTreeMap<NaiveDate, i64>,
    quality: &mut Quality,
) -> Vec<SeriesPoint> {
    let mut points = Vec::new();
    let mut cursor = start.date_naive();
    let end_date = end.date_naive();
    while cursor < end_date && points.len() < MAX_SERIES_POINTS {
        points.push(series_point(cursor, values, quality));
        let Some(next) = cursor.succ_opt() else { break };
        cursor = next;
    }
    points
}

/// Python `slugify`: a stable lowercase key for a catalog label, never an id.
pub fn slugify(value: &str) -> String {
    let mut cleaned = String::new();
    for character in value.trim().to_lowercase().chars() {
        if character.is_alphanumeric() {
            cleaned.push(character);
        } else {
            cleaned.push('-');
        }
    }
    while cleaned.contains("--") {
        cleaned = cleaned.replace("--", "-");
    }
    cleaned.trim_matches('-').to_owned()
}

/// The canonical product code for an exact catalog label tuple.
///
/// This is the Python `derive_market_product` lookup: a label tuple maps to a
/// code, or the caller falls back to [`slugify`]. Catalog *admission* is a
/// different rule and uses [`catalog::product_matches`].
pub fn derive_product_code<'a>(
    policy: &'a MarketPolicy,
    name: &str,
    fuel_type: &str,
    fuel_grade: &str,
) -> Option<&'a str> {
    policy
        .products
        .iter()
        .find(|product| {
            product.name == name
                && product.fuel_type == fuel_type
                && product.fuel_grade == fuel_grade
        })
        .map(|product| product.code.as_str())
}

/// Display label for a stored availability window, falling back to the code.
pub fn window_label(value: &str) -> String {
    windows::window_display_label(value).unwrap_or_else(|_| value.to_owned())
}

/// Map a database failure to the shared typed error. `57P03`/`57014` keep their
/// own statuses; everything else is a plain database failure.
pub fn database(error: sqlx::Error) -> ApiError {
    crate::business::error::database_failure(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
    )
}

// ---------------------------------------------------------------------------
// Marketplace response vocabulary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct MarketplaceKpis {
    pub participating_organizations: MetricValue,
    pub open_bids: MetricValue,
    pub open_asks: MetricValue,
    pub confirmed_trades: MetricValue,
    pub confirmed_volume_mt: DecimalMetricValue,
    pub execution_rate: RatioValue,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MarketBalanceTrend {
    pub buyer_organizations: Vec<SeriesPoint>,
    pub supplier_organizations: Vec<SeriesPoint>,
    pub bids: Vec<SeriesPoint>,
    pub asks: Vec<SeriesPoint>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductPortCell {
    pub product_key: String,
    pub product_label: String,
    pub delivery_point_key: String,
    pub delivery_point_label: String,
    pub orders: AggregateCell,
    pub organizations: AggregateCell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConcentrationBand {
    Low,
    Moderate,
    High,
}

#[derive(Clone, Debug, Serialize)]
pub struct OrganizationConcentration {
    pub hhi_band: Option<ConcentrationBand>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MarketActivitySection {
    pub kpis: MarketplaceKpis,
    pub liquidity: super::liquidity::LiquiditySummary,
    pub balance_trend: MarketBalanceTrend,
    pub product_port_matrix: Vec<ProductPortCell>,
    pub window_distribution: Vec<RankedRow>,
    pub order_status_distribution: Vec<AggregateCell>,
    pub trade_status_distribution: Vec<AggregateCell>,
    pub concentration: OrganizationConcentration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ReferenceSourceLabel {
    #[serde(rename = "MANUAL_OVERRIDE")]
    ManualOverride,
    #[serde(rename = "SEED_MATRIX")]
    SeedMatrix,
    #[serde(rename = "OTHER")]
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ReferenceSourceKind {
    #[serde(rename = "ADMIN_BENCHMARK")]
    AdminBenchmark,
    #[serde(rename = "SEEDED_BENCHMARK")]
    SeededBenchmark,
    #[serde(rename = "TRADE_DERIVED_VWAP")]
    TradeDerivedVwap,
    #[serde(rename = "OTHER")]
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ReferenceScope {
    #[serde(rename = "EXACT_SLICE")]
    ExactSlice,
    #[serde(rename = "WINDOW_ADJUSTED")]
    WindowAdjusted,
    #[serde(rename = "OTHER")]
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageStatus {
    Current,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferenceCoverageRow {
    pub product_key: String,
    pub product_label: String,
    pub delivery_point_key: String,
    pub delivery_point_label: String,
    pub availability_window: String,
    pub availability_window_label: String,
    #[serde(serialize_with = "wire::decimal_option")]
    pub benchmark_price_usd_per_mt: Option<Decimal>,
    pub source_label: ReferenceSourceLabel,
    pub generated_at: DateTime<Utc>,
    pub observed_at: Option<DateTime<Utc>>,
    pub source_kind: ReferenceSourceKind,
    pub scope: ReferenceScope,
    pub coverage_status: CoverageStatus,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferenceCoverageSection {
    pub rows: Vec<ReferenceCoverageRow>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MarketplaceResponse {
    pub meta: Meta,
    pub live: Option<MarketActivitySection>,
    pub demo: Option<MarketActivitySection>,
    pub unknown: Option<MarketActivitySection>,
    pub reference: Option<ReferenceCoverageSection>,
    pub commercial: Option<super::commercial::CommercialSummary>,
}

// ---------------------------------------------------------------------------
// Provider view (consumed from Task 16; typed availability, never raw JSON)
// ---------------------------------------------------------------------------

/// One registered analytics event. Closed: no free-text event name reaches a
/// query or an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Event {
    RegistrationCompleted,
    OrganizationCreated,
    OrderCreated,
    TradeCreated,
    LandingCtaClicked,
    EnergyCalculatorStarted,
    EnergyCalculatorCompleted,
    PublicLanguageChanged,
    SignupStarted,
    SignupRoleSelected,
    SignupSubmitted,
    SignupOrganizationRequired,
    SignupOrganizationSubmitted,
    LoginSubmitted,
    LoginSucceeded,
    LoginFailed,
    PlatformNavigation,
    MarketSliceSelected,
    ListingOpened,
    OrderFormOpened,
    OrderFormSubmitted,
    TradeConfirmationOpened,
    TutorialStarted,
    TutorialStepCompleted,
    TutorialStepSkipped,
    TutorialCompleted,
    EstimatorOpened,
    EstimatorCompleted,
    FrontendError,
    BackendUnavailable,
    NavigationPerformance,
}

impl Event {
    pub const ALL: [Event; 31] = [
        Event::RegistrationCompleted,
        Event::OrganizationCreated,
        Event::OrderCreated,
        Event::TradeCreated,
        Event::LandingCtaClicked,
        Event::EnergyCalculatorStarted,
        Event::EnergyCalculatorCompleted,
        Event::PublicLanguageChanged,
        Event::SignupStarted,
        Event::SignupRoleSelected,
        Event::SignupSubmitted,
        Event::SignupOrganizationRequired,
        Event::SignupOrganizationSubmitted,
        Event::LoginSubmitted,
        Event::LoginSucceeded,
        Event::LoginFailed,
        Event::PlatformNavigation,
        Event::MarketSliceSelected,
        Event::ListingOpened,
        Event::OrderFormOpened,
        Event::OrderFormSubmitted,
        Event::TradeConfirmationOpened,
        Event::TutorialStarted,
        Event::TutorialStepCompleted,
        Event::TutorialStepSkipped,
        Event::TutorialCompleted,
        Event::EstimatorOpened,
        Event::EstimatorCompleted,
        Event::FrontendError,
        Event::BackendUnavailable,
        Event::NavigationPerformance,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Event::RegistrationCompleted => "registration_completed",
            Event::OrganizationCreated => "organization_created",
            Event::OrderCreated => "order_created",
            Event::TradeCreated => "trade_created",
            Event::LandingCtaClicked => "landing_cta_clicked",
            Event::EnergyCalculatorStarted => "energy_calculator_started",
            Event::EnergyCalculatorCompleted => "energy_calculator_completed",
            Event::PublicLanguageChanged => "public_language_changed",
            Event::SignupStarted => "signup_started",
            Event::SignupRoleSelected => "signup_role_selected",
            Event::SignupSubmitted => "signup_submitted",
            Event::SignupOrganizationRequired => "signup_organization_required",
            Event::SignupOrganizationSubmitted => "signup_organization_submitted",
            Event::LoginSubmitted => "login_submitted",
            Event::LoginSucceeded => "login_succeeded",
            Event::LoginFailed => "login_failed",
            Event::PlatformNavigation => "platform_navigation",
            Event::MarketSliceSelected => "market_slice_selected",
            Event::ListingOpened => "listing_opened",
            Event::OrderFormOpened => "order_form_opened",
            Event::OrderFormSubmitted => "order_form_submitted",
            Event::TradeConfirmationOpened => "trade_confirmation_opened",
            Event::TutorialStarted => "tutorial_started",
            Event::TutorialStepCompleted => "tutorial_step_completed",
            Event::TutorialStepSkipped => "tutorial_step_skipped",
            Event::TutorialCompleted => "tutorial_completed",
            Event::EstimatorOpened => "estimator_opened",
            Event::EstimatorCompleted => "estimator_completed",
            Event::FrontendError => "frontend_error",
            Event::BackendUnavailable => "backend_unavailable",
            Event::NavigationPerformance => "navigation_performance",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == value)
    }
}

/// One registered event property. The event decides which of these are legal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Property {
    Role,
    Side,
    CanonicalProduct,
    DeliveryPoint,
    AvailabilityWindow,
    Cta,
    Placement,
    Language,
    Fuel,
    Port,
    From,
    To,
    EntryPoint,
    OrganizationPath,
    OrganizationType,
    Country,
    ReasonCategory,
    Destination,
    ViewMode,
    Product,
    Window,
    DemoStatus,
    Step,
    RouteFamily,
    Category,
    LatencyBucket,
}

impl Property {
    pub fn as_str(self) -> &'static str {
        match self {
            Property::Role => "role",
            Property::Side => "side",
            Property::CanonicalProduct => "canonical_product",
            Property::DeliveryPoint => "delivery_point",
            Property::AvailabilityWindow => "availability_window",
            Property::Cta => "cta",
            Property::Placement => "placement",
            Property::Language => "language",
            Property::Fuel => "fuel",
            Property::Port => "port",
            Property::From => "from",
            Property::To => "to",
            Property::EntryPoint => "entry_point",
            Property::OrganizationPath => "organization_path",
            Property::OrganizationType => "organization_type",
            Property::Country => "country",
            Property::ReasonCategory => "reason_category",
            Property::Destination => "destination",
            Property::ViewMode => "view_mode",
            Property::Product => "product",
            Property::Window => "window",
            Property::DemoStatus => "demo_status",
            Property::Step => "step",
            Property::RouteFamily => "route_family",
            Property::Category => "category",
            Property::LatencyBucket => "latency_bucket",
        }
    }
}

/// The event→property registry of CF4_03 `EventPropertyDomains`.
pub fn event_properties(event: Event) -> &'static [Property] {
    use Property::*;
    match event {
        Event::RegistrationCompleted | Event::OrganizationCreated => &[Role],
        Event::OrderCreated | Event::TradeCreated => &[
            Role,
            Side,
            CanonicalProduct,
            DeliveryPoint,
            AvailabilityWindow,
        ],
        Event::LandingCtaClicked => &[Cta, Placement, Language],
        Event::EnergyCalculatorStarted => &[Language],
        Event::EnergyCalculatorCompleted => &[Language, Fuel, Port],
        Event::PublicLanguageChanged => &[From, To],
        Event::SignupStarted => &[EntryPoint, Language],
        Event::SignupRoleSelected => &[Role],
        Event::SignupSubmitted => &[Role, OrganizationPath],
        Event::SignupOrganizationRequired => &[Role],
        Event::SignupOrganizationSubmitted => &[Role, OrganizationType, Country],
        Event::LoginSubmitted => &[],
        Event::LoginSucceeded => &[Role],
        Event::LoginFailed => &[ReasonCategory],
        Event::PlatformNavigation => &[Destination, ViewMode],
        Event::MarketSliceSelected => &[Product, DeliveryPoint, Window],
        Event::ListingOpened => &[Product, DeliveryPoint, Window, Side, DemoStatus],
        Event::OrderFormOpened | Event::OrderFormSubmitted => {
            &[Product, DeliveryPoint, Window, Side]
        }
        Event::TradeConfirmationOpened => &[Side, DemoStatus],
        Event::TutorialStarted => &[Role],
        Event::TutorialStepCompleted | Event::TutorialStepSkipped => &[Step, Role],
        Event::TutorialCompleted => &[Role],
        Event::EstimatorOpened => &[],
        Event::EstimatorCompleted => &[Port, Fuel],
        Event::FrontendError => &[RouteFamily, Category],
        Event::BackendUnavailable => &[RouteFamily],
        Event::NavigationPerformance => &[Destination, ViewMode, LatencyBucket],
    }
}

/// Sorted, deduplicated, validated `(event, property)` request set.
///
/// A property that the event does not register is refused with
/// `ANALYTICS_PROPERTY_INVALID`; at most twelve distinct events may be asked
/// for in one window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AllowedPropertySet {
    pairs: Vec<(Event, Property)>,
}

/// CF4_03 bound: at most twelve distinct events per window request.
pub const MAX_PROPERTY_EVENTS: usize = 12;

impl AllowedPropertySet {
    pub fn new(
        requests: impl IntoIterator<Item = (Event, Property)>,
    ) -> Result<Self, ApiError> {
        let mut pairs: Vec<(Event, Property)> = requests.into_iter().collect();
        pairs.sort();
        pairs.dedup();
        for (event, property) in &pairs {
            if !event_properties(*event).contains(property) {
                return Err(invalid_property(
                    "the requested property is not registered for that event",
                ));
            }
        }
        let mut events: Vec<Event> = pairs.iter().map(|(event, _)| *event).collect();
        events.dedup();
        if events.len() > MAX_PROPERTY_EVENTS {
            return Err(invalid_property(
                "at most twelve event breakdowns are allowed per request",
            ));
        }
        Ok(Self { pairs })
    }

    pub fn pairs(&self) -> &[(Event, Property)] {
        &self.pairs
    }

    pub fn events(&self) -> Vec<Event> {
        let mut events: Vec<Event> = self.pairs.iter().map(|(event, _)| *event).collect();
        events.dedup();
        events
    }

    /// No breakdown requested. Distinct from an empty request that was
    /// answered: this set was never sent anywhere.
    pub fn not_requested() -> Self {
        Self::default()
    }

    pub fn contains(&self, event: Event, property: Property) -> bool {
        self.pairs.binary_search(&(event, property)).is_ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentStatus {
    Available,
    Partial,
    Unavailable,
    NotRequested,
}

/// One typed provider component. Unavailable and not-requested components carry
/// no data at all: never a zero and never an empty collection.
#[derive(Clone, Debug)]
pub struct ProviderComponent<T> {
    pub status: ComponentStatus,
    pub diagnostic: Option<Diagnostic>,
    pub data: Option<T>,
}

impl<T> ProviderComponent<T> {
    pub fn available(data: T) -> Self {
        Self {
            status: ComponentStatus::Available,
            diagnostic: None,
            data: Some(data),
        }
    }

    pub fn partial(diagnostic: Option<Diagnostic>, data: Option<T>) -> Self {
        Self {
            status: ComponentStatus::Partial,
            diagnostic,
            data,
        }
    }

    pub fn unavailable(diagnostic: Diagnostic) -> Self {
        Self {
            status: ComponentStatus::Unavailable,
            diagnostic: Some(diagnostic),
            data: None,
        }
    }

    pub fn not_requested() -> Self {
        Self {
            status: ComponentStatus::NotRequested,
            diagnostic: None,
            data: None,
        }
    }

    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    pub fn status(&self) -> ComponentStatus {
        self.status
    }
}

#[derive(Clone, Debug, Default)]
pub struct CoreMetrics {
    pub visitors: Option<u64>,
    pub visits: Option<u64>,
    pub pageviews: Option<u64>,
    pub total_time_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryPage {
    Landing,
    Signup,
    Login,
    Pilot,
    Calculator,
    Platform,
    Admin,
    Other,
}

impl EntryPage {
    pub const ALL: [EntryPage; 8] = [
        EntryPage::Landing,
        EntryPage::Signup,
        EntryPage::Login,
        EntryPage::Pilot,
        EntryPage::Calculator,
        EntryPage::Platform,
        EntryPage::Admin,
        EntryPage::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EntryPage::Landing => "landing",
            EntryPage::Signup => "signup",
            EntryPage::Login => "login",
            EntryPage::Pilot => "pilot",
            EntryPage::Calculator => "calculator",
            EntryPage::Platform => "platform",
            EntryPage::Admin => "admin",
            EntryPage::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReferrerCategory {
    Direct,
    Search,
    Social,
    Referral,
    Other,
}

impl ReferrerCategory {
    pub const ALL: [ReferrerCategory; 5] = [
        ReferrerCategory::Direct,
        ReferrerCategory::Search,
        ReferrerCategory::Social,
        ReferrerCategory::Referral,
        ReferrerCategory::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ReferrerCategory::Direct => "direct",
            ReferrerCategory::Search => "search",
            ReferrerCategory::Social => "social",
            ReferrerCategory::Referral => "referral",
            ReferrerCategory::Other => "other",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DailyVisitor {
    pub date: NaiveDate,
    pub visitors: u64,
}

#[derive(Clone, Debug)]
pub struct DailyEvent {
    pub date: NaiveDate,
    pub event: Event,
    pub value: u64,
}

#[derive(Clone, Debug)]
pub struct EntryRanking {
    pub entry: EntryPage,
    pub count: u64,
}

#[derive(Clone, Debug)]
pub struct ReferrerRanking {
    pub referrer: ReferrerCategory,
    pub count: u64,
}

#[derive(Clone, Debug)]
pub struct PropertyValueCount {
    pub value: String,
    pub total: u64,
}

#[derive(Clone, Debug)]
pub struct PropertyResult {
    pub event: Event,
    pub property: Property,
    pub result: ProviderComponent<Vec<PropertyValueCount>>,
}

/// The typed view of one behavioral window that these tabs consume. The
/// provider adapter (Task 16) produces it; nothing here is raw upstream JSON.
#[derive(Clone, Debug)]
pub struct BehavioralWindow {
    pub observed_at: DateTime<Utc>,
    pub core: ProviderComponent<CoreMetrics>,
    pub event_totals: ProviderComponent<Vec<(Event, u64)>>,
    pub daily_visitors: ProviderComponent<Vec<DailyVisitor>>,
    pub event_series: ProviderComponent<Vec<DailyEvent>>,
    pub top_entries: ProviderComponent<Vec<EntryRanking>>,
    pub top_referrers: ProviderComponent<Vec<ReferrerRanking>>,
    pub property_results: Vec<PropertyResult>,
}

impl BehavioralWindow {
    pub fn unavailable(diagnostic: Diagnostic, observed_at: DateTime<Utc>) -> Self {
        Self {
            observed_at,
            core: ProviderComponent::unavailable(diagnostic),
            event_totals: ProviderComponent::unavailable(diagnostic),
            daily_visitors: ProviderComponent::unavailable(diagnostic),
            event_series: ProviderComponent::unavailable(diagnostic),
            top_entries: ProviderComponent::unavailable(diagnostic),
            top_referrers: ProviderComponent::unavailable(diagnostic),
            property_results: Vec::new(),
        }
    }

    /// The published source status, from the core component.
    pub fn status(&self) -> SourceStatus {
        match self.core.status {
            ComponentStatus::Available => SourceStatus::Available,
            ComponentStatus::Partial => SourceStatus::Partial,
            _ => SourceStatus::Unavailable,
        }
    }

    pub fn diagnostic(&self) -> Option<Diagnostic> {
        self.core.diagnostic.or_else(|| {
            self.event_totals
                .diagnostic
                .or(self.daily_visitors.diagnostic)
                .or(self.event_series.diagnostic)
        })
    }

    pub fn property(&self, event: Event, property: Property) -> Option<&PropertyResult> {
        self.property_results
            .iter()
            .find(|result| result.event == event && result.property == property)
    }

    pub fn event_total(&self, event: Event) -> Option<i64> {
        self.event_totals
            .data()
            .map(|totals| {
                totals
                    .iter()
                    .filter(|(name, _)| *name == event)
                    .map(|(_, count)| *count as i64)
                    .sum()
            })
    }

    pub fn daily_value(&self, date: NaiveDate) -> Option<i64> {
        self.daily_visitors.data().map(|rows| {
            rows.iter()
                .filter(|row| row.date == date)
                .map(|row| row.visitors as i64)
                .sum()
        })
    }

    pub fn event_day(&self, event: Event) -> Option<BTreeMap<NaiveDate, i64>> {
        self.event_series.data().map(|rows| {
            let mut values: BTreeMap<NaiveDate, i64> = BTreeMap::new();
            for row in rows.iter().filter(|row| row.event == event) {
                *values.entry(row.date).or_insert(0) += row.value as i64;
            }
            values
        })
    }
}

/// The behavioral seam consumed by P06 and P11. Implementations return typed
/// availability; they never raise a raw upstream body and never fabricate zero.
pub type ProviderFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

pub trait BehavioralRead: Send + Sync + 'static {
    fn window<'a>(
        &'a self,
        query: &'a AnalyticsQuery,
        properties: &'a AllowedPropertySet,
    ) -> ProviderFuture<'a, BehavioralWindow>;
}

impl BehavioralRead for std::sync::Arc<dyn BehavioralRead> {
    fn window<'a>(
        &'a self,
        query: &'a AnalyticsQuery,
        properties: &'a AllowedPropertySet,
    ) -> ProviderFuture<'a, BehavioralWindow> {
        self.as_ref().window(query, properties)
    }
}

impl std::fmt::Debug for dyn BehavioralRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BehavioralRead")
    }
}

/// The canonical reference seam. Phase 5's reference read owns the benchmark
/// resolution; this lane supplies the catalog enumeration it must answer for.
pub struct ReferenceRequest {
    pub product_code: String,
    pub delivery_point_id: Uuid,
    pub window: String,
    pub delivery_point_name: String,
}

#[derive(Clone, Debug)]
pub struct ReferenceQuote {
    pub product_code: String,
    pub delivery_point_id: Uuid,
    pub window: String,
    pub benchmark_price_usd_per_mt: Decimal,
    pub source: String,
    pub generated_at: DateTime<Utc>,
    pub observed_at: Option<DateTime<Utc>>,
}

pub type ReferenceFuture<'a> = ProviderFuture<'a, Result<Vec<ReferenceQuote>, ApiError>>;

pub trait ReferenceRead: Send + Sync + 'static {
    fn quotes<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        requests: &'a [ReferenceRequest],
    ) -> ReferenceFuture<'a>;
}

// ---------------------------------------------------------------------------
// Market rows and SQL
// ---------------------------------------------------------------------------

const DEMO_MARKET_ORG_IDS: [Uuid; 14] = [
    Uuid::from_u128(0x4da7b285_34ee_5443_9406_f96b4ed1a251),
    Uuid::from_u128(0x0dbce576_2026_5925_ab66_674d505e98ad),
    Uuid::from_u128(0x3cd0bc8a_0e92_5eb8_9ae3_3c87a7460a6a),
    Uuid::from_u128(0x277491df_cb0d_5f2d_a2cf_5746829c6da6),
    Uuid::from_u128(0x3b302066_d65c_5c3e_8fcc_70b3da3bcafd),
    Uuid::from_u128(0x79609f48_0a3e_560e_a1e1_63d90601d84a),
    Uuid::from_u128(0x2c4e387e_de22_5adb_ad88_9274ba84ebe1),
    Uuid::from_u128(0x612953c7_567a_58b3_bc42_ee817d2bbe74),
    Uuid::from_u128(0x93ccda09_54b3_53ee_afc0_759d3048161f),
    Uuid::from_u128(0x82426590_0963_5486_9b05_f81e97afe6ef),
    Uuid::from_u128(0xacc3f20a_fe94_4463_9029_a55e35634eb7),
    Uuid::from_u128(0xc9c1ccbf_66fe_4a1b_b171_fe4f7ddc31a4),
    Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111),
    Uuid::from_u128(0xd1e43e55_3fb0_4b5e_9f0b_93aa10050222),
];

/// The seeded demo organizations are fixed application identities, never a
/// database lookup and never inferred from a name.
pub fn demo_market_org_ids() -> &'static [Uuid] {
    &DEMO_MARKET_ORG_IDS
}

/// The three provenance labels a marketplace activity request can produce.
/// They are separate sections; no response and no aggregate mixes them.
pub fn ordered_provenance_labels() -> [&'static str; 3] {
    ["live", "demo", "unknown"]
}

/// Bind order shared by the marketplace statements:
/// `$1 start, $2 end, $3 previous_start, $4 previous_end, $5 demo orgs,
/// $6 product, $7 delivery point, $8 availability window`.
const ORDER_SLICE: &str = "o.created_at >= COALESCE($3::timestamptz, $1::timestamptz)\n      AND o.created_at < $2::timestamptz\n      AND ($6::uuid IS NULL OR o.product_id = $6)\n      AND ($7::uuid IS NULL OR o.delivery_point_id = $7)\n      AND ($8::text IS NULL OR o.availability_window = $8)";

const TRADE_SLICE: &str = "(COALESCE($6::uuid IS NULL AND $7::uuid IS NULL AND $8::text IS NULL, false)\n         OR EXISTS (\n              SELECT 1 FROM orderbook_orders f\n              WHERE f.id IN (t.bid_order_id, t.ask_order_id)\n                AND ($6::uuid IS NULL OR f.product_id = $6)\n                AND ($7::uuid IS NULL OR f.delivery_point_id = $7)\n                AND ($8::text IS NULL OR f.availability_window = $8)\n         ))";

const RECOGNIZED_ORGS: &str = "o.organization_id IN (SELECT u.organization_id FROM users u WHERE u.organization_id IS NOT NULL AND u.role IN ('BUYER','SUPPLIER') AND u.status = 'APPROVED' AND NOT (u.organization_id = ANY($5::uuid[])))";

const ORDER_PROVENANCE: &str = "CASE WHEN o.provenance = 'DEMO' THEN 'demo' WHEN o.organization_id = ANY($5::uuid[]) THEN 'demo' WHEN o.provenance = 'REAL' THEN 'live' ELSE 'unknown' END";

const TRADE_PROVENANCE: &str = "CASE WHEN t.buyer_provenance = 'DEMO' OR t.seller_provenance = 'DEMO' OR t.buyer_id = ANY($5::uuid[]) OR t.seller_id = ANY($5::uuid[]) THEN 'demo' WHEN t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL' THEN 'live' ELSE 'unknown' END";

#[derive(Clone, Debug)]
pub struct OrderAggregate {
    pub provenance: String,
    pub current: i64,
    pub previous: i64,
    pub bids_current: i64,
    pub asks_current: i64,
    pub orgs_current: i64,
    pub orgs_previous: i64,
    pub executed_current: i64,
    pub still_open_current: i64,
}

#[derive(Clone, Debug)]
pub struct TradeAggregate {
    pub provenance: String,
    pub confirmed_current_strict: i64,
    pub confirmed_previous_strict: i64,
    pub legacy_current: i64,
    pub legacy_previous: i64,
    pub volume_current_strict: Decimal,
    pub volume_current_legacy: Decimal,
    pub volume_previous: Decimal,
    pub gmv_current: Decimal,
    pub gmv_previous: Decimal,
    pub missing_paid_at_current: i64,
}

#[derive(Clone, Debug, Default)]
pub struct CommissionAggregate {
    pub revenue_current: Decimal,
    pub revenue_previous: Decimal,
    pub missing_payment_date: i64,
    pub pending_total: Decimal,
    pub invoiced_total: Decimal,
}

#[derive(Clone, Debug)]
pub struct StatusRow {
    pub kind: String,
    pub provenance: String,
    pub status: String,
    pub count: i64,
    pub organizations: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct MatrixRow {
    pub kind: String,
    pub product_name: Option<String>,
    pub fuel_type: Option<String>,
    pub fuel_grade: Option<String>,
    pub delivery_point_name: Option<String>,
    pub window: Option<String>,
    pub orders: i64,
    pub organizations: i64,
}

#[derive(Clone, Debug)]
pub struct TrendRow {
    pub day: NaiveDate,
    pub side: String,
    pub orders: i64,
    pub organizations: i64,
}

#[derive(Clone, Debug)]
pub struct QuoteRow {
    pub provenance: String,
    pub organization_id: Uuid,
    pub side: String,
    pub price: Decimal,
    pub remaining: Decimal,
    pub window: String,
    pub created_at: DateTime<Utc>,
    pub product_name: String,
    pub fuel_type: String,
    pub fuel_grade: String,
    pub delivery_point_name: String,
}

#[derive(Clone, Debug)]
pub struct FillRow {
    pub created_at: DateTime<Utc>,
    pub first_confirmed_at: DateTime<Utc>,
}

type Slice = sqlx::query::Query<'static, sqlx::Postgres, sqlx::postgres::PgArguments>;

/// Bind the shared slice parameters. The statement text is assembled only from
/// the constant fragments above — never from caller input — so the assertion is
/// over reviewed text, and every value below is a bound parameter.
fn bind_slice(text: String, query_ref: &AnalyticsQuery) -> Slice {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(text));
    let (previous_start, previous_end) = match query_ref.previous() {
        Some((start, end)) => (Some(start), Some(end)),
        None => (None, None),
    };
    let filters = query_ref.filters();
    query = query
        .bind(query_ref.start)
        .bind(query_ref.end)
        .bind(previous_start)
        .bind(previous_end)
        .bind(DEMO_MARKET_ORG_IDS.to_vec())
        .bind(filters.product_id)
        .bind(filters.delivery_point_id)
        .bind(filters.availability_window);
    query
}

pub async fn aggregate_orders(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<OrderAggregate>, ApiError> {
    let text = format!(
        "WITH scoped AS (\n    SELECT {ORDER_PROVENANCE} AS provenance, o.id, o.organization_id, o.side, o.status, o.remaining_quantity_mt, o.expires_at, o.created_at\n    FROM orderbook_orders o\n    WHERE {ORDER_SLICE}\n)\nSELECT provenance,\n    count(*) FILTER (WHERE created_at >= $1 AND created_at < $2) AS orders_current,\n    count(*) FILTER (WHERE $3::timestamptz IS NOT NULL AND created_at >= $3 AND created_at < $4) AS orders_previous,\n    count(*) FILTER (WHERE created_at >= $1 AND created_at < $2 AND side = 'BID') AS bids_current,\n    count(*) FILTER (WHERE created_at >= $1 AND created_at < $2 AND side = 'ASK') AS asks_current,\n    count(DISTINCT organization_id) FILTER (WHERE created_at >= $1 AND created_at < $2) AS orgs_current,\n    count(DISTINCT organization_id) FILTER (WHERE $3::timestamptz IS NOT NULL AND created_at >= $3 AND created_at < $4) AS orgs_previous,\n    count(*) FILTER (WHERE created_at >= $1 AND created_at < $2 AND EXISTS (SELECT 1 FROM trades t WHERE (t.bid_order_id = scoped.id OR t.ask_order_id = scoped.id) AND t.status IN ('CONFIRMED','DELIVERED','PAID') AND t.confirmed_at IS NOT NULL AND t.confirmed_at < $2)) AS executed_current,\n    count(*) FILTER (WHERE created_at >= $1 AND created_at < $2 AND status IN ('OPEN','PARTIALLY_FILLED') AND remaining_quantity_mt > 0 AND (expires_at IS NULL OR expires_at >= $2)) AS still_open_current\nFROM scoped\nGROUP BY provenance"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| OrderAggregate {
            provenance: row.get("provenance"),
            current: row.get("orders_current"),
            previous: row.get("orders_previous"),
            bids_current: row.get("bids_current"),
            asks_current: row.get("asks_current"),
            orgs_current: row.get("orgs_current"),
            orgs_previous: row.get("orgs_previous"),
            executed_current: row.get("executed_current"),
            still_open_current: row.get("still_open_current"),
        })
        .collect())
}

pub async fn aggregate_trades(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<TradeAggregate>, ApiError> {
    let text = format!(
        "WITH scoped AS (\n    SELECT {TRADE_PROVENANCE} AS provenance, t.status, t.confirmed_at, t.created_at, t.paid_at,\n        COALESCE(t.confirmed_at, t.created_at) AS economic_ts,\n        COALESCE(t.final_quantity_mt, t.quantity_mt) AS volume,\n        t.final_total_usd\n    FROM trades t\n    WHERE t.status IN ('CONFIRMED','DELIVERED','PAID')\n      AND ((COALESCE(t.confirmed_at, t.created_at) >= COALESCE($3::timestamptz, $1::timestamptz) AND COALESCE(t.confirmed_at, t.created_at) < $2::timestamptz)\n        OR (t.status = 'PAID' AND t.paid_at >= COALESCE($3::timestamptz, $1::timestamptz) AND t.paid_at < $2::timestamptz))\n      AND {TRADE_SLICE}\n)\nSELECT provenance,\n    count(*) FILTER (WHERE confirmed_at IS NOT NULL AND confirmed_at >= $1 AND confirmed_at < $2) AS confirmed_current_strict,\n    count(*) FILTER (WHERE confirmed_at IS NOT NULL AND confirmed_at >= $3 AND confirmed_at < $4) AS confirmed_previous_strict,\n    count(*) FILTER (WHERE confirmed_at IS NULL AND created_at >= $1 AND created_at < $2) AS legacy_current,\n    count(*) FILTER (WHERE confirmed_at IS NULL AND created_at >= $3 AND created_at < $4) AS legacy_previous,\n    COALESCE(sum(volume) FILTER (WHERE confirmed_at IS NOT NULL AND confirmed_at >= $1 AND confirmed_at < $2), 0) AS volume_current_strict,\n    COALESCE(sum(volume) FILTER (WHERE confirmed_at IS NULL AND created_at >= $1 AND created_at < $2), 0) AS volume_current_legacy,\n    COALESCE(sum(volume) FILTER (WHERE (confirmed_at IS NOT NULL AND confirmed_at >= $3 AND confirmed_at < $4) OR (confirmed_at IS NULL AND created_at >= $3 AND created_at < $4)), 0) AS volume_previous,\n    COALESCE(sum(final_total_usd) FILTER (WHERE status = 'PAID' AND paid_at >= $1 AND paid_at < $2), 0) AS gmv_current,\n    COALESCE(sum(final_total_usd) FILTER (WHERE status = 'PAID' AND paid_at >= $3 AND paid_at < $4), 0) AS gmv_previous,\n    count(*) FILTER (WHERE status = 'PAID' AND paid_at IS NULL AND economic_ts >= $1 AND economic_ts < $2) AS missing_paid_at_current\nFROM scoped\nGROUP BY provenance"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| TradeAggregate {
            provenance: row.get("provenance"),
            confirmed_current_strict: row.get("confirmed_current_strict"),
            confirmed_previous_strict: row.get("confirmed_previous_strict"),
            legacy_current: row.get("legacy_current"),
            legacy_previous: row.get("legacy_previous"),
            volume_current_strict: row.get("volume_current_strict"),
            volume_current_legacy: row.get("volume_current_legacy"),
            volume_previous: row.get("volume_previous"),
            gmv_current: row.get("gmv_current"),
            gmv_previous: row.get("gmv_previous"),
            missing_paid_at_current: row.get("missing_paid_at_current"),
        })
        .collect())
}

pub async fn aggregate_commissions(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<CommissionAggregate, ApiError> {
    let filters = query_ref.filters();
    let current = date_window(Some(query_ref.start), Some(query_ref.end));
    let previous = match query_ref.previous() {
        Some((start, end)) => date_window(Some(start), Some(end)),
        None => None,
    };
    let (current_start, current_end) = current.unwrap_or((query_ref.start.date_naive(), query_ref.start.date_naive()));
    let row = sqlx::query(
        "SELECT\n    COALESCE(sum(c.amount_usd) FILTER (WHERE c.status = 'PAID' AND c.payment_date IS NOT NULL AND c.payment_date >= $6 AND c.payment_date < $7), 0) AS revenue_current,\n    COALESCE(sum(c.amount_usd) FILTER (WHERE c.status = 'PAID' AND c.payment_date IS NOT NULL AND $8::date IS NOT NULL AND c.payment_date >= $8 AND c.payment_date < $9), 0) AS revenue_previous,\n    count(*) FILTER (WHERE c.status = 'PAID' AND c.payment_date IS NULL) AS missing_payment_date,\n    COALESCE(sum(c.amount_usd) FILTER (WHERE c.status = 'PENDING'), 0) AS pending_total,\n    COALESCE(sum(c.amount_usd) FILTER (WHERE c.status = 'INVOICED'), 0) AS invoiced_total\nFROM commissions c\nLEFT JOIN trades t ON t.id = c.trade_id\nWHERE NOT (t.id IS NOT NULL AND (t.buyer_id = ANY($1::uuid[]) OR t.seller_id = ANY($1::uuid[])))\n  AND (NOT $2::boolean OR (t.id IS NOT NULL AND EXISTS (\n        SELECT 1 FROM orderbook_orders f\n        WHERE f.id IN (t.bid_order_id, t.ask_order_id)\n          AND ($3::uuid IS NULL OR f.product_id = $3)\n          AND ($4::uuid IS NULL OR f.delivery_point_id = $4)\n          AND ($5::text IS NULL OR f.availability_window = $5)\n      )))",
    )
    .bind(DEMO_MARKET_ORG_IDS.to_vec())
    .bind(filters.active())
    .bind(filters.product_id)
    .bind(filters.delivery_point_id)
    .bind(filters.availability_window)
    .bind(current_start)
    .bind(current_end)
    .bind(previous.map(|(lower, _)| lower))
    .bind(previous.map(|(_, upper)| upper))
    .fetch_one(&mut *conn)
    .await
    .map_err(database)?;
    Ok(CommissionAggregate {
        revenue_current: row.get("revenue_current"),
        revenue_previous: row.get("revenue_previous"),
        missing_payment_date: row.get("missing_payment_date"),
        pending_total: row.get("pending_total"),
        invoiced_total: row.get("invoiced_total"),
    })
}

pub async fn status_distribution(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<StatusRow>, ApiError> {
    let text = format!(
        "WITH order_rows AS (\n    SELECT 'order'::text AS kind, {ORDER_PROVENANCE} AS provenance, o.status::text AS status,\n        count(*) AS count, count(DISTINCT o.organization_id) AS organizations\n    FROM orderbook_orders o\n    WHERE {ORDER_SLICE}\n    GROUP BY 2, o.status\n), trade_rows AS (\n    SELECT 'trade'::text AS kind, {TRADE_PROVENANCE} AS provenance, t.status::text AS status,\n        count(*) AS count, NULL::bigint AS organizations\n    FROM trades t\n    WHERE COALESCE(t.confirmed_at, t.created_at) >= $1 AND COALESCE(t.confirmed_at, t.created_at) < $2\n      AND {TRADE_SLICE}\n    GROUP BY 2, t.status\n)\nSELECT * FROM order_rows UNION ALL SELECT * FROM trade_rows"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| StatusRow {
            kind: row.get("kind"),
            provenance: row.get("provenance"),
            status: row.get("status"),
            count: row.get("count"),
            organizations: row.get("organizations"),
        })
        .collect())
}

pub async fn matrix_and_windows(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<MatrixRow>, ApiError> {
    let text = format!(
        "SELECT 'matrix'::text AS kind, p.name AS product_name, p.fuel_type, p.fuel_grade, d.name AS delivery_point_name,\n    NULL::text AS window, count(*) AS orders, count(DISTINCT o.organization_id) AS organizations\nFROM orderbook_orders o\nJOIN products p ON p.id = o.product_id\nJOIN delivery_points d ON d.id = o.delivery_point_id\nWHERE {ORDER_SLICE} AND {RECOGNIZED_ORGS}\nGROUP BY p.name, p.fuel_type, p.fuel_grade, d.name\nUNION ALL\nSELECT 'window'::text AS kind, NULL::text, NULL::text, NULL::text, NULL::text,\n    o.availability_window, count(*) AS orders, count(DISTINCT o.organization_id) AS organizations\nFROM orderbook_orders o\nWHERE {ORDER_SLICE} AND {RECOGNIZED_ORGS}\nGROUP BY o.availability_window"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| MatrixRow {
            kind: row.get("kind"),
            product_name: row.get("product_name"),
            fuel_type: row.get("fuel_type"),
            fuel_grade: row.get("fuel_grade"),
            delivery_point_name: row.get("delivery_point_name"),
            window: row.get("window"),
            orders: row.get("orders"),
            organizations: row.get("organizations"),
        })
        .collect())
}

pub async fn balance_trend(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<TrendRow>, ApiError> {
    let text = format!(
        "SELECT (timezone('UTC', o.created_at))::date AS day, o.side::text AS side,\n    count(*) AS orders, count(DISTINCT o.organization_id) AS organizations\nFROM orderbook_orders o\nWHERE {ORDER_SLICE} AND {RECOGNIZED_ORGS}\nGROUP BY day, o.side"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| TrendRow {
            day: row.get("day"),
            side: row.get("side"),
            orders: row.get("orders"),
            organizations: row.get("organizations"),
        })
        .collect())
}

pub async fn eligible_quotes(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<QuoteRow>, ApiError> {
    let text = format!(
        "SELECT {ORDER_PROVENANCE} AS provenance, o.organization_id, o.side::text AS side,\n    o.price_per_mt_usd AS price, o.remaining_quantity_mt AS remaining,\n    o.availability_window AS window, o.created_at,\n    p.name AS product_name, p.fuel_type, p.fuel_grade, d.name AS delivery_point_name\nFROM orderbook_orders o\nJOIN products p ON p.id = o.product_id\nJOIN delivery_points d ON d.id = o.delivery_point_id\nWHERE o.status IN ('OPEN','PARTIALLY_FILLED')\n  AND o.remaining_quantity_mt > 0\n  AND o.delivery_point_id IS NOT NULL\n  AND (o.expires_at IS NULL OR o.expires_at >= $2)\n  AND o.created_at < $2\n  AND ($6::uuid IS NULL OR o.product_id = $6)\n  AND ($7::uuid IS NULL OR o.delivery_point_id = $7)\n  AND ($8::text IS NULL OR o.availability_window = $8)"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| QuoteRow {
            provenance: row.get("provenance"),
            organization_id: row.get("organization_id"),
            side: row.get("side"),
            price: row.get("price"),
            remaining: row.get("remaining"),
            window: row.get("window"),
            created_at: row.get("created_at"),
            product_name: row.get("product_name"),
            fuel_type: row.get("fuel_type"),
            fuel_grade: row.get("fuel_grade"),
            delivery_point_name: row.get("delivery_point_name"),
        })
        .collect())
}

pub async fn time_to_fill(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<FillRow>, ApiError> {
    let text = format!(
        "SELECT o.created_at, min(t.confirmed_at) AS first_confirmed_at\nFROM orderbook_orders o\nJOIN trades t ON (t.bid_order_id = o.id OR t.ask_order_id = o.id)\nWHERE {ORDER_SLICE}\n  AND {RECOGNIZED_ORGS}\n  AND t.status IN ('CONFIRMED','DELIVERED','PAID')\n  AND t.confirmed_at IS NOT NULL\n  AND t.confirmed_at < $2\nGROUP BY o.id, o.created_at"
    );
    let rows = bind_slice(text, query_ref)
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;
    rows.into_iter()
        .map(|row| {
            Ok(FillRow {
                created_at: row.get("created_at"),
                first_confirmed_at: row.get("first_confirmed_at"),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Marketplace composition
// ---------------------------------------------------------------------------

fn provenance_row<'a, T>(
    rows: &'a [T],
    provenance: &str,
    select: impl Fn(&'a T) -> &'a str,
) -> Option<&'a T> {
    rows.iter().find(|row| select(row) == provenance)
}

/// The rows one marketplace request reads. Passing them as one value keeps the
/// section builder about composition rather than about six positional slices.
struct SectionRows<'a> {
    quotes: &'a [QuoteRow],
    matrix: &'a [MatrixRow],
    trend: &'a [TrendRow],
    status: &'a [StatusRow],
    fill: &'a [FillRow],
}

fn activity_section(
    policy: &MarketPolicy,
    provenance: &str,
    query_ref: &AnalyticsQuery,
    orders: Option<&OrderAggregate>,
    trades: Option<&TradeAggregate>,
    rows: &SectionRows<'_>,
    quality: &mut Quality,
) -> MarketActivitySection {
    let confirmed_current = trades
        .map(|trades| trades.confirmed_current_strict + trades.legacy_current)
        .unwrap_or_default();
    let confirmed_previous = trades
        .map(|trades| trades.confirmed_previous_strict + trades.legacy_previous)
        .unwrap_or_default();
    let volume_current = trades
        .map(|trades| money(trades.volume_current_strict) + money(trades.volume_current_legacy))
        .unwrap_or_else(|| money(Decimal::ZERO));
    let volume_previous = trades
        .map(|trades| money(trades.volume_previous))
        .unwrap_or_else(|| money(Decimal::ZERO));

    let executed = orders.map(|orders| orders.executed_current).unwrap_or_default();
    let created = orders.map(|orders| orders.current).unwrap_or_default();
    let still_open = orders
        .map(|orders| orders.still_open_current)
        .unwrap_or_default();
    let cohort_complete = !(query_ref.end > Utc::now() && still_open > 0);
    if !cohort_complete {
        quality.cohort_complete = false;
    }

    let kpis = MarketplaceKpis {
        participating_organizations: MetricValue {
            value: orders.map(|orders| orders.orgs_current).or(Some(0)),
            previous: orders.map(|orders| orders.orgs_previous).or(Some(0)),
            suppressed: false,
        },
        open_bids: MetricValue {
            value: orders.map(|orders| orders.bids_current).or(Some(0)),
            previous: None,
            suppressed: false,
        },
        open_asks: MetricValue {
            value: orders.map(|orders| orders.asks_current).or(Some(0)),
            previous: None,
            suppressed: false,
        },
        confirmed_trades: MetricValue {
            value: Some(confirmed_current),
            previous: Some(confirmed_previous),
            suppressed: false,
        },
        confirmed_volume_mt: DecimalMetricValue {
            value: Some(volume_current),
            previous: Some(volume_previous),
            suppressed: false,
        },
        execution_rate: RatioValue {
            numerator: Some(executed),
            denominator: Some(created),
            rate_pct: rate_pct(executed, created),
            suppressed: false,
            cohort_complete,
        },
    };

    let liquidity = super::liquidity::summarize(
        policy,
        provenance,
        rows.quotes,
        rows.fill,
        query_ref.end,
        quality,
    );

    let mut matrix_cells: Vec<ProductPortCell> = Vec::new();
    let mut window_rows: Vec<RankedRow> = Vec::new();
    let mut trend = MarketBalanceTrend::default();
    if provenance == "live" {
        let (cells, windows) = matrix_and_window_rows(policy, rows.matrix, quality);
        matrix_cells = cells;
        window_rows = windows;
        trend = balance_trend_section(rows.trend, query_ref, quality);
    }

    let mut order_statuses: Vec<AggregateCell> = Vec::new();
    let mut trade_statuses: Vec<AggregateCell> = Vec::new();
    for row in rows
        .status
        .iter()
        .filter(|row| row.provenance == provenance)
    {
        let cell = cell(row.status.clone(), Some(row.count), row.organizations, quality);
        if row.kind == "order" {
            order_statuses.push(cell);
        } else {
            trade_statuses.push(cell);
        }
    }
    order_statuses.sort_by(|left, right| left.key.cmp(&right.key));
    trade_statuses.sort_by(|left, right| left.key.cmp(&right.key));

    MarketActivitySection {
        kpis,
        liquidity,
        balance_trend: trend,
        product_port_matrix: matrix_cells,
        window_distribution: window_rows,
        order_status_distribution: order_statuses,
        trade_status_distribution: trade_statuses,
        concentration: concentration(
            orders.map(|orders| orders.orgs_current).unwrap_or_default(),
            quality,
        ),
    }
}

/// HHI needs per-organization shares, and publishing them is prohibited, so
/// the band derives from the participating-organization count alone. Fewer
/// than three organizations is definitionally concentrated and suppressed.
fn concentration(organizations: i64, quality: &mut Quality) -> OrganizationConcentration {
    if organizations > 0 && organizations < SUPPRESSION_THRESHOLD {
        quality.suppressed_cell_count += 1;
        return OrganizationConcentration {
            hhi_band: None,
            suppressed: true,
        };
    }
    if organizations == 0 {
        return OrganizationConcentration {
            hhi_band: None,
            suppressed: false,
        };
    }
    let band = if organizations < 5 {
        ConcentrationBand::High
    } else if organizations < 10 {
        ConcentrationBand::Moderate
    } else {
        ConcentrationBand::Low
    };
    OrganizationConcentration {
        hhi_band: Some(band),
        suppressed: false,
    }
}

fn matrix_and_window_rows(
    policy: &MarketPolicy,
    rows: &[MatrixRow],
    quality: &mut Quality,
) -> (Vec<ProductPortCell>, Vec<RankedRow>) {
    let window_total: i64 = rows
        .iter()
        .filter(|row| row.kind == "window")
        .map(|row| row.orders)
        .sum();
    let mut cells: Vec<ProductPortCell> = Vec::new();
    let mut windows: Vec<RankedRow> = Vec::new();
    for row in rows {
        if row.kind == "matrix" {
            let name = row.product_name.clone().unwrap_or_default();
            let product_key = derive_product_code(
                policy,
                &name,
                row.fuel_type.as_deref().unwrap_or_default(),
                row.fuel_grade.as_deref().unwrap_or_default(),
            )
            .map(str::to_owned)
            .unwrap_or_else(|| slugify(&name));
            let point_name = row.delivery_point_name.clone().unwrap_or_default();
            cells.push(ProductPortCell {
                product_key,
                product_label: name,
                delivery_point_key: slugify(&point_name),
                delivery_point_label: point_name,
                orders: cell("orders", Some(row.orders), Some(row.organizations), quality),
                organizations: cell(
                    "organizations",
                    Some(row.organizations),
                    Some(row.organizations),
                    quality,
                ),
            });
        } else {
            let key = row.window.clone().unwrap_or_default();
            let (count, suppressed) =
                suppress_cell(Some(row.orders), Some(row.organizations), quality);
            let share = if suppressed {
                None
            } else {
                rate_pct(row.orders, window_total)
            };
            windows.push(RankedRow {
                label: window_label(&key),
                key,
                count,
                share_pct: share,
                suppressed,
            });
        }
    }
    cells.sort_by(|left, right| {
        left.product_key
            .cmp(&right.product_key)
            .then_with(|| left.delivery_point_key.cmp(&right.delivery_point_key))
    });
    windows.sort_by(|left, right| left.key.cmp(&right.key));
    cells.truncate(MAX_MATRIX_CELLS);
    windows.truncate(MAX_RANKED_ROWS);
    (cells, windows)
}

fn balance_trend_section(
    rows: &[TrendRow],
    query_ref: &AnalyticsQuery,
    quality: &mut Quality,
) -> MarketBalanceTrend {
    let mut buyer_orgs: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut supplier_orgs: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut bids: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut asks: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for row in rows {
        if row.side == "BID" {
            buyer_orgs.insert(row.day, row.organizations);
            bids.insert(row.day, row.orders);
        } else {
            supplier_orgs.insert(row.day, row.organizations);
            asks.insert(row.day, row.orders);
        }
    }
    MarketBalanceTrend {
        buyer_organizations: dense_series(query_ref.start, query_ref.end, &buyer_orgs, quality),
        supplier_organizations: dense_series(
            query_ref.start,
            query_ref.end,
            &supplier_orgs,
            quality,
        ),
        bids: dense_series(query_ref.start, query_ref.end, &bids, quality),
        asks: dense_series(query_ref.start, query_ref.end, &asks, quality),
    }
}

/// Canonical catalog validation for the marketplace tab. Only supplied filters
/// are validated; an unknown or inactive catalog row is a structured 422.
pub async fn validate_marketplace_filters(
    conn: &mut PgConnection,
    query_ref: &AnalyticsQuery,
) -> Result<(), ApiError> {
    if let Some(window) = &query_ref.availability_window {
        windows::normalize_window(window, Utc::now().date_naive())
            .map_err(|_| invalid_query("availability_window is not a canonical window"))?;
    }
    if let Some(product_id) = query_ref.product_id {
        let active: Option<bool> =
            sqlx::query_scalar("SELECT is_active FROM products WHERE id = $1")
                .bind(product_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(database)?;
        if active != Some(true) {
            return Err(invalid_query("product_id is not an active catalog product"));
        }
    }
    if let Some(delivery_point_id) = query_ref.delivery_point_id {
        let active: Option<bool> =
            sqlx::query_scalar("SELECT is_active FROM delivery_points WHERE id = $1")
                .bind(delivery_point_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(database)?;
        if active != Some(true) {
            return Err(invalid_query(
                "delivery_point_id is not an active catalog delivery point",
            ));
        }
    }
    Ok(())
}

struct CatalogCell {
    product_code: String,
    product_label: String,
    delivery_point_id: Uuid,
    delivery_point_name: String,
}

async fn reference_catalog(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    query_ref: &AnalyticsQuery,
) -> Result<Vec<CatalogCell>, ApiError> {
    let products = sqlx::query(
        "SELECT id, name, fuel_type, fuel_grade, is_active FROM products ORDER BY id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(database)?;
    let points = sqlx::query("SELECT id, name, region, is_active FROM delivery_points ORDER BY id")
        .fetch_all(&mut *conn)
        .await
        .map_err(database)?;

    let mut canonical_products: Vec<(String, String)> = Vec::new();
    for row in products {
        let id: Uuid = row.get("id");
        let name: String = row.get("name");
        let fuel_type: String = row.get("fuel_type");
        let fuel_grade: String = row.get("fuel_grade");
        let active: bool = row.get("is_active");
        let Some(spec) = policy.products.iter().find(|product| {
            catalog::product_matches(product, id, &name, &fuel_type, &fuel_grade, active)
        }) else {
            continue;
        };
        if query_ref.product_id.is_some_and(|wanted| wanted != id) {
            continue;
        }
        canonical_products.push((spec.code.clone(), spec.name.clone()));
    }

    let mut canonical_points: Vec<(Uuid, String)> = Vec::new();
    for row in points {
        let id: Uuid = row.get("id");
        let name: String = row.get("name");
        let region: String = row.get("region");
        let active: bool = row.get("is_active");
        let Some(spec) = policy
            .points
            .iter()
            .find(|point| catalog::point_matches(point, id, &name, &region, active))
        else {
            continue;
        };
        if query_ref
            .delivery_point_id
            .is_some_and(|wanted| wanted != id)
        {
            continue;
        }
        canonical_points.push((spec.id, spec.name.clone()));
    }

    let mut cells = Vec::new();
    for (code, product_label) in canonical_products {
        for (point_id, point_name) in &canonical_points {
            cells.push(CatalogCell {
                product_code: code.clone(),
                product_label: product_label.clone(),
                delivery_point_id: *point_id,
                delivery_point_name: point_name.clone(),
            });
        }
    }
    Ok(cells)
}

/// Compose reference coverage from the canonical benchmark seam. Reference is
/// coverage, never activity: no participants, orders, depth, spread, execution,
/// GMV or revenue appears in this section.
async fn reference_section(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    query_ref: &AnalyticsQuery,
    reference: &dyn ReferenceRead,
    now: DateTime<Utc>,
) -> Result<ReferenceCoverageSection, ApiError> {
    let cells = reference_catalog(conn, policy, query_ref).await?;
    let window = query_ref
        .availability_window
        .clone()
        .unwrap_or_else(|| windows::SPOT_WINDOW.to_owned());
    let requests: Vec<ReferenceRequest> = cells
        .iter()
        .map(|cell| ReferenceRequest {
            product_code: cell.product_code.clone(),
            delivery_point_id: cell.delivery_point_id,
            window: window.clone(),
            delivery_point_name: cell.delivery_point_name.clone(),
        })
        .collect();
    let quotes = reference.quotes(conn, &requests).await?;

    let mut keys: Vec<(String, Uuid, String)> = cells
        .iter()
        .map(|cell| {
            (
                cell.product_code.clone(),
                cell.delivery_point_id,
                window.clone(),
            )
        })
        .collect();
    keys.sort();
    keys.dedup();

    let mut rows = Vec::new();
    for key in keys {
        let Some(cell) = cells.iter().find(|cell| {
            cell.product_code == key.0 && cell.delivery_point_id == key.1
        }) else {
            continue;
        };
        let base_window = window_label(&key.2);
        let quote = quotes.iter().find(|quote| {
            quote.product_code == key.0
                && quote.delivery_point_id == key.1
                && quote.window == key.2
        });
        let base = |product_key: &str,
                    product_label: &str,
                    point_key: String,
                    point_label: &str,
                    window_key: &str,
                    window_text: String| ReferenceCoverageRow {
            product_key: product_key.to_owned(),
            product_label: product_label.to_owned(),
            delivery_point_key: point_key,
            delivery_point_label: point_label.to_owned(),
            availability_window: window_key.to_owned(),
            availability_window_label: window_text,
            benchmark_price_usd_per_mt: None,
            source_label: ReferenceSourceLabel::Other,
            generated_at: now,
            observed_at: None,
            source_kind: ReferenceSourceKind::Other,
            scope: ReferenceScope::Other,
            coverage_status: CoverageStatus::Unavailable,
        };
        let point_key = slugify(&cell.delivery_point_name);
        let mut row = base(
            &cell.product_code,
            &cell.product_label,
            point_key,
            &cell.delivery_point_name,
            &key.2,
            base_window,
        );
        if let Some(quote) = quote {
            row.benchmark_price_usd_per_mt = Some(quote.benchmark_price_usd_per_mt);
            row.generated_at = quote.generated_at;
            row.observed_at = quote.observed_at;
            row.coverage_status = if quote.observed_at.is_some() {
                CoverageStatus::Current
            } else {
                CoverageStatus::Stale
            };
            match quote.source.as_str() {
                "manual_override" => {
                    row.source_label = ReferenceSourceLabel::ManualOverride;
                    row.source_kind = ReferenceSourceKind::AdminBenchmark;
                    row.scope = ReferenceScope::ExactSlice;
                }
                "seed_matrix" => {
                    row.source_label = ReferenceSourceLabel::SeedMatrix;
                    row.source_kind = ReferenceSourceKind::SeededBenchmark;
                    row.scope = ReferenceScope::WindowAdjusted;
                }
                _ => {}
            }
        }
        rows.push(row);
    }
    rows.truncate(MAX_MATRIX_CELLS);
    Ok(ReferenceCoverageSection { rows })
}

/// Compose the marketplace tab. Only the sections matching the requested
/// activity are populated, and no aggregate sums across sources.
pub async fn compose(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    query_ref: &AnalyticsQuery,
    reference: &dyn ReferenceRead,
    now: DateTime<Utc>,
) -> Result<MarketplaceResponse, ApiError> {
    let mut quality = Quality::default();
    let mut live = None;
    let mut demo = None;
    let mut unknown = None;
    let mut commercial = None;

    if query_ref.activity.wants_activity() {
        let orders = aggregate_orders(conn, query_ref).await?;
        let trades = aggregate_trades(conn, query_ref).await?;
        let commissions = aggregate_commissions(conn, query_ref).await?;
        let statuses = status_distribution(conn, query_ref).await?;
        let matrix_rows = matrix_and_windows(conn, query_ref).await?;
        let trend_rows = balance_trend(conn, query_ref).await?;
        let quotes = eligible_quotes(conn, query_ref).await?;
        let fill_rows = time_to_fill(conn, query_ref).await?;

        let live_trades = provenance_row(&trades, "live", |row| row.provenance.as_str());
        quality.missing_commission_payment_date_count = commissions.missing_payment_date;
        if let Some(trades) = live_trades {
            quality.legacy_timestamp_fallback_count = trades.legacy_current + trades.legacy_previous;
            quality.missing_paid_at_count = trades.missing_paid_at_current;
        }

        let rows = SectionRows {
            quotes: &quotes,
            matrix: &matrix_rows,
            trend: &trend_rows,
            status: &statuses,
            fill: &fill_rows,
        };
        let sections = ordered_provenance_labels()
            .into_iter()
            .map(|provenance| {
                (
                    provenance,
                    activity_section(
                        policy,
                        provenance,
                        query_ref,
                        provenance_row(&orders, provenance, |row| row.provenance.as_str()),
                        provenance_row(&trades, provenance, |row| row.provenance.as_str()),
                        &rows,
                        &mut quality,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();

        if query_ref.activity.wants_live() {
            live = sections.get("live").cloned();
        }
        if query_ref.activity.wants_demo() {
            demo = sections.get("demo").cloned();
        }
        if query_ref.activity.wants_unknown() {
            unknown = sections.get("unknown").cloned();
        }
        if query_ref.activity.wants_live() {
            commercial = Some(super::commercial::summarize(
                live_trades,
                &commissions,
            ));
        }
    }

    let reference_section = if query_ref.activity.wants_reference() {
        Some(reference_section(conn, policy, query_ref, reference, now).await?)
    } else {
        None
    };

    let reference_status = if reference_section.is_some() {
        SourceStatus::Available
    } else {
        SourceStatus::NotApplicable
    };
    let meta = Meta {
        start: query_ref.start,
        end: query_ref.end,
        previous_start: query_ref.previous().map(|(start, _)| start),
        previous_end: query_ref.previous().map(|(_, end)| end),
        observed_at: now,
        coverage: Coverage {
            authoritative: SourceCoverage::available(now),
            behavioral: SourceCoverage::not_applicable(),
            login_history: SourceCoverage::not_applicable(),
            status_history: SourceCoverage::not_applicable(),
            reference: SourceCoverage {
                coverage_start: None,
                coverage_end: None,
                observed_at: None,
                status: reference_status,
                diagnostic: None,
            },
        },
        data_quality: quality.data_quality(),
    };

    Ok(MarketplaceResponse {
        meta,
        live,
        demo,
        unknown,
        reference: reference_section,
        commercial,
    })
}

/// Serialize a tab response for the HTTP adapter.
pub fn to_json<T: Serialize>(value: &T) -> Json {
    serde_json::to_value(value).unwrap_or(Json::Null)
}

// ---------------------------------------------------------------------------
// Coverage and meta
// ---------------------------------------------------------------------------

/// Behavioral coverage from a provider reading. A window that never arrived is
/// `unavailable` with the diagnostic that ended it, never `available`.
pub fn behavioral_coverage(reading: Option<&BehavioralWindow>) -> SourceCoverage {
    match reading {
        None => SourceCoverage {
            coverage_start: None,
            coverage_end: None,
            observed_at: None,
            status: SourceStatus::Unavailable,
            diagnostic: Some(Diagnostic::Upstream),
        },
        Some(window) => SourceCoverage {
            coverage_start: None,
            coverage_end: None,
            observed_at: Some(window.observed_at),
            status: window.status(),
            diagnostic: window.diagnostic(),
        },
    }
}

/// Fact-source coverage. `coverage_start` is the earliest persisted fact; a
/// period, or its comparison period, that starts before it is `partial` with
/// `insufficient_coverage` — never silently `available`.
pub fn fact_coverage(
    coverage_start: Option<NaiveDate>,
    query_ref: &AnalyticsQuery,
    now: DateTime<Utc>,
) -> SourceCoverage {
    let Some(coverage_start) = coverage_start else {
        return SourceCoverage {
            coverage_start: None,
            coverage_end: None,
            observed_at: None,
            status: SourceStatus::Unavailable,
            diagnostic: Some(Diagnostic::InsufficientCoverage),
        };
    };
    let start_of_period = query_ref.start.date_naive();
    let fully_covered = coverage_start <= start_of_period
        && query_ref
            .previous()
            .is_none_or(|(previous_start, _)| coverage_start <= previous_start.date_naive());
    SourceCoverage {
        coverage_start: Some(coverage_start.and_hms_opt(0, 0, 0).expect("midnight").and_utc()),
        coverage_end: None,
        observed_at: Some(now),
        status: if fully_covered {
            SourceStatus::Available
        } else {
            SourceStatus::Partial
        },
        diagnostic: if fully_covered {
            None
        } else {
            Some(Diagnostic::InsufficientCoverage)
        },
    }
}

/// Which fact sources a tab declares. `NotApplicable` is not the same as
/// unavailable: a tab that never reads login history says so explicitly.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    Applicable,
    NotApplicable,
}

/// Meta assembly. Every tab publishes the same shape; the options decide which
/// sources are applicable, so a tab never claims coverage it did not read.
pub struct MetaOptions<'a> {
    pub behavioral: Option<&'a BehavioralWindow>,
    pub behavioral_source: Source,
    pub login_coverage: Option<NaiveDate>,
    pub login_source: Source,
    pub status_coverage: Option<NaiveDate>,
    pub status_source: Source,
    pub reference_status: SourceStatus,
    pub data_quality: DataQuality,
}

impl Default for MetaOptions<'_> {
    fn default() -> Self {
        Self {
            behavioral: None,
            behavioral_source: Source::NotApplicable,
            login_coverage: None,
            login_source: Source::NotApplicable,
            status_coverage: None,
            status_source: Source::NotApplicable,
            reference_status: SourceStatus::NotApplicable,
            data_quality: DataQuality {
                legacy_timestamp_fallback_count: 0,
                missing_paid_at_count: 0,
                missing_commission_payment_date_count: 0,
                suppressed_cell_count: 0,
                cohort_complete: true,
            },
        }
    }
}

pub fn meta(query_ref: &AnalyticsQuery, now: DateTime<Utc>, options: MetaOptions<'_>) -> Meta {
    let (previous_start, previous_end) = match query_ref.previous() {
        Some((start, end)) => (Some(start), Some(end)),
        None => (None, None),
    };
    Meta {
        start: query_ref.start,
        end: query_ref.end,
        previous_start,
        previous_end,
        observed_at: now,
        coverage: Coverage {
            authoritative: SourceCoverage::available(now),
            behavioral: match options.behavioral_source {
                Source::Applicable => behavioral_coverage(options.behavioral),
                Source::NotApplicable => SourceCoverage::not_applicable(),
            },
            login_history: match options.login_source {
                Source::Applicable => fact_coverage(options.login_coverage, query_ref, now),
                Source::NotApplicable => SourceCoverage::not_applicable(),
            },
            status_history: match options.status_source {
                Source::Applicable => fact_coverage(options.status_coverage, query_ref, now),
                Source::NotApplicable => SourceCoverage::not_applicable(),
            },
            reference: SourceCoverage {
                coverage_start: None,
                coverage_end: None,
                observed_at: None,
                status: options.reference_status,
                diagnostic: None,
            },
        },
        data_quality: options.data_quality,
    }
}

// ---------------------------------------------------------------------------
// Query-string parsing
// ---------------------------------------------------------------------------

/// Longest accepted `availability_window`.
pub const MAX_WINDOW_CHARACTERS: usize = 16;
/// Longest accepted period, in seconds (365 days).
pub const MAX_RANGE_SECONDS: i64 = 31_536_000;

/// A raw, unvalidated analytics query: the query-string values as they arrived.
#[derive(Clone, Debug, Default)]
pub struct QueryInput {
    pub start: Option<String>,
    pub end: Option<String>,
    pub compare: Option<String>,
    pub audience: Option<String>,
    pub activity: Option<String>,
    pub product_id: Option<String>,
    pub delivery_point_id: Option<String>,
    pub availability_window: Option<String>,
}

impl QueryInput {
    /// Collect the recognised parameters; unknown parameters are ignored and
    /// never reach SQL or a provider.
    pub fn from_query_string(raw: &str) -> Self {
        let mut input = Self::default();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            let slot = match key.as_ref() {
                "start" => &mut input.start,
                "end" => &mut input.end,
                "compare" => &mut input.compare,
                "audience" => &mut input.audience,
                "activity" => &mut input.activity,
                "product_id" => &mut input.product_id,
                "delivery_point_id" => &mut input.delivery_point_id,
                "availability_window" => &mut input.availability_window,
                _ => continue,
            };
            // A repeated parameter is ambiguous; the last one wins, as the
            // framework-level scalar binding this replaces also did.
            *slot = Some(value.into_owned());
        }
        input
    }
}

/// Parse one ISO-8601 timestamp. A value without an offset is UTC, an explicit
/// offset is converted to UTC, and anything else is refused.
pub fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(parsed.with_timezone(&Utc));
    }
    // A space separator is valid ISO 8601 and accepted by the reference parser.
    let normalized = trimmed.replacen(' ', "T", 1);
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(naive.and_utc());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M") {
        return Some(naive.and_utc());
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Validate a raw query into the typed form.
///
/// Every refusal is `422 ANALYTICS_QUERY_INVALID`. The previous-window
/// subtraction is checked: an underflowing period is refused, never wrapped
/// into a different window.
pub fn parse_query(input: QueryInput) -> Result<AnalyticsQuery, ApiError> {
    let start = input
        .start
        .as_deref()
        .and_then(parse_timestamp)
        .ok_or_else(|| invalid_query("start must be a timestamp"))?;
    let end = input
        .end
        .as_deref()
        .and_then(parse_timestamp)
        .ok_or_else(|| invalid_query("end must be a timestamp"))?;
    if end <= start {
        return Err(invalid_query("end must be after start"));
    }
    let elapsed = end - start;
    if elapsed.num_seconds() > MAX_RANGE_SECONDS {
        return Err(invalid_query("the period must not exceed 365 days"));
    }
    let compare = match input.compare.as_deref() {
        None => true,
        Some(value) => parse_bool(value).ok_or_else(|| invalid_query("compare must be a bool"))?,
    };
    // `audience` is validated and carried by the query but applies no SQL
    // predicate: the Python business reference uses it only in its aggregate
    // cache key, and CF4_03 forbids a Phase 4 result cache. Accepting it keeps
    // the client contract; inventing a filter would change every tab's totals.
    let audience = match input.audience.as_deref() {
        None => Audience::All,
        Some(value) => Audience::parse(value)
            .ok_or_else(|| invalid_query("audience must be ALL, BUYER or SUPPLIER"))?,
    };
    let activity = match input.activity.as_deref() {
        None => Activity::Live,
        Some(value) => Activity::parse(value)
            .ok_or_else(|| invalid_query("activity must be LIVE, DEMO, REFERENCE or ALL"))?,
    };
    let product_id = match input.product_id.as_deref() {
        None => None,
        Some(value) => Some(
            Uuid::parse_str(value)
                .map_err(|_| invalid_query("product_id must be a UUID"))?,
        ),
    };
    let delivery_point_id = match input.delivery_point_id.as_deref() {
        None => None,
        Some(value) => Some(
            Uuid::parse_str(value)
                .map_err(|_| invalid_query("delivery_point_id must be a UUID"))?,
        ),
    };
    if let Some(window) = input.availability_window.as_deref()
        && window.chars().count() > MAX_WINDOW_CHARACTERS
    {
        return Err(invalid_query(
            "availability_window must be at most 16 characters",
        ));
    }
    let query_ref = AnalyticsQuery {
        start,
        end,
        compare,
        audience,
        activity,
        product_id,
        delivery_point_id,
        availability_window: input.availability_window,
    };
    check_previous(&query_ref)?;
    Ok(query_ref)
}

/// Checked period arithmetic.
///
/// A comparison request whose subtraction is not representable is refused; it
/// is never served with a silently missing comparison and never wrapped into a
/// different window. The guard is exercised directly, because the four-digit
/// RFC 3339 years a query string can carry stay far inside the representable
/// range: the check exists for values assembled by a caller, not for hostile
/// input reaching it through this parser.
pub fn check_previous(query_ref: &AnalyticsQuery) -> Result<(), ApiError> {
    if query_ref.compare && query_ref.previous().is_none() {
        return Err(invalid_query(
            "the comparison window is not representable for this period",
        ));
    }
    Ok(())
}


// ---------------------------------------------------------------------------
// Route adapters
// ---------------------------------------------------------------------------

use std::sync::Arc;

/// The three product-analytics tabs this lane owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyticsRoute {
    Acquisition,
    Marketplace,
    Reliability,
}

impl AnalyticsRoute {
    pub const ALL: [Self; 3] = [Self::Acquisition, Self::Marketplace, Self::Reliability];

    pub fn method(self) -> &'static str {
        "GET"
    }

    pub fn template(self) -> &'static str {
        match self {
            Self::Acquisition => "/api/admin/analytics/product-analytics/acquisition",
            Self::Marketplace => "/api/admin/analytics/product-analytics/marketplace",
            Self::Reliability => "/api/admin/analytics/product-analytics/reliability",
        }
    }

    /// `activity=REFERENCE` is a marketplace-only input.
    pub fn accepts_reference(self) -> bool {
        matches!(self, Self::Marketplace)
    }
}

/// The behavioral and reference seams a mounted adapter consumes. Both are
/// typed: no raw provider body and no second market policy reach a tab.
#[derive(Clone)]
pub struct AnalyticsService {
    pub behavioral: Arc<dyn BehavioralRead>,
    pub reference: Arc<dyn ReferenceRead>,
}

impl std::fmt::Debug for AnalyticsService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AnalyticsService")
    }
}

/// A whole-window provider reading is bounded by one budget. This is an
/// admission bound, not a measurement; expiry yields a typed `timeout`
/// diagnostic and never a partial success.
fn provider_budget() -> std::time::Duration {
    std::time::Duration::from_secs(10)
}

async fn read_window(
    service: &AnalyticsService,
    query: &AnalyticsQuery,
    properties: &AllowedPropertySet,
) -> BehavioralWindow {
    tokio::select! {
        window = service.behavioral.window(query, properties) => window,
        _ = tokio::time::sleep(provider_budget()) => {
            BehavioralWindow::unavailable(Diagnostic::Timeout, Utc::now())
        }
    }
}

async fn read_windows(
    service: &AnalyticsService,
    query: &AnalyticsQuery,
    properties: &AllowedPropertySet,
) -> (BehavioralWindow, Option<BehavioralWindow>) {
    let current = read_window(service, query, properties).await;
    let previous = match query.previous() {
        Some((start, end)) => {
            let mut comparison = query.clone();
            comparison.start = start;
            comparison.end = end;
            comparison.compare = false;
            Some(read_window(service, &comparison, properties).await)
        }
        None => None,
    };
    (current, previous)
}

fn parse_state(raw: &str) -> Result<AnalyticsQuery, ApiError> {
    parse_query(QueryInput::from_query_string(raw))
}

/// Administrator authority, then token quota, in that order.
///
/// This is the one admission decision every tab route uses. A non-administrator
/// is refused before the quota counter changes, so a refused caller cannot
/// exhaust another caller's window, and an anonymous caller never reaches it.
/// The route validates the query between the two, so a malformed request is a
/// 422 that also spends nothing.
pub fn admit_analytics(
    actor: &crate::auth::CurrentActor,
    limits: &crate::auth::limits::SharedLimits,
    route: AnalyticsRoute,
    client: &str,
    token: &str,
    now: u64,
) -> Result<(), ApiError> {
    require_admin(actor)?;
    token_limit(limits, route, client, token, now)
}

/// The validated request plus the quota admission, in route order.
///
/// Split out so the ordering is testable without HTTP: authority first, then
/// the query, then quota. A caller that fails either check leaves the token
/// window untouched.
pub fn admit_tab(
    actor: &crate::auth::CurrentActor,
    limits: &crate::auth::limits::SharedLimits,
    route: AnalyticsRoute,
    client: &str,
    token: &str,
    now: u64,
    raw_query: &str,
) -> Result<AnalyticsQuery, ApiError> {
    require_admin(actor)?;
    let query = parse_state(raw_query)?;
    if !route.accepts_reference() {
        refuse_reference(&query)?;
    }
    token_limit(limits, route, client, token, now)?;
    Ok(query)
}

fn token_limit(
    limits: &crate::auth::limits::SharedLimits,
    route: AnalyticsRoute,
    client: &str,
    token: &str,
    now: u64,
) -> Result<(), ApiError> {
    limits.check_business(
        route.method(),
        route.template(),
        crate::auth::limits::BusinessLimitKeys {
            client,
            user: None,
            organization: None,
            token: Some(token),
        },
        now,
    )
}

/// Administrator authority is its own step: a non-admin is refused before any
/// quota is spent and before any target is read.
fn require_admin(actor: &crate::auth::CurrentActor) -> Result<(), ApiError> {
    if actor.role.as_deref() != Some("ADMIN") {
        return Err(crate::business::error::refusal(
            axum::http::StatusCode::FORBIDDEN,
            "ADMIN_REQUIRED",
            "Administrator access is required.",
        ));
    }
    Ok(())
}

impl AnalyticsService {
    /// Read one tab on a caller-owned connection. The caller has already
    /// authenticated, admitted the quota and parsed the query.
    /// Read the behavioral provider window for a tab, before any database
    /// connection is taken. A tab that needs no provider reads nothing here.
    ///
    /// The plan forbids holding a pooled connection through provider I/O, so
    /// this phase and [`Self::compose`] are separate steps and the caller takes
    /// the connection only for the second one.
    pub async fn provider_read(
        &self,
        route: AnalyticsRoute,
        query: &AnalyticsQuery,
    ) -> ProviderReadings {
        let properties = match route {
            AnalyticsRoute::Acquisition => super::acquisition::requested_properties(),
            AnalyticsRoute::Reliability => super::reliability::requested_properties(),
            AnalyticsRoute::Marketplace => AllowedPropertySet::not_requested(),
        };
        if !matches!(
            route,
            AnalyticsRoute::Acquisition | AnalyticsRoute::Reliability
        ) {
            return ProviderReadings::not_requested();
        }
        let (current, previous) = read_windows(self, query, &properties).await;
        ProviderReadings {
            current: Some(super::acquisition::Reading { window: current }),
            previous: previous.map(|window| super::acquisition::Reading { window }),
        }
    }

    /// Compose a tab on a caller-owned connection, with the provider reading
    /// already taken by [`Self::provider_read`].
    pub async fn compose(
        &self,
        conn: &mut PgConnection,
        policy: &MarketPolicy,
        route: AnalyticsRoute,
        query: &AnalyticsQuery,
        readings: &ProviderReadings,
        now: DateTime<Utc>,
    ) -> Result<Json, ApiError> {
        match route {
            AnalyticsRoute::Marketplace => {
                let response = compose(conn, policy, query, self.reference.as_ref(), now).await?;
                Ok(to_json(&response))
            }
            AnalyticsRoute::Acquisition => {
                let meta = meta(
                    query,
                    now,
                    MetaOptions {
                        behavioral: readings.current.as_ref().map(|reading| &reading.window),
                        behavioral_source: Source::Applicable,
                        ..MetaOptions::default()
                    },
                );
                let mut quality = Quality::default();
                let response = super::acquisition::compose(
                    query,
                    meta,
                    readings.current.as_ref(),
                    readings.previous.as_ref(),
                    &mut quality,
                );
                Ok(with_suppression(to_json(&response), &quality))
            }
            AnalyticsRoute::Reliability => {
                // Provider readings are already in hand, so the connection is
                // used for the bounded audit sample and nothing else.
                let audit = super::reliability::audit_activity(conn, query.start, query.end).await?;
                let meta = meta(
                    query,
                    now,
                    MetaOptions {
                        behavioral: readings.current.as_ref().map(|reading| &reading.window),
                        behavioral_source: Source::Applicable,
                        ..MetaOptions::default()
                    },
                );
                let mut quality = Quality::default();
                let response = super::reliability::compose(
                    query,
                    meta,
                    readings.current.as_ref(),
                    readings.previous.as_ref(),
                    audit,
                    &mut quality,
                );
                Ok(with_suppression(to_json(&response), &quality))
            }
        }
    }
}

/// A tab's typed provider reading. `None` means the tab never asked the
/// provider (marketplace), which is not the same as an unavailable window.
#[derive(Clone, Debug, Default)]
pub struct ProviderReadings {
    pub current: Option<super::acquisition::Reading>,
    pub previous: Option<super::acquisition::Reading>,
}

impl ProviderReadings {
    pub fn not_requested() -> Self {
        Self::default()
    }
}

/// The suppression count is final only once the whole tab is composed, so it is
/// published into `meta.data_quality` here.
fn with_suppression(mut body: Json, quality: &Quality) -> Json {
    if let Some(quality_node) = body
        .get_mut("meta")
        .and_then(|meta| meta.get_mut("data_quality"))
    {
        *quality_node = to_json(&quality.data_quality());
    }
    body
}

/// Dispatch one mounted analytics request.
///
/// Order is fixed and identical for every tab: authenticate the bearer and
/// current account, then require administrator authority, then consume the
/// token quota, then parse the query, then read. A refusal at any step costs no
/// quota beyond the steps already passed, and no resource is read before its
/// authority is established.
pub async fn dispatch(
    app: &crate::AppState,
    verification: &crate::auth::VerificationSettings,
    limits: &crate::auth::limits::SharedLimits,
    service: &AnalyticsService,
    route: AnalyticsRoute,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let now = Utc::now();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let access = match crate::auth::verify_authorization(authorization.as_deref(), verification, now)
    {
        Ok(value) => value,
        Err(error) => return crate::business::error::authentication(error).into_response(),
    };
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = crate::runtime::http::client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let raw_query = request.uri().query().unwrap_or("").to_owned();
    let token = authorization.clone().unwrap_or_default();
    let limits = limits.clone();
    let now_seconds = now.timestamp().max(0) as u64;

    // Order: authenticate the current account, require administrator
    // authority, validate the query, then consume quota. A refused authority or
    // a malformed query therefore never spends a token, and every refusal is
    // the same refusal whichever tab it arrives at.
    let admission = crate::auth::run_authenticated(
        app,
        access,
        crate::auth::AuthenticatedRequest::new(route.method(), route.template(), false),
        move |tx| {
            Box::pin(async move {
                admit_tab(
                    tx.actor(),
                    &limits,
                    route,
                    &client,
                    &token,
                    now_seconds,
                    &raw_query,
                )
            })
        },
    )
    .await;
    let query = match admission {
        Ok(query) => query,
        Err(crate::auth::AuthenticatedError::Authentication(error)) => {
            return crate::business::error::authentication(error).into_response();
        }
        Err(crate::auth::AuthenticatedError::Database(error)) => return error.into_response(),
        Err(crate::auth::AuthenticatedError::AmbiguousCommit) => {
            return crate::ApiError::database().into_response();
        }
    };

    // Provider I/O happens before a pooled connection is taken: the plan
    // forbids holding one through a remote call, and an unavailable provider
    // must not tie up a database slot for its whole timeout.
    let readings = service.provider_read(route, &query).await;

    // The admitted owner is detached from the caller, so it owns everything it
    // needs: no borrow of `app`, the request or the caller outlives this call.
    let policy = app.policy.clone();
    let service = AnalyticsService::clone(service);
    let result = crate::runtime::admission::run_admitted(app, move |conn| {
        Box::pin(async move {
            if route == AnalyticsRoute::Marketplace {
                validate_marketplace_filters(conn, &query).await?;
            }
            service
                .compose(conn, policy.as_ref(), route, &query, &readings, now)
                .await
        })
    })
    .await;
    match result {
        Ok(body) => (axum::http::StatusCode::OK, axum::Json(body)).into_response(),
        Err(error) => error.into_response(),
    }
}

#[derive(Clone)]
struct RouteState {
    app: crate::AppState,
    verification: Arc<crate::auth::VerificationSettings>,
    limits: crate::auth::limits::SharedLimits,
    service: AnalyticsService,
}

/// Mount the three product-analytics tabs. The integration owner calls this
/// from the central router; the lane mounts nothing by itself.
pub fn router(
    app: crate::AppState,
    verification: crate::auth::VerificationSettings,
    limits: crate::auth::limits::SharedLimits,
    service: AnalyticsService,
) -> axum::Router {
    use axum::{Router, routing::get};
    Router::new()
        .route(AnalyticsRoute::Acquisition.template(), get(acquisition_route))
        .route(AnalyticsRoute::Marketplace.template(), get(marketplace_route))
        .route(AnalyticsRoute::Reliability.template(), get(reliability_route))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            limits,
            service,
        })
}

async fn acquisition_route(
    axum::extract::State(state): axum::extract::State<RouteState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let RouteState {
        app,
        verification,
        limits,
        service,
    } = state;
    dispatch(
        &app,
        &verification,
        &limits,
        &service,
        AnalyticsRoute::Acquisition,
        request,
    )
    .await
}

async fn marketplace_route(
    axum::extract::State(state): axum::extract::State<RouteState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let RouteState {
        app,
        verification,
        limits,
        service,
    } = state;
    dispatch(
        &app,
        &verification,
        &limits,
        &service,
        AnalyticsRoute::Marketplace,
        request,
    )
    .await
}

async fn reliability_route(
    axum::extract::State(state): axum::extract::State<RouteState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let RouteState {
        app,
        verification,
        limits,
        service,
    } = state;
    dispatch(
        &app,
        &verification,
        &limits,
        &service,
        AnalyticsRoute::Reliability,
        request,
    )
    .await
}
