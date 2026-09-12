"""Canonical PostgreSQL checks for writable market records.

Writable numerics deliberately use unbounded ``NUMERIC`` columns. PostgreSQL
typmods round before a CHECK sees a value; explicit finite/range/scale checks
therefore provide the only reliable reject-instead-of-round contract.
"""

from sqlalchemy import CheckConstraint
from app.demo_identities import DEMO_MARKET_ORG_IDS
from app.market_catalog import MARKET_PRODUCT_CODES


def postgresql_check(expression: str, *, name: str) -> CheckConstraint:
    """Keep production checks in metadata without emitting them on SQLite."""
    return CheckConstraint(expression, name=name).ddl_if(dialect="postgresql")

PROVENANCE_VALUES = "'UNKNOWN','REAL','DEMO','TEST','CANARY'"
MARKET_PRODUCT_VALUES = ",".join(f"'{value}'" for value in MARKET_PRODUCT_CODES)
DEMO_ORG_VALUES = ",".join(f"'{organization_id}'" for organization_id in sorted(DEMO_MARKET_ORG_IDS, key=str))


def _required_numeric(column: str, *, minimum: str, maximum: str, scale: int) -> str:
    multiplier = 10 ** scale
    return (
        f"{column} IS NOT NULL AND {column} >= {minimum} AND {column} <= {maximum} "
        f"AND {column} * {multiplier} = trunc({column} * {multiplier})"
    )


def _optional_numeric(column: str, *, minimum: str, maximum: str, scale: int) -> str:
    return f"({column} IS NULL OR ({_required_numeric(column, minimum=minimum, maximum=maximum, scale=scale)}))"


ORGANIZATION_PROVENANCE_DOMAIN = (
    f"provenance IS NOT NULL AND provenance IN ({PROVENANCE_VALUES})"
)

ORDER_DOMAIN = (
    "side IS NOT NULL AND side IN ('BID','ASK') AND "
    "status IS NOT NULL AND status IN ('OPEN','PARTIALLY_FILLED','FILLED','CANCELLED','EXPIRED') AND "
    f"provenance IS NOT NULL AND provenance IN ({PROVENANCE_VALUES}) AND "
    "(provenance <> 'DEMO' OR status NOT IN ('OPEN','PARTIALLY_FILLED') OR expires_at IS NOT NULL) AND "
    "((idempotency_key IS NULL AND idempotency_operation IS NULL AND idempotency_request_hash IS NULL) OR "
    "(idempotency_key IS NOT NULL AND idempotency_operation IS NOT NULL AND idempotency_request_hash IS NOT NULL))"
)
ORDER_NUMERIC_VALUES = " AND ".join(
    (
        _required_numeric("quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _required_numeric("remaining_quantity_mt", minimum="0", maximum="100000.00", scale=2),
        _required_numeric("price_per_mt_usd", minimum="0.01", maximum="1000000.00", scale=2),
        _optional_numeric("carbon_intensity_gco2_mj", minimum="0", maximum="10000.00", scale=2),
        _optional_numeric("energy_density_mj_kg", minimum="0", maximum="10000.00", scale=2),
    )
)
ORDER_LIFECYCLE = (
    "remaining_quantity_mt <= quantity_mt AND ("
    "(status = 'OPEN' AND remaining_quantity_mt = quantity_mt) OR "
    "(status = 'PARTIALLY_FILLED' AND remaining_quantity_mt > 0 AND remaining_quantity_mt < quantity_mt) OR "
    "(status = 'FILLED' AND remaining_quantity_mt = 0) OR "
    "status IN ('CANCELLED','EXPIRED'))"
)

TRADE_DOMAIN = (
    "status IS NOT NULL AND status IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID','CANCELLED','DECLINED') AND "
    "initiated_by IS NOT NULL AND initiated_by IN ('BUYER','SELLER') AND "
    f"buyer_provenance IS NOT NULL AND buyer_provenance IN ({PROVENANCE_VALUES}) AND "
    f"seller_provenance IS NOT NULL AND seller_provenance IN ({PROVENANCE_VALUES}) AND "
    "buyer_id <> seller_id AND "
    "((initiated_by = 'BUYER' AND initiator_org_id = buyer_id) OR "
    "(initiated_by = 'SELLER' AND initiator_org_id = seller_id)) AND "
    "((idempotency_key IS NULL AND idempotency_operation IS NULL AND idempotency_request_hash IS NULL) OR "
    "(idempotency_key IS NOT NULL AND idempotency_operation IS NOT NULL AND idempotency_request_hash IS NOT NULL)) AND "
    "(market_snapshot_version IS NULL OR "
    "(buyer_provenance = 'REAL' AND seller_provenance = 'REAL') OR "
    "(buyer_provenance = 'DEMO' AND seller_provenance = 'DEMO' "
    f"AND buyer_id IN ({DEMO_ORG_VALUES}) AND seller_id IN ({DEMO_ORG_VALUES})))"
)
TRADE_NUMERIC_VALUES = " AND ".join(
    (
        _required_numeric("quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _required_numeric("price_per_mt_usd", minimum="0.01", maximum="1000000.00", scale=2),
        _optional_numeric("final_quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _optional_numeric("final_price_per_mt", minimum="0.01", maximum="1000000.00", scale=2),
        _optional_numeric("final_total_usd", minimum="0", maximum="100000000000.00", scale=2),
        _required_numeric("commission_rate_pct", minimum="0", maximum="100", scale=3),
        _optional_numeric("commission_fee_per_mt_usd", minimum="0", maximum="100000.00", scale=2),
        _optional_numeric("commission_amount_usd", minimum="0", maximum="100000000000.00", scale=2),
    )
)
TRADE_COMMISSION_SNAPSHOT = (
    "(commission_fee_per_mt_usd IS NULL AND commission_plan IS NULL) OR "
    "(commission_fee_per_mt_usd IS NOT NULL AND commission_rate_pct = 0 "
    "AND commission_plan IS NOT NULL "
    "AND commission_plan IN ('free','standard','enterprise'))"
)
TRADE_LIFECYCLE = (
    "(final_quantity_mt IS NULL OR final_quantity_mt <= quantity_mt) AND ("
    "(status = 'PENDING_CONFIRMATION' AND confirmed_at IS NULL AND delivered_at IS NULL AND paid_at IS NULL "
    "AND final_quantity_mt IS NULL AND final_price_per_mt IS NULL AND final_total_usd IS NULL) OR "
    "(status = 'CONFIRMED' AND confirmed_at IS NOT NULL AND delivered_at IS NULL AND paid_at IS NULL "
    "AND final_quantity_mt IS NULL AND final_price_per_mt IS NULL AND final_total_usd IS NULL) OR "
    "(status = 'DELIVERED' AND confirmed_at IS NOT NULL AND delivered_at IS NOT NULL AND paid_at IS NULL "
    "AND final_quantity_mt IS NOT NULL AND final_price_per_mt IS NOT NULL AND final_total_usd IS NOT NULL) OR "
    "(status = 'PAID' AND confirmed_at IS NOT NULL AND delivered_at IS NOT NULL AND paid_at IS NOT NULL "
    "AND final_quantity_mt IS NOT NULL AND final_price_per_mt IS NOT NULL AND final_total_usd IS NOT NULL) OR "
    "(status IN ('CANCELLED','DECLINED') AND delivered_at IS NULL AND paid_at IS NULL "
    "AND final_quantity_mt IS NULL AND final_price_per_mt IS NULL AND final_total_usd IS NULL))"
)
TRADE_SNAPSHOT = (
    "(market_snapshot_version IS NULL OR (market_snapshot_version = 1 "
    "AND product_id IS NOT NULL AND product_name IS NOT NULL AND trim(product_name) <> '' "
    "AND fuel_type IS NOT NULL AND trim(fuel_type) <> '' "
    "AND fuel_grade IS NOT NULL AND trim(fuel_grade) <> '' "
    f"AND market_product IN ({MARKET_PRODUCT_VALUES}) "
    "AND availability_window IS NOT NULL AND (availability_window = 'SPOT' "
    "OR availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$' "
    "OR availability_window ~ '^[0-9]{4}-Q[1-4]$' "
    "OR availability_window ~ '^[0-9]{4}-CAL$') "
    "AND delivery_point_id IS NOT NULL AND delivery_point_name IS NOT NULL "
    "AND delivery_point_region IS NOT NULL))"
)

INVENTORY_NUMERIC_VALUES = " AND ".join(
    (
        _required_numeric("current_stock_mt", minimum="0", maximum="100000.00", scale=2),
        _required_numeric("incoming_stock_mt", minimum="0", maximum="100000.00", scale=2),
        _required_numeric("reserved_stock_mt", minimum="0", maximum="100000.00", scale=2),
        _optional_numeric("price_per_mt_usd", minimum="0.01", maximum="1000000.00", scale=2),
        _optional_numeric("energy_density_mj_kg", minimum="0", maximum="10000.00", scale=2),
        _optional_numeric("carbon_intensity_gco2_mj", minimum="0", maximum="10000.00", scale=2),
    )
)

RFQ_DOMAIN = "status IS NOT NULL AND status IN ('OPEN','QUOTED','ACCEPTED','EXPIRED','CANCELLED')"
RFQ_NUMERIC_VALUES = " AND ".join(
    (
        _required_numeric("quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _optional_numeric("target_price_per_mt", minimum="0.01", maximum="1000000.00", scale=2),
    )
)
RFQ_LIFECYCLE = (
    "((status = 'ACCEPTED' AND accepted_quote_id IS NOT NULL AND trade_id IS NOT NULL) OR "
    "(status <> 'ACCEPTED' AND accepted_quote_id IS NULL AND trade_id IS NULL))"
)
RFQ_QUOTE_DOMAIN = "status IS NOT NULL AND status IN ('PENDING','ACCEPTED','DECLINED','WITHDRAWN')"
RFQ_QUOTE_NUMERIC_VALUES = _required_numeric(
    "price_per_mt_usd", minimum="0.01", maximum="1000000.00", scale=2
)

NEGOTIATION_DOMAIN = (
    "status IS NOT NULL AND status IN ('OPEN','COUNTERED','AGREED','DECLINED','EXPIRED') AND "
    "initiator_side IS NOT NULL AND initiator_side IN ('BUYER','SELLER') AND "
    "initiator_org_id IS NOT NULL AND counterparty_org_id IS NOT NULL AND "
    "initiator_org_id <> counterparty_org_id"
)
NEGOTIATION_NUMERIC_VALUES = " AND ".join(
    (
        _required_numeric("quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _required_numeric("current_price", minimum="0.01", maximum="1000000.00", scale=2),
    )
)
NEGOTIATION_LIFECYCLE = (
    "((status = 'AGREED' AND trade_id IS NOT NULL) OR "
    "(status <> 'AGREED' AND trade_id IS NULL))"
)
NEGOTIATION_ROUND_NUMERIC_VALUES = _required_numeric(
    "proposed_price", minimum="0.01", maximum="1000000.00", scale=2
)
