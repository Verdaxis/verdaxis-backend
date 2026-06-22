"""
Pydantic schemas for the forward curve API.
"""
from datetime import datetime, timezone
from decimal import Decimal
from enum import Enum
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, Field

from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind


class MarketSignalType(str, Enum):
    """Signal families displayed in the Forward Curve monitoring workspace."""

    CONFIRMED_TRADE = "CONFIRMED_TRADE"
    ORDERBOOK_BID = "ORDERBOOK_BID"
    ORDERBOOK_ASK = "ORDERBOOK_ASK"
    BENCHMARK_MID = "BENCHMARK_MID"
    MARKET_INDICATION = "MARKET_INDICATION"
    FAIR_PRICE_BAND = "FAIR_PRICE_BAND"
    PHYSICAL_STEM = "PHYSICAL_STEM"
    NO_DATA = "NO_DATA"


class ForwardCurveSignalSourceKind(str, Enum):
    """Source categories for read-only Forward Curve monitoring signals."""

    MARKET_INDICATION = "MARKET_INDICATION"
    PHYSICAL_STEM = "PHYSICAL_STEM"
    FAIR_PRICE_MODEL = "FAIR_PRICE_MODEL"
    DEMO_SEED = "DEMO_SEED"
    MIXED_SOURCE = "MIXED_SOURCE"
    NO_DATA = "NO_DATA"
    UNKNOWN = "UNKNOWN"


class ForwardCurveIndicationSide(str, Enum):
    BID = "BID"
    ASK = "ASK"
    MID = "MID"


class ForwardCurvePhysicalStemStatus(str, Enum):
    AVAILABLE = "AVAILABLE"
    TENTATIVE = "TENTATIVE"
    ALLOCATED = "ALLOCATED"
    CANCELLED = "CANCELLED"


class ForwardCurveStalenessStatus(str, Enum):
    FRESH = "FRESH"
    STALE = "STALE"
    NO_DATA = "NO_DATA"


class ForwardCurveEvidenceLayer(str, Enum):
    HISTORICAL_TRADE = "HISTORICAL_TRADE"
    ORDERBOOK_BID = "ORDERBOOK_BID"
    ORDERBOOK_ASK = "ORDERBOOK_ASK"
    MARKET_INDICATION = "MARKET_INDICATION"
    FAIR_PRICE_BAND = "FAIR_PRICE_BAND"
    BENCHMARK_MID = "BENCHMARK_MID"
    PHYSICAL_STEM = "PHYSICAL_STEM"


class ForwardCurveLabelPolicy(BaseModel):
    """Server-owned wording policy for public monitoring labels."""

    public_label: str
    tooltip: Optional[str] = None
    allowed_terms: list[str] = Field(default_factory=list)
    forbidden_terms: list[str] = Field(default_factory=list)
    disclaimer: Optional[str] = None


def no_data_label_policy() -> ForwardCurveLabelPolicy:
    return ForwardCurveLabelPolicy(
        public_label="No data",
        tooltip="No eligible public signal is available for this market slice.",
        allowed_terms=["no data"],
        forbidden_terms=["live", "firm", "executable", "market price", "clearing"],
    )


class ForwardCurveSignalProvenance(BaseModel):
    """Sanitized provenance for public read-only monitoring signals."""

    signal_type: MarketSignalType
    signal_source_kind: ForwardCurveSignalSourceKind = ForwardCurveSignalSourceKind.UNKNOWN
    scope: MarketScope = MarketScope.DELIVERY_POINT
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN
    observed_at: Optional[datetime] = None
    generated_at: datetime
    real_count: int = 0
    demo_count: int = 0
    unknown_count: int = 0


def no_data_signal_provenance(signal_type: MarketSignalType) -> ForwardCurveSignalProvenance:
    """Build a no-data provenance placeholder for a public signal family."""

    return ForwardCurveSignalProvenance(
        signal_type=signal_type,
        signal_source_kind=ForwardCurveSignalSourceKind.NO_DATA,
        scope=MarketScope.DELIVERY_POINT,
        demo_status=MarketDemoStatus.NOT_APPLICABLE,
        observed_at=None,
        generated_at=datetime.now(timezone.utc),
    )


class ForwardCurveBoardIndication(BaseModel):
    """Non-executable market indication for one canonical market slice."""

    side: ForwardCurveIndicationSide
    price_per_mt_usd: Decimal
    quantity_mt: Optional[Decimal] = None
    provenance: ForwardCurveSignalProvenance

    model_config = {"from_attributes": True}


class ForwardCurveBoardIndicationSummary(BaseModel):
    """Board-level summary of market indications for one canonical slice."""

    provenance: ForwardCurveSignalProvenance = Field(
        default_factory=lambda: no_data_signal_provenance(MarketSignalType.MARKET_INDICATION)
    )
    latest_bid_price_per_mt_usd: Optional[Decimal] = None
    latest_ask_price_per_mt_usd: Optional[Decimal] = None
    latest_mid_price_per_mt_usd: Optional[Decimal] = None
    total_quantity_mt: Optional[Decimal] = None
    indication_count: int = 0


class ForwardCurveBoardFairPriceBand(BaseModel):
    """Verdaxis fair-price band for one canonical market slice."""

    low_price_per_mt_usd: Decimal
    mid_price_per_mt_usd: Decimal
    high_price_per_mt_usd: Decimal
    provenance: ForwardCurveSignalProvenance

    model_config = {"from_attributes": True}


class ForwardCurveBoardPhysicalStem(BaseModel):
    """Physical availability signal for one canonical market slice."""

    quantity_mt: Decimal
    status: ForwardCurvePhysicalStemStatus
    stem_start: Optional[datetime] = None
    stem_end: Optional[datetime] = None
    provenance: ForwardCurveSignalProvenance

    model_config = {"from_attributes": True}


class ForwardCurveBoardPhysicalStemSummary(BaseModel):
    """Board-level physical stem summary for one canonical slice."""

    provenance: ForwardCurveSignalProvenance = Field(
        default_factory=lambda: no_data_signal_provenance(MarketSignalType.PHYSICAL_STEM)
    )
    available_quantity_mt: Optional[Decimal] = None
    tentative_quantity_mt: Optional[Decimal] = None
    stem_count: int = 0
    earliest_stem_start: Optional[datetime] = None
    latest_stem_end: Optional[datetime] = None


class ForwardCurvePoint(BaseModel):
    """
    A single point on the forward curve for one availability_window.

    best_bid = highest open BID price for this window
    best_ask = lowest open ASK price for this window
    mid_price = (best_bid + best_ask) / 2, None if only one side exists
    spread = best_ask - best_bid, None if only one side exists
    volume_mt = total remaining_quantity_mt across all OPEN/PARTIALLY_FILLED orders in this window
    order_count = total number of orders across both sides
    """

    availability_window: str
    best_bid: Optional[Decimal] = None
    best_ask: Optional[Decimal] = None
    mid_price: Optional[Decimal] = None
    spread: Optional[Decimal] = None
    volume_mt: Decimal
    order_count: int

    model_config = {"from_attributes": True}


class ForwardCurveResponse(BaseModel):
    """Response wrapper for the forward curve endpoint."""

    product_id: UUID
    delivery_point_id: Optional[UUID] = None
    curve: list[ForwardCurvePoint]
    generated_at: datetime


class ForwardCurveBoardProduct(BaseModel):
    """Product identity used by the forward-curve monitoring board."""

    product_id: UUID
    market_product: str
    product_name: str


class ForwardCurveBoardDepthLevel(BaseModel):
    """Aggregated visible depth for one side of the focused market slice."""

    price_per_mt_usd: Decimal
    quantity_mt: Decimal
    order_count: int
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN
    real_order_count: int = 0
    demo_order_count: int = 0
    unknown_order_count: int = 0


class ForwardCurveBoardCell(BaseModel):
    """One product-port cell in the monitoring board matrix."""

    product_id: UUID
    market_product: str
    product_name: str
    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    availability_window: str
    benchmark_mid: Optional[Decimal] = None
    benchmark_source: Optional[str] = None
    is_demo_benchmark: bool = False
    order_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    benchmark_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    scope: MarketScope = MarketScope.DELIVERY_POINT
    demo_status: MarketDemoStatus = MarketDemoStatus.NOT_APPLICABLE
    real_order_count: int = 0
    demo_order_count: int = 0
    unknown_order_count: int = 0
    real_best_bid: Optional[Decimal] = None
    real_best_ask: Optional[Decimal] = None
    demo_best_bid: Optional[Decimal] = None
    demo_best_ask: Optional[Decimal] = None
    best_bid_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    best_ask_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    order_observed_at: Optional[datetime] = None
    benchmark_observed_at: Optional[datetime] = None
    indication_summary: ForwardCurveBoardIndicationSummary = Field(default_factory=ForwardCurveBoardIndicationSummary)
    fair_price_band: Optional[ForwardCurveBoardFairPriceBand] = None
    fair_price_band_provenance: ForwardCurveSignalProvenance = Field(
        default_factory=lambda: no_data_signal_provenance(MarketSignalType.FAIR_PRICE_BAND)
    )
    physical_stem_summary: ForwardCurveBoardPhysicalStemSummary = Field(default_factory=ForwardCurveBoardPhysicalStemSummary)
    best_bid: Optional[Decimal] = None
    best_ask: Optional[Decimal] = None
    spread: Optional[Decimal] = None
    volume_mt: Decimal = Decimal("0")
    order_count: int = 0


class ForwardCurveBoardPort(BaseModel):
    """Port row in the monitoring board matrix."""

    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    cells: list[ForwardCurveBoardCell]


class ForwardCurveBoardFocus(BaseModel):
    """Focused detail context selected from the board matrix."""

    product_id: UUID
    market_product: str
    product_name: str
    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    availability_window: str
    curve: list[ForwardCurveBoardCell]
    depth_bids: list[ForwardCurveBoardDepthLevel]
    depth_asks: list[ForwardCurveBoardDepthLevel]
    indications: list[ForwardCurveBoardIndication] = Field(default_factory=list)
    fair_price_band: Optional[ForwardCurveBoardFairPriceBand] = None
    fair_price_band_provenance: ForwardCurveSignalProvenance = Field(
        default_factory=lambda: no_data_signal_provenance(MarketSignalType.FAIR_PRICE_BAND)
    )
    physical_stems: list[ForwardCurveBoardPhysicalStem] = Field(default_factory=list)


class ForwardCurveBoardResponse(BaseModel):
    """Aggregated board for the Forward Curve monitoring workspace."""

    availability_window: str
    products: list[ForwardCurveBoardProduct]
    ports: list[ForwardCurveBoardPort]
    focus: ForwardCurveBoardFocus
    generated_at: datetime


class ForwardCurveTableColumn(BaseModel):
    """Window column metadata for the monitoring matrix."""

    availability_window: str
    display_label: str
    group: str


class ForwardCurveMarketCell(BaseModel):
    """Public canonical market-slice cell for the monitoring table."""

    market_product: str
    product_name: str
    representative_product_id: UUID
    product_count: int = 1
    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    availability_window: str
    primary_value: Optional[Decimal] = None
    primary_signal_type: MarketSignalType = MarketSignalType.NO_DATA
    primary_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    public_source_label: str = "No data"
    label_policy: ForwardCurveLabelPolicy = Field(default_factory=no_data_label_policy)
    staleness_status: ForwardCurveStalenessStatus = ForwardCurveStalenessStatus.NO_DATA
    is_executable: bool = False
    is_reference: bool = False
    demo_status: MarketDemoStatus = MarketDemoStatus.NOT_APPLICABLE
    scope: MarketScope = MarketScope.DELIVERY_POINT
    observed_at: Optional[datetime] = None
    generated_at: datetime
    best_bid: Optional[Decimal] = None
    best_ask: Optional[Decimal] = None
    spread: Optional[Decimal] = None
    volume_mt: Decimal = Decimal("0")
    order_count: int = 0
    real_order_count: int = 0
    demo_order_count: int = 0
    unknown_order_count: int = 0
    real_best_bid: Optional[Decimal] = None
    real_best_ask: Optional[Decimal] = None
    demo_best_bid: Optional[Decimal] = None
    demo_best_ask: Optional[Decimal] = None
    benchmark_mid: Optional[Decimal] = None
    benchmark_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    benchmark_observed_at: Optional[datetime] = None
    indication_summary: ForwardCurveBoardIndicationSummary = Field(default_factory=ForwardCurveBoardIndicationSummary)
    fair_price_band: Optional[ForwardCurveBoardFairPriceBand] = None
    fair_price_band_provenance: ForwardCurveSignalProvenance = Field(
        default_factory=lambda: no_data_signal_provenance(MarketSignalType.FAIR_PRICE_BAND)
    )
    physical_stem_summary: ForwardCurveBoardPhysicalStemSummary = Field(default_factory=ForwardCurveBoardPhysicalStemSummary)


class ForwardCurveTableRow(BaseModel):
    """One product-port row keyed by canonical monitoring windows."""

    row_key: str
    market_product: str
    product_name: str
    representative_product_id: UUID
    product_count: int = 1
    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    cells: dict[str, ForwardCurveMarketCell]


class ForwardCurveLatestSignal(BaseModel):
    """Compact strip item for the latest monitored signal feed."""

    market_product: str
    delivery_point_id: UUID
    delivery_point_name: str
    availability_window: str
    primary_value: Optional[Decimal] = None
    primary_signal_type: MarketSignalType = MarketSignalType.NO_DATA
    primary_source_kind: MarketSourceKind = MarketSourceKind.NO_DATA
    public_source_label: str = "No data"
    demo_status: MarketDemoStatus = MarketDemoStatus.NOT_APPLICABLE
    observed_at: Optional[datetime] = None
    staleness_status: ForwardCurveStalenessStatus = ForwardCurveStalenessStatus.NO_DATA


class ForwardCurveTableResponse(BaseModel):
    """Canonical monitoring matrix for the Forward Curve workspace."""

    columns: list[ForwardCurveTableColumn]
    rows: list[ForwardCurveTableRow]
    latest_signals: list[ForwardCurveLatestSignal] = Field(default_factory=list)
    generated_at: datetime
    disclaimer: str = "Indicative estimate only. Not legal, regulatory, tax, or compliance filing advice."


class ForwardCurveSliceTrade(BaseModel):
    """Anonymized confirmed trade print for a selected monitoring slice."""

    price_per_mt_usd: Decimal
    quantity_mt: Decimal
    confirmed_at: datetime
    source_kind: MarketSourceKind
    demo_status: MarketDemoStatus


class ForwardCurveSliceEvidencePoint(BaseModel):
    """Graph-ready evidence point for a selected product-port-window period."""

    layer: ForwardCurveEvidenceLayer
    side: Optional[ForwardCurveIndicationSide] = None
    price_per_mt_usd: Optional[Decimal] = None
    low_price_per_mt_usd: Optional[Decimal] = None
    high_price_per_mt_usd: Optional[Decimal] = None
    quantity_mt: Optional[Decimal] = None
    observed_at: Optional[datetime] = None
    public_source_label: str
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN


class ForwardCurveSliceResponse(BaseModel):
    """Exact selected slice drilldown for the Forward Curve monitoring page."""

    cell: ForwardCurveMarketCell
    previous_window: Optional[str] = None
    next_window: Optional[str] = None
    depth_bids: list[ForwardCurveBoardDepthLevel] = Field(default_factory=list)
    depth_asks: list[ForwardCurveBoardDepthLevel] = Field(default_factory=list)
    trades: list[ForwardCurveSliceTrade] = Field(default_factory=list)
    indications: list[ForwardCurveBoardIndication] = Field(default_factory=list)
    fair_price_band: Optional[ForwardCurveBoardFairPriceBand] = None
    physical_stems: list[ForwardCurveBoardPhysicalStem] = Field(default_factory=list)
    evidence_points: list[ForwardCurveSliceEvidencePoint] = Field(default_factory=list)
    generated_at: datetime
    disclaimer: str = "Indicative estimate only. Not legal, regulatory, tax, or compliance filing advice."
