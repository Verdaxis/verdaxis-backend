"""One provenance policy for every public market projection."""
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from uuid import UUID

from sqlalchemy import and_, or_

from app.demo_identities import DEMO_MARKET_ORG_IDS
from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
from app.models.user import OrganizationProvenance
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.services.provenance import coerce_provenance

SYNTHETIC_PROVENANCE = frozenset({
    OrganizationProvenance.DEMO,
    OrganizationProvenance.TEST,
    OrganizationProvenance.CANARY,
})


class MarketEvidenceScope(str, Enum):
    """A public aggregate is computed inside exactly one evidence class."""

    REAL = "REAL"
    DEMO = "DEMO"


@dataclass(frozen=True)
class MarketEvidencePolicy:
    scope: MarketEvidenceScope
    provenance: OrganizationProvenance
    source_kind: MarketSourceKind
    demo_status: MarketDemoStatus


@dataclass(frozen=True)
class AggregateEvidenceSelection:
    """Labels and value namespace for one non-blended aggregate result."""

    scope: MarketEvidenceScope | None
    source_kind: MarketSourceKind
    demo_status: MarketDemoStatus
    real_count: int
    demo_count: int
    unknown_count: int
    value_prefix: str | None


def select_aggregate_evidence(
    *,
    real_count: int,
    demo_count: int,
    unknown_count: int,
    real_source: MarketSourceKind,
) -> AggregateEvidenceSelection:
    """Prefer REAL, then disclosed DEMO, without consuming quarantined values."""
    counts = (max(0, int(real_count)), max(0, int(demo_count)), max(0, int(unknown_count)))
    real_count, demo_count, unknown_count = counts
    if real_count:
        policy = evidence_policy_for_scope(MarketEvidenceScope.REAL, real_source=real_source)
        scope, prefix = MarketEvidenceScope.REAL, "real"
    elif demo_count:
        policy = evidence_policy_for_scope(MarketEvidenceScope.DEMO, real_source=real_source)
        scope, prefix = MarketEvidenceScope.DEMO, "demo"
    elif unknown_count:
        return AggregateEvidenceSelection(
            scope=None,
            source_kind=MarketSourceKind.UNKNOWN,
            demo_status=MarketDemoStatus.UNKNOWN,
            real_count=real_count,
            demo_count=demo_count,
            unknown_count=unknown_count,
            value_prefix=None,
        )
    else:
        return AggregateEvidenceSelection(
            scope=None,
            source_kind=MarketSourceKind.NO_DATA,
            demo_status=MarketDemoStatus.NOT_APPLICABLE,
            real_count=0,
            demo_count=0,
            unknown_count=0,
            value_prefix=None,
        )
    return AggregateEvidenceSelection(
        scope=scope,
        source_kind=policy.source_kind,
        demo_status=policy.demo_status,
        real_count=real_count,
        demo_count=demo_count,
        unknown_count=unknown_count,
        value_prefix=prefix,
    )


def evidence_policy_for_scope(
    scope: MarketEvidenceScope,
    *,
    real_source: MarketSourceKind,
) -> MarketEvidencePolicy:
    """Return labels for one already-separated evidence scope."""
    if scope == MarketEvidenceScope.REAL:
        return MarketEvidencePolicy(
            scope=scope,
            provenance=OrganizationProvenance.REAL,
            source_kind=real_source,
            demo_status=MarketDemoStatus.REAL_ONLY,
        )
    if scope == MarketEvidenceScope.DEMO:
        return MarketEvidencePolicy(
            scope=scope,
            provenance=OrganizationProvenance.DEMO,
            source_kind=MarketSourceKind.DEMO_SEED,
            demo_status=MarketDemoStatus.DEMO_ONLY,
        )
    raise ValueError("market evidence scope must be REAL or DEMO")


def order_evidence_clause(provenance_column, scope: MarketEvidenceScope):
    policy = evidence_policy_for_scope(scope, real_source=MarketSourceKind.LIVE_ORDER)
    return provenance_column == policy.provenance.value


FORMAL_TRADE_STATUSES = ("CONFIRMED", "DELIVERED", "PAID")


def organization_evidence_clause(organization, user, scope: MarketEvidenceScope):
    """Current admission plus immutable organization evidence classification."""
    policy = evidence_policy_for_scope(scope, real_source=MarketSourceKind.CONFIRMED_TRADE)
    clauses = [
        organization.verification_status == "APPROVED",
        organization.provenance == policy.provenance.value,
        user.organization_id == organization.id,
        user.status == "APPROVED",
        user.kyc_status == "APPROVED",
    ]
    if scope == MarketEvidenceScope.DEMO:
        clauses.append(organization.id.in_(tuple(DEMO_MARKET_ORG_IDS)))
    return and_(*clauses)


def canonical_trade_product_snapshot_clause(trade):
    return or_(
        *(
            and_(
                trade.product_id == spec.id,
                trade.product_name == spec.name,
                trade.fuel_type == spec.fuel_type,
                trade.fuel_grade == spec.fuel_grade,
                trade.market_product == spec.market_product.value,
            )
            for spec in CANONICAL_PRODUCTS
        )
    )


def canonical_trade_delivery_point_snapshot_clause(trade):
    return or_(
        *(
            and_(
                trade.delivery_point_id == spec.id,
                trade.delivery_point_name == spec.name,
                trade.delivery_point_region == spec.region,
            )
            for spec in CANONICAL_DELIVERY_POINTS
        )
    )


def canonical_availability_window_clause(column):
    """SQL form of the canonical v1 availability-window grammar."""
    return or_(
        column == "SPOT",
        column.regexp_match(r"^[0-9]{4}-(0[1-9]|1[0-2])$"),
        column.regexp_match(r"^[0-9]{4}-Q[1-4]$"),
        column.regexp_match(r"^[0-9]{4}-CAL$"),
    )


def formal_trade_snapshot_clause(trade):
    """Canonical immutable identity required before a trade can be evidence."""
    return and_(
        trade.market_snapshot_version == 1,
        canonical_trade_product_snapshot_clause(trade),
        canonical_trade_delivery_point_snapshot_clause(trade),
        canonical_availability_window_clause(trade.availability_window),
    )


def trade_evidence_clause(
    trade,
    scope: MarketEvidenceScope,
    *,
    confirmed_since=None,
    confirmed_before=None,
):
    """The singular SQL boundary for formal historical trade evidence."""
    policy = evidence_policy_for_scope(scope, real_source=MarketSourceKind.CONFIRMED_TRADE)
    clauses = [
        formal_trade_snapshot_clause(trade),
        trade.buyer_provenance == policy.provenance.value,
        trade.seller_provenance == policy.provenance.value,
        trade.status.in_(FORMAL_TRADE_STATUSES),
        trade.confirmed_at.is_not(None),
    ]
    if confirmed_since is not None:
        clauses.append(trade.confirmed_at >= confirmed_since)
    if confirmed_before is not None:
        clauses.append(trade.confirmed_at < confirmed_before)
    return and_(*clauses)


def public_order_evidence_clause(provenance_column):
    """Admit only separately aggregatable REAL or DEMO order evidence."""
    return or_(
        order_evidence_clause(provenance_column, MarketEvidenceScope.REAL),
        order_evidence_clause(provenance_column, MarketEvidenceScope.DEMO),
    )


def public_trade_evidence_clause(trade, *, confirmed_since=None, confirmed_before=None):
    """Admit only exact REAL/REAL or DEMO/DEMO trade snapshots."""
    return or_(
        trade_evidence_clause(
            trade,
            MarketEvidenceScope.REAL,
            confirmed_since=confirmed_since,
            confirmed_before=confirmed_before,
        ),
        trade_evidence_clause(
            trade,
            MarketEvidenceScope.DEMO,
            confirmed_since=confirmed_since,
            confirmed_before=confirmed_before,
        ),
    )


def evidence_scope_for_provenance(value: object) -> MarketEvidenceScope | None:
    provenance = coerce_provenance(value)
    if provenance == OrganizationProvenance.REAL:
        return MarketEvidenceScope.REAL
    if provenance == OrganizationProvenance.DEMO:
        return MarketEvidenceScope.DEMO
    return None


def _provenance_value(value: object) -> OrganizationProvenance:
    return coerce_provenance(value)


def order_market_provenance(order) -> dict[str, object]:
    provenance = _provenance_value(getattr(order, "provenance", None))
    is_demo = provenance == OrganizationProvenance.DEMO
    is_real = provenance == OrganizationProvenance.REAL
    delivery_point_id = getattr(order, "delivery_point_id", None)
    if not isinstance(delivery_point_id, UUID):
        delivery_point_id = None
    if is_demo:
        source = MarketSourceKind.DEMO_SEED
        demo_status = MarketDemoStatus.DEMO_ONLY
    elif is_real:
        source = MarketSourceKind.LIVE_ORDER
        demo_status = MarketDemoStatus.REAL_ONLY
    else:
        source = MarketSourceKind.UNKNOWN
        demo_status = MarketDemoStatus.UNKNOWN
    return {
        "source_kind": source.value,
        "scope": MarketScope.DELIVERY_POINT.value if delivery_point_id else MarketScope.UNKNOWN.value,
        "demo_status": demo_status.value,
        "is_demo_listing": is_demo,
        "unknown_count": 1 if provenance == OrganizationProvenance.UNKNOWN else 0,
        "market_product": getattr(order, "market_product", None),
        "delivery_point_id": str(delivery_point_id) if delivery_point_id else None,
        "availability_window": getattr(order, "availability_window", None),
    }


def trade_market_provenance(trade) -> dict[str, object]:
    buyer = _provenance_value(getattr(trade, "buyer_provenance", None))
    seller = _provenance_value(getattr(trade, "seller_provenance", None))
    if buyer == seller == OrganizationProvenance.DEMO:
        source, demo_status = MarketSourceKind.DEMO_SEED, MarketDemoStatus.DEMO_ONLY
    elif buyer == seller == OrganizationProvenance.REAL:
        source, demo_status = MarketSourceKind.CONFIRMED_TRADE, MarketDemoStatus.REAL_ONLY
    elif buyer in SYNTHETIC_PROVENANCE or seller in SYNTHETIC_PROVENANCE:
        source, demo_status = MarketSourceKind.UNKNOWN, MarketDemoStatus.UNKNOWN
    else:
        source, demo_status = MarketSourceKind.UNKNOWN, MarketDemoStatus.UNKNOWN
    delivery_point_id = getattr(trade, "delivery_point_id", None)
    if not isinstance(delivery_point_id, UUID):
        delivery_point_id = None
    return {
        "source_kind": source.value,
        "scope": MarketScope.DELIVERY_POINT.value if delivery_point_id else MarketScope.UNKNOWN.value,
        "demo_status": demo_status.value,
        "unknown_count": int(buyer == OrganizationProvenance.UNKNOWN or seller == OrganizationProvenance.UNKNOWN),
        "product_id": str(trade.product_id) if getattr(trade, "product_id", None) else None,
        "market_product": getattr(trade, "market_product", None),
        "delivery_point_id": str(delivery_point_id) if delivery_point_id else None,
        "availability_window": getattr(trade, "availability_window", None),
    }
