"""Typed API contracts for the Product Analytics workspace.

Defined by fe/docs/plans/2026-07-15-product-analytics-workspace.md §2. The
backend owns every metric definition; these models are presentation-ready
aggregate contracts. Hard rules encoded here:

- Periods are half-open UTC intervals ``[start, end)``; the previous
  comparison period is ``[start - (end - start), start)``.
- ``null`` (not zero) marks unavailable or mathematically invalid metrics.
- Suppressed cells are typed (``AggregateCell``), never zeros or magic
  strings; derived values render only when every input is unsuppressed.
- Responses never carry emails, names, user/organization/order/trade
  identifiers, IP addresses, or free text; breakdown keys come from
  server-owned allowlists.
- Arrays are bounded: daily series ≤ 366 points, ranked lists ≤ 20 rows,
  matrices ≤ canonical catalog cardinality.
"""

from __future__ import annotations

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from enum import Enum
from typing import Literal
from uuid import UUID

from pydantic import BaseModel, Field, field_validator, model_validator

MAX_RANGE_DAYS = 365
_MAX_SERIES_POINTS = 366
_MAX_RANKED_ROWS = 20
# Matrices are bounded by canonical catalog cardinality (products × delivery
# points); this static ceiling is a defensive backstop above any real catalog.
_MAX_MATRIX_CELLS = 512
_MAX_COHORT_ROWS = 54  # 53 ISO weeks + safety


# ---------------------------------------------------------------------------
# Shared query (§2.1)
# ---------------------------------------------------------------------------


class AnalyticsAudience(str, Enum):
    ALL = "ALL"
    BUYER = "BUYER"
    SUPPLIER = "SUPPLIER"


class AnalyticsActivity(str, Enum):
    LIVE = "LIVE"
    DEMO = "DEMO"
    REFERENCE = "REFERENCE"
    ALL = "ALL"


class ProductAnalyticsQuery(BaseModel):
    start: datetime
    end: datetime
    compare: bool = True
    audience: AnalyticsAudience = AnalyticsAudience.ALL
    activity: AnalyticsActivity = AnalyticsActivity.LIVE
    product_id: UUID | None = None
    delivery_point_id: UUID | None = None
    availability_window: str | None = Field(default=None, max_length=16)

    @field_validator("start", "end")
    @classmethod
    def _normalize_utc(cls, value: datetime) -> datetime:
        if value.tzinfo is None:
            return value.replace(tzinfo=UTC)
        return value.astimezone(UTC)

    @model_validator(mode="after")
    def _validate_range(self) -> "ProductAnalyticsQuery":
        if self.end <= self.start:
            raise ValueError("end must be after start")
        if self.end - self.start > timedelta(days=MAX_RANGE_DAYS):
            raise ValueError(f"range must not exceed {MAX_RANGE_DAYS} days")
        return self

    @property
    def previous_start(self) -> datetime | None:
        if not self.compare:
            return None
        return self.start - (self.end - self.start)

    @property
    def previous_end(self) -> datetime | None:
        if not self.compare:
            return None
        return self.start


# ---------------------------------------------------------------------------
# Meta, coverage, and data quality (§2.2)
# ---------------------------------------------------------------------------


class AnalyticsDiagnostic(str, Enum):
    DISABLED = "disabled"
    CONFIGURATION = "configuration"
    AUTHENTICATION = "authentication"
    TIMEOUT = "timeout"
    UPSTREAM = "upstream"
    MALFORMED_RESPONSE = "malformed_response"
    INSUFFICIENT_COVERAGE = "insufficient_coverage"


class AnalyticsDataQuality(BaseModel):
    legacy_timestamp_fallback_count: int = 0
    missing_paid_at_count: int = 0
    missing_commission_payment_date_count: int = 0
    suppressed_cell_count: int = 0
    cohort_complete: bool = True


class AnalyticsSourceStatus(str, Enum):
    AVAILABLE = "available"
    PARTIAL = "partial"
    UNAVAILABLE = "unavailable"
    NOT_APPLICABLE = "not_applicable"


class AnalyticsSourceCoverage(BaseModel):
    coverage_start: datetime | None = None
    coverage_end: datetime | None = None
    observed_at: datetime | None = None
    status: AnalyticsSourceStatus
    diagnostic: AnalyticsDiagnostic | None = None

    @classmethod
    def not_applicable(cls) -> "AnalyticsSourceCoverage":
        return cls(status=AnalyticsSourceStatus.NOT_APPLICABLE)


class AnalyticsCoverage(BaseModel):
    authoritative: AnalyticsSourceCoverage
    behavioral: AnalyticsSourceCoverage
    login_history: AnalyticsSourceCoverage
    status_history: AnalyticsSourceCoverage
    reference: AnalyticsSourceCoverage


class AnalyticsMeta(BaseModel):
    start: datetime
    end: datetime
    previous_start: datetime | None
    previous_end: datetime | None
    observed_at: datetime
    coverage: AnalyticsCoverage
    data_quality: AnalyticsDataQuality


class AggregateCell(BaseModel):
    """A count cell that can be privacy-suppressed (§1.4 rule 12).

    Suppression is explicit: a suppressed cell has ``count=None`` and
    ``suppressed=True``; an unavailable-but-not-suppressed metric is plain
    ``count=None``. Zero always means a genuine zero.
    """

    key: str
    count: int | None = None
    suppressed: bool = False

    @model_validator(mode="after")
    def _suppressed_cells_have_no_count(self) -> "AggregateCell":
        if self.suppressed and self.count is not None:
            raise ValueError("a suppressed cell must not carry a count")
        return self


# ---------------------------------------------------------------------------
# Shared value/series/rank primitives
# ---------------------------------------------------------------------------


class MetricValue(BaseModel):
    """Headline integer metric with previous-period comparison."""

    value: int | None = None
    previous: int | None = None
    suppressed: bool = False


class DecimalMetricValue(BaseModel):
    """Headline decimal metric (money, volume, duration) with comparison."""

    value: Decimal | None = None
    previous: Decimal | None = None
    suppressed: bool = False


class RatioValue(BaseModel):
    """A ratio published with its exact components.

    ``rate_pct`` is present only when both components are unsuppressed and
    the denominator is positive. ``cohort_complete=False`` marks a
    right-censored cohort that can still change (§1.5).
    """

    numerator: int | None = None
    denominator: int | None = None
    rate_pct: Decimal | None = None
    suppressed: bool = False
    cohort_complete: bool = True


class SeriesPoint(BaseModel):
    date: date
    value: int | None = None


class DecimalSeriesPoint(BaseModel):
    date: date
    value: Decimal | None = None


class RankedRow(BaseModel):
    """A ranked breakdown row keyed by a server-owned allowlisted value."""

    key: str
    label: str
    count: int | None = None
    share_pct: Decimal | None = None
    suppressed: bool = False


def _series_field():
    return Field(default_factory=list, max_length=_MAX_SERIES_POINTS)


def _ranked_field():
    return Field(default_factory=list, max_length=_MAX_RANKED_ROWS)


# ---------------------------------------------------------------------------
# Canonical registries (§1.4 rule 11, §2.5)
# ---------------------------------------------------------------------------


class RouteFamily(str, Enum):
    LANDING = "landing"
    SIGNUP = "signup"
    PLATFORM = "platform"
    ADMIN = "admin"


class FrontendErrorCategory(str, Enum):
    RENDER = "render"
    CHUNK = "chunk"
    NETWORK = "network"
    UNKNOWN = "unknown"


class NavigationDestination(str, Enum):
    HOME = "home"
    MAP = "map"
    MARKETPLACE = "marketplace"
    CURVE = "curve"
    WATCHLIST = "watchlist"
    ANALYTICS = "analytics"
    TRADES = "trades"
    QUOTES = "quotes"
    COMPLIANCE = "compliance"
    TRAINING = "training"
    SETTINGS = "settings"
    ADMIN = "admin"


class NavigationLatencyBucket(str, Enum):
    LT_250 = "lt250"
    FROM_250_TO_500 = "250_500"
    FROM_500_TO_1000 = "500_1000"
    FROM_1000_TO_2500 = "1000_2500"
    GTE_2500 = "gte2500"


class LifecycleStageKey(str, Enum):
    VISITORS = "visitors"
    REGISTERED = "registered"
    ACTIVE = "active"
    PARTICIPATING = "participating"
    TRADING = "trading"
    RETAINED = "retained"


class NeedsAttentionRule(str, Enum):
    APPROVED_MEMBERS_NEVER_LOGGED_IN = "approved_members_never_logged_in"
    SIGNUP_SUBMISSION_DROP = "signup_submission_drop"
    ONE_SIDED_LIVE_MARKET = "one_sided_live_market"
    ELEVATED_LOGIN_FAILURES = "elevated_login_failures"
    DEGRADED_ANALYTICS_COLLECTION = "degraded_analytics_collection"


# ---------------------------------------------------------------------------
# Overview (§1.6)
# ---------------------------------------------------------------------------


class LifecycleStage(BaseModel):
    """One stage of the lifecycle spine. Stages mix entity types (anonymous
    visitors, people, organizations), so no cross-stage conversion is ever
    published here — counts and deltas only."""

    key: LifecycleStageKey
    count: int | None = None
    previous: int | None = None
    coverage: AnalyticsSourceStatus
    detail_tab: str


class ActivityTrend(BaseModel):
    visitors: list[SeriesPoint] = _series_field()
    active_members: list[SeriesPoint] = _series_field()
    orders: list[SeriesPoint] = _series_field()
    confirmed_trades: list[SeriesPoint] = _series_field()


class MarketplaceBalance(BaseModel):
    buyer_organizations: MetricValue
    supplier_organizations: MetricValue
    bid_orders: MetricValue
    ask_orders: MetricValue


class NeedsAttentionItem(BaseModel):
    rule: NeedsAttentionRule
    count: int | None = None


class OverviewKpis(BaseModel):
    qualified_organizations: MetricValue
    active_members: MetricValue
    participating_organizations: MetricValue
    live_orders: MetricValue
    confirmed_trades: MetricValue


class OverviewResponse(BaseModel):
    meta: AnalyticsMeta
    kpis: OverviewKpis
    lifecycle: list[LifecycleStage] = Field(default_factory=list, max_length=6)
    activity_trend: ActivityTrend
    marketplace_balance: MarketplaceBalance
    needs_attention: list[NeedsAttentionItem] = Field(default_factory=list, max_length=10)


# ---------------------------------------------------------------------------
# Acquisition (§1.6)
# ---------------------------------------------------------------------------


class AcquisitionKpis(BaseModel):
    visitors: MetricValue
    visits: MetricValue
    pageviews: MetricValue
    average_session_duration_seconds: DecimalMetricValue
    cta_clicks: MetricValue


class CtaMatrixRow(BaseModel):
    cta: str
    placement: str
    clicks: int | None = None
    share_pct: Decimal | None = None
    suppressed: bool = False


class CalculatorFunnel(BaseModel):
    starts: MetricValue
    completions: MetricValue


class AcquisitionResponse(BaseModel):
    meta: AnalyticsMeta
    kpis: AcquisitionKpis
    visitors_trend: list[SeriesPoint] = _series_field()
    previous_visitors_trend: list[SeriesPoint] = _series_field()
    visits_trend: list[SeriesPoint] = _series_field()
    # Referrer keys are normalized bounded hostnames, plus the reserved keys
    # ``direct`` (empty referrer) and ``other`` (§1.4 rule 13).
    referrers: list[RankedRow] = _ranked_field()
    entry_pages: list[RankedRow] = _ranked_field()
    cta_matrix: list[CtaMatrixRow] = _ranked_field()
    languages: list[RankedRow] = _ranked_field()
    calculator: CalculatorFunnel


# ---------------------------------------------------------------------------
# Activation (§1.6)
# ---------------------------------------------------------------------------


class JourneySource(str, Enum):
    BEHAVIORAL = "behavioral"
    AUTHORITATIVE = "authoritative"


class JourneyStage(BaseModel):
    key: str
    source: JourneySource
    total: AggregateCell
    buyer: AggregateCell | None = None
    supplier: AggregateCell | None = None


class DurationBucket(BaseModel):
    bucket: str
    cell: AggregateCell


class DurationDistribution(BaseModel):
    buckets: list[DurationBucket] = Field(default_factory=list, max_length=12)
    median_hours: Decimal | None = None
    sample_size: int | None = None
    suppressed: bool = False


class ConversionRatioKind(str, Enum):
    USER_COHORT = "user_cohort"
    ORGANIZATION_COHORT = "organization_cohort"
    AGGREGATE_EVENT = "aggregate_event"


class LabeledRatio(BaseModel):
    """A ratio whose numerator and denominator share one entity type and a
    compatible collection window; behavioral ratios are labelled aggregate
    event ratios and never mix with database cohorts (§1.6 Activation)."""

    key: str
    kind: ConversionRatioKind
    ratio: RatioValue


class ActivationResponse(BaseModel):
    meta: AnalyticsMeta
    journey: list[JourneyStage] = Field(default_factory=list, max_length=12)
    time_to_first_login: DurationDistribution
    time_to_first_live_order: DurationDistribution
    drop_off: list[AggregateCell] = Field(default_factory=list, max_length=8)
    ratios: list[LabeledRatio] = Field(default_factory=list, max_length=12)


# ---------------------------------------------------------------------------
# Engagement (§1.6)
# ---------------------------------------------------------------------------


class EngagementKpis(BaseModel):
    dau: MetricValue
    wau: MetricValue
    mau: MetricValue
    stickiness_pct: Decimal | None = None


class FeatureAdoptionRow(BaseModel):
    family: str
    events: int | None = None
    suppressed: bool = False


class NavigationDestinationRow(BaseModel):
    destination: NavigationDestination
    buyer: AggregateCell
    supplier: AggregateCell


class TutorialStepRow(BaseModel):
    step: str
    completed: AggregateCell
    skipped: AggregateCell


class EngagementResponse(BaseModel):
    meta: AnalyticsMeta
    kpis: EngagementKpis
    active_members_trend: list[SeriesPoint] = _series_field()
    feature_adoption: list[FeatureAdoptionRow] = _ranked_field()
    workflow_ratios: list[LabeledRatio] = Field(default_factory=list, max_length=12)
    navigation_destinations: list[NavigationDestinationRow] = Field(
        default_factory=list, max_length=12
    )
    tutorial_steps: list[TutorialStepRow] = _ranked_field()


# ---------------------------------------------------------------------------
# Marketplace (§1.6)
# ---------------------------------------------------------------------------


class MarketplaceKpis(BaseModel):
    participating_organizations: MetricValue
    open_bids: MetricValue
    open_asks: MetricValue
    confirmed_trades: MetricValue
    confirmed_volume_mt: DecimalMetricValue
    execution_rate: RatioValue


class SliceLiquidityRow(BaseModel):
    """Liquidity diagnostics for one exact product+port+window slice.

    Price, depth, and spread fields are populated only when the slice is
    executable (all coordinates specific) and at least three distinct live
    organizations contribute; otherwise ``suppressed=True`` (§1.6).
    """

    product_key: str
    product_label: str
    delivery_point_key: str
    delivery_point_label: str
    availability_window: str
    availability_window_label: str
    contributing_organizations: int | None = None
    best_bid_usd_per_mt: Decimal | None = None
    best_ask_usd_per_mt: Decimal | None = None
    spread_usd_per_mt: Decimal | None = None
    spread_bps: Decimal | None = None
    best_bid_depth_mt: Decimal | None = None
    best_ask_depth_mt: Decimal | None = None
    one_percent_bid_depth_mt: Decimal | None = None
    one_percent_ask_depth_mt: Decimal | None = None
    crossed: bool | None = None
    suppressed: bool = False


class LiquiditySummary(BaseModel):
    two_sided_slices: int | None = None
    one_sided_slices: int | None = None
    crossed_slices: int | None = None
    median_spread_usd_per_mt: Decimal | None = None
    median_spread_bps: Decimal | None = None
    median_open_order_age_hours: Decimal | None = None
    median_hours_to_first_fill: Decimal | None = None
    slices: list[SliceLiquidityRow] = _ranked_field()


class MarketBalanceTrend(BaseModel):
    buyer_organizations: list[SeriesPoint] = _series_field()
    supplier_organizations: list[SeriesPoint] = _series_field()
    bids: list[SeriesPoint] = _series_field()
    asks: list[SeriesPoint] = _series_field()


class ProductPortCell(BaseModel):
    product_key: str
    product_label: str
    delivery_point_key: str
    delivery_point_label: str
    orders: AggregateCell
    organizations: AggregateCell


class ConcentrationBand(str, Enum):
    LOW = "low"
    MODERATE = "moderate"
    HIGH = "high"


class OrganizationConcentration(BaseModel):
    """Aggregate concentration only — never names, IDs, or per-organization
    shares (§1.6 Marketplace)."""

    hhi_band: ConcentrationBand | None = None
    suppressed: bool = False


class CommercialSummary(BaseModel):
    realized_gmv_usd: DecimalMetricValue
    realized_revenue_usd: DecimalMetricValue
    commission_pending_usd: Decimal | None = None
    commission_invoiced_usd: Decimal | None = None


class MarketActivitySection(BaseModel):
    """Order/trade aggregates for one provenance source. ``ALL`` responses
    return one section per source; sources are never summed (§1.4 rule 15)."""

    kpis: MarketplaceKpis
    liquidity: LiquiditySummary
    balance_trend: MarketBalanceTrend
    product_port_matrix: list[ProductPortCell] = Field(
        default_factory=list, max_length=_MAX_MATRIX_CELLS
    )
    window_distribution: list[RankedRow] = _ranked_field()
    order_status_distribution: list[AggregateCell] = Field(default_factory=list, max_length=8)
    trade_status_distribution: list[AggregateCell] = Field(default_factory=list, max_length=8)
    concentration: OrganizationConcentration


class ReferenceSourceLabel(str, Enum):
    """Display registry for benchmark provenance; unknown DB values map to
    OTHER, never to arbitrary response strings."""

    MANUAL_OVERRIDE = "MANUAL_OVERRIDE"
    SEED_MATRIX = "SEED_MATRIX"
    OTHER = "OTHER"


class ReferenceSourceKind(str, Enum):
    ADMIN_BENCHMARK = "ADMIN_BENCHMARK"
    SEEDED_BENCHMARK = "SEEDED_BENCHMARK"
    # Trade-derived VWAP from /price-discovery/reference is price-discovery
    # coverage and must never be relabelled as an external benchmark.
    TRADE_DERIVED_VWAP = "TRADE_DERIVED_VWAP"
    OTHER = "OTHER"


class ReferenceScope(str, Enum):
    EXACT_SLICE = "EXACT_SLICE"
    WINDOW_ADJUSTED = "WINDOW_ADJUSTED"
    OTHER = "OTHER"


class ReferenceCoverageRow(BaseModel):
    """Benchmark/price-discovery coverage for one canonical slice (§1.6).

    Reference is coverage, never activity: no participants, orders, depth,
    spread, execution, GMV, or revenue. ``current`` requires price and
    observation timestamp; ``stale`` may keep a price with a null
    observation timestamp (seeded legacy); ``unavailable`` has neither.
    """

    product_key: str
    product_label: str
    delivery_point_key: str
    delivery_point_label: str
    availability_window: str
    availability_window_label: str
    benchmark_price_usd_per_mt: Decimal | None = None
    source_label: ReferenceSourceLabel
    generated_at: datetime
    observed_at: datetime | None = None
    source_kind: ReferenceSourceKind
    scope: ReferenceScope
    coverage_status: Literal["current", "stale", "unavailable"]

    @model_validator(mode="after")
    def _status_matches_fields(self) -> "ReferenceCoverageRow":
        has_price = self.benchmark_price_usd_per_mt is not None
        has_observation = self.observed_at is not None
        if self.coverage_status == "current" and not (has_price and has_observation):
            raise ValueError("current coverage requires price and observation timestamp")
        if self.coverage_status == "unavailable" and (has_price or has_observation):
            raise ValueError("unavailable coverage must expose neither price nor observation")
        return self


class ReferenceCoverageSection(BaseModel):
    rows: list[ReferenceCoverageRow] = Field(
        default_factory=list, max_length=_MAX_MATRIX_CELLS
    )


class MarketplaceResponse(BaseModel):
    """Marketplace tab. Only the sections matching the requested activity are
    populated; ``ALL`` populates every section separately and no aggregate in
    any section sums across sources (§1.4 rule 15)."""

    meta: AnalyticsMeta
    live: MarketActivitySection | None = None
    demo: MarketActivitySection | None = None
    unknown: MarketActivitySection | None = None
    reference: ReferenceCoverageSection | None = None
    commercial: CommercialSummary | None = None


# ---------------------------------------------------------------------------
# Retention (§1.6)
# ---------------------------------------------------------------------------


class CohortCell(BaseModel):
    offset: int
    cell: AggregateCell
    pct: Decimal | None = None


class CohortRow(BaseModel):
    cohort_start: date
    size: AggregateCell
    cells: list[CohortCell] = Field(default_factory=list, max_length=_MAX_COHORT_ROWS)


class RetentionKpis(BaseModel):
    returning_members: MetricValue
    retained_organizations: MetricValue
    reactivated_organizations: MetricValue
    dormant_approved_members: MetricValue


class RetentionResponse(BaseModel):
    meta: AnalyticsMeta
    kpis: RetentionKpis
    member_cohorts: list[CohortRow] = Field(default_factory=list, max_length=_MAX_COHORT_ROWS)
    organization_cohorts: list[CohortRow] = Field(
        default_factory=list, max_length=_MAX_COHORT_ROWS
    )
    repeat_participation: list[AggregateCell] = Field(default_factory=list, max_length=3)


# ---------------------------------------------------------------------------
# Reliability (§1.6)
# ---------------------------------------------------------------------------


class CollectorState(BaseModel):
    status: AnalyticsSourceStatus
    diagnostic: AnalyticsDiagnostic | None = None
    last_observation_at: datetime | None = None


class LoginFailurePanel(BaseModel):
    total: MetricValue
    categories: list[AggregateCell] = Field(default_factory=list, max_length=8)
    trend: list[SeriesPoint] = _series_field()


class FrontendErrorPanel(BaseModel):
    total: MetricValue
    by_route_family: list[AggregateCell] = Field(default_factory=list, max_length=4)
    by_category: list[AggregateCell] = Field(default_factory=list, max_length=4)


class BackendUnavailablePanel(BaseModel):
    total: MetricValue
    by_route_family: list[AggregateCell] = Field(default_factory=list, max_length=4)


class NavigationLatencyRow(BaseModel):
    destination: NavigationDestination
    buckets: list[AggregateCell] = Field(default_factory=list, max_length=5)


class AuditActivityRow(BaseModel):
    """Recent security/business audit activity without actors or IPs:
    action, resource type, and actor role only."""

    occurred_at: datetime
    action: str
    resource_type: str | None = None
    actor_role: str | None = None


class ReliabilityResponse(BaseModel):
    meta: AnalyticsMeta
    collector: CollectorState
    login_failures: LoginFailurePanel
    frontend_errors: FrontendErrorPanel
    backend_unavailable: BackendUnavailablePanel
    navigation_latency: list[NavigationLatencyRow] = Field(
        default_factory=list, max_length=12
    )
    audit_activity: list[AuditActivityRow] = _ranked_field()
