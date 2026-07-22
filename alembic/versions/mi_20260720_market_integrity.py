"""Harden market provenance, lifecycle, numeric, and snapshot integrity.

Revision ID: mi_20260720_market_integrity
Revises: miq_20260720_market_quarantine

This migration is deliberately deterministic from its parent schema.  It does
not inspect-and-skip divergent histories and it never removes economic rows.
The known zero-value sentinel must first be moved by the explicit quarantine
tool after upgrading only as far as the parent quarantine revision.

Integration note: the security and market heads must be linearized in the
final integration branch.  This standalone branch references only revisions
that are present here; no security revision is assumed.

All expressions are immutable migration-local literals.  Importing this file
does not import application configuration or require secrets.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "mi_20260720_market_integrity"
down_revision = "miq_20260720_market_quarantine"
branch_labels = None
depends_on = None

_SENTINEL = "00000000-dead-beef-0000-aaa0e15eed01"
_DEMO_IDS = (
    "4da7b285-34ee-5443-9406-f96b4ed1a251",
    "0dbce576-2026-5925-ab66-674d505e98ad",
    "3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a",
    "277491df-cb0d-5f2d-a2cf-5746829c6da6",
    "3b302066-d65c-5c3e-8fcc-70b3da3bcafd",
    "79609f48-0a3e-560e-a1e1-63d90601d84a",
    "2c4e387e-de22-5adb-ad88-9274ba84ebe1",
    "612953c7-567a-58b3-bc42-ee817d2bbe74",
    "93ccda09-54b3-53ee-afc0-759d3048161f",
    "82426590-0963-5486-9b05-f81e97afe6ef",
    "acc3f20a-fe94-4463-9029-a55e35634eb7",
    "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
    "7cc77115-0a9f-4ec4-8c74-05aa10050111",
    "d1e43e55-3fb0-4b5e-9f0b-93aa10050222",
)
_TEST_IDS = (
    "9e63f7a1-0000-4000-8000-000000000001",
    "9e63f7a1-0000-4000-8000-000000000002",
    "9e63f7a1-0000-4000-8000-000000000003",
    "9e63f7a1-0000-4000-8000-000000000004",
    "9e63f7a1-0000-4000-8000-000000000011",
    "9e63f7a1-0000-4000-8000-000000000012",
    "9e63f7a1-0000-4000-8000-000000000013",
    "9e63f7a1-0000-4000-8000-000000000014",
)

_PROVENANCES = "'UNKNOWN','REAL','DEMO','TEST','CANARY'"
_MARKET_PRODUCTS = "'BIO_METHANOL','E_METHANOL','BIO_ETHANOL','SYNTHETIC_ETHANOL'"
_PRODUCT_IDENTITIES = (
    ("9510c713-6e39-5080-add3-0a7c29657b79", "Bio Methanol", "Methanol", "Bio", "BIO_METHANOL"),
    ("f9b20492-b445-59cd-b292-a386d913f488", "e-Methanol", "Methanol", "E", "E_METHANOL"),
    ("c4a688be-f7c2-5edc-8f93-6b34e387609c", "Bio Ethanol", "Ethanol", "Bio", "BIO_ETHANOL"),
    ("d186bffb-766d-5944-8825-989abbdcfc46", "Synthetic Ethanol", "Ethanol", "Synthetic", "SYNTHETIC_ETHANOL"),
)
_DELIVERY_POINT_IDENTITIES = (
    ("78281fb3-e726-5396-802e-34b01fda21e8", "Dalian", "Asia"),
    ("262d36ae-6f35-5785-b9e8-9e221b0f1b78", "Busan", "Asia"),
    ("633c0593-f9b9-52ad-abe5-a0285f6888d3", "Shanghai", "Asia"),
    ("73835e92-820e-584b-8280-bb61c63aa28e", "Singapore", "Asia"),
    ("1379d36c-1ca9-55b7-9c0d-5235a0ba1f36", "Rotterdam", "Europe"),
    ("a083db06-b050-56c2-a274-3897eac2fdae", "Houston", "Americas"),
    ("1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534", "Los Angeles", "Americas"),
    ("a4762444-4e0b-5bc1-b647-a185047adae5", "Santos", "Americas"),
)


def _quoted(values: tuple[str, ...]) -> str:
    return ",".join(f"'{value}'" for value in values)


def _literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _product_snapshot_predicate(alias: str) -> str:
    return "(" + " OR ".join(
        "(" + " AND ".join(
            (
                f"{alias}.product_id = {_literal(product_id)}",
                f"{alias}.product_name = {_literal(name)}",
                f"{alias}.fuel_type = {_literal(fuel_type)}",
                f"{alias}.fuel_grade = {_literal(fuel_grade)}",
                f"{alias}.market_product = {_literal(market_product)}",
            )
        ) + ")"
        for product_id, name, fuel_type, fuel_grade, market_product in _PRODUCT_IDENTITIES
    ) + ")"


def _delivery_snapshot_predicate(alias: str) -> str:
    return "(" + " OR ".join(
        "(" + " AND ".join(
            (
                f"{alias}.delivery_point_id = {_literal(point_id)}",
                f"{alias}.delivery_point_name = {_literal(name)}",
                f"{alias}.delivery_point_region = {_literal(region)}",
            )
        ) + ")"
        for point_id, name, region in _DELIVERY_POINT_IDENTITIES
    ) + ")"


def _product_row_case(alias: str) -> str:
    branches = " ".join(
        "WHEN " + " AND ".join(
            (
                f"{alias}.id = {_literal(product_id)}",
                f"{alias}.name = {_literal(name)}",
                f"{alias}.fuel_type = {_literal(fuel_type)}",
                f"{alias}.fuel_grade = {_literal(fuel_grade)}",
            )
        ) + f" THEN {_literal(market_product)}"
        for product_id, name, fuel_type, fuel_grade, market_product in _PRODUCT_IDENTITIES
    )
    return f"CASE {branches} ELSE NULL END"


def _delivery_row_predicate(alias: str) -> str:
    return "(" + " OR ".join(
        "(" + " AND ".join(
            (
                f"{alias}.id = {_literal(point_id)}",
                f"{alias}.name = {_literal(name)}",
                f"{alias}.region = {_literal(region)}",
            )
        ) + ")"
        for point_id, name, region in _DELIVERY_POINT_IDENTITIES
    ) + ")"


def _required_numeric(column: str, *, minimum: str, maximum: str, scale: int) -> str:
    multiplier = 10 ** scale
    return (
        f"{column} IS NOT NULL AND {column} >= {minimum} AND {column} <= {maximum} "
        f"AND {column} * {multiplier} = trunc({column} * {multiplier})"
    )


def _optional_numeric(column: str, *, minimum: str, maximum: str, scale: int) -> str:
    required = _required_numeric(column, minimum=minimum, maximum=maximum, scale=scale)
    return f"({column} IS NULL OR ({required}))"


ORGANIZATION_PROVENANCE_DOMAIN = (
    f"provenance IS NOT NULL AND provenance IN ({_PROVENANCES})"
)
ORDER_DOMAIN = (
    "side IS NOT NULL AND side IN ('BID','ASK') AND "
    "status IS NOT NULL AND status IN ('OPEN','PARTIALLY_FILLED','FILLED','CANCELLED','EXPIRED') AND "
    f"provenance IS NOT NULL AND provenance IN ({_PROVENANCES}) AND "
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
    f"buyer_provenance IS NOT NULL AND buyer_provenance IN ({_PROVENANCES}) AND "
    f"seller_provenance IS NOT NULL AND seller_provenance IN ({_PROVENANCES}) AND "
    "buyer_id <> seller_id AND "
    "((initiated_by = 'BUYER' AND initiator_org_id = buyer_id) OR "
    "(initiated_by = 'SELLER' AND initiator_org_id = seller_id)) AND "
    "((idempotency_key IS NULL AND idempotency_operation IS NULL AND idempotency_request_hash IS NULL) OR "
    "(idempotency_key IS NOT NULL AND idempotency_operation IS NOT NULL AND idempotency_request_hash IS NOT NULL)) AND "
    "(market_snapshot_version IS NULL OR "
    "(buyer_provenance = 'REAL' AND seller_provenance = 'REAL') OR "
    "(buyer_provenance = 'DEMO' AND seller_provenance = 'DEMO' "
    f"AND buyer_id IN ({_quoted(_DEMO_IDS)}) AND seller_id IN ({_quoted(_DEMO_IDS)})))"
)
TRADE_NUMERIC_VALUES = " AND ".join(
    (
        _required_numeric("quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _required_numeric("price_per_mt_usd", minimum="0.01", maximum="1000000.00", scale=2),
        _optional_numeric("final_quantity_mt", minimum="0.01", maximum="100000.00", scale=2),
        _optional_numeric("final_price_per_mt", minimum="0.01", maximum="1000000.00", scale=2),
        _optional_numeric("final_total_usd", minimum="0", maximum="100000000000.00", scale=2),
        _required_numeric("commission_rate_pct", minimum="0", maximum="100", scale=3),
        _optional_numeric("commission_amount_usd", minimum="0", maximum="100000000000.00", scale=2),
    )
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
    f"AND market_product IN ({_MARKET_PRODUCTS}) "
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
    "((status = 'AGREED' AND trade_id IS NOT NULL) OR (status <> 'AGREED' AND trade_id IS NULL))"
)
NEGOTIATION_ROUND_NUMERIC_VALUES = _required_numeric(
    "proposed_price", minimum="0.01", maximum="1000000.00", scale=2
)


def _preflight_sentinel() -> None:
    """Fail before any migration mutation when the exact sentinel remains."""
    op.execute(
        sa.text(
            f"""
            DO $$
            DECLARE
                order_count bigint;
                trade_count bigint;
                commission_count bigint;
                suggestion_count bigint;
                target_count bigint;
                negotiation_count bigint;
            BEGIN
                SELECT count(*) INTO order_count FROM orderbook_orders WHERE id = '{_SENTINEL}';
                SELECT count(*) INTO trade_count FROM trades
                  WHERE bid_order_id = '{_SENTINEL}' OR ask_order_id = '{_SENTINEL}';
                SELECT count(*) INTO commission_count FROM commissions WHERE trade_id IN (
                    SELECT id FROM trades
                    WHERE bid_order_id = '{_SENTINEL}' OR ask_order_id = '{_SENTINEL}'
                );
                SELECT count(*) INTO suggestion_count FROM match_suggestions
                  WHERE bid_order_id = '{_SENTINEL}' OR ask_order_id = '{_SENTINEL}';
                SELECT count(*) INTO target_count FROM watchlist_targets WHERE order_id = '{_SENTINEL}';
                SELECT count(*) INTO negotiation_count FROM negotiations
                  WHERE bid_order_id = '{_SENTINEL}' OR ask_order_id = '{_SENTINEL}';
                IF order_count + trade_count + commission_count + suggestion_count
                   + target_count + negotiation_count > 0 THEN
                    RAISE EXCEPTION
                      'market-integrity preflight: sentinel {_SENTINEL} requires explicit-ID quarantine; orders=%, trades=%, commissions=%, match_suggestions=%, watchlist_targets=%, negotiations=%',
                      order_count, trade_count, commission_count, suggestion_count,
                      target_count, negotiation_count;
                END IF;
            END $$;
            """
        )
    )


def _preflight() -> None:
    """Reject incompatible economics before any market-owned mutation."""
    op.execute(
        sa.text(
            f"""
            DO $$
            DECLARE bad_ids text;
            BEGIN
                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT id FROM orderbook_orders
                    WHERE side IS NULL OR side NOT IN ('BID','ASK')
                       OR status IS NULL OR status NOT IN ('OPEN','PARTIALLY_FILLED','FILLED','CANCELLED','EXPIRED')
                       OR availability_window IS NULL OR length(availability_window) > 16
                       OR quantity_mt IS NULL OR quantity_mt < 0.01 OR quantity_mt > 100000
                       OR remaining_quantity_mt IS NULL OR remaining_quantity_mt < 0
                       OR remaining_quantity_mt > quantity_mt OR remaining_quantity_mt > 100000
                       OR price_per_mt_usd IS NULL OR price_per_mt_usd < 0.01 OR price_per_mt_usd > 1000000
                       OR carbon_intensity_gco2_mj < 0 OR carbon_intensity_gco2_mj > 10000
                       OR energy_density_mj_kg < 0 OR energy_density_mj_kg > 10000
                       OR (organization_id IN ({_quoted(_DEMO_IDS)})
                           AND status IN ('OPEN','PARTIALLY_FILLED') AND expires_at IS NULL)
                       OR NOT (
                            (status = 'OPEN' AND remaining_quantity_mt = quantity_mt)
                         OR (status = 'PARTIALLY_FILLED' AND remaining_quantity_mt > 0 AND remaining_quantity_mt < quantity_mt)
                         OR (status = 'FILLED' AND remaining_quantity_mt = 0)
                         OR status IN ('CANCELLED','EXPIRED')
                       )
                    ORDER BY id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid order rows (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT id FROM trades
                    WHERE buyer_id = seller_id
                       OR initiated_by IS NULL OR initiated_by NOT IN ('BUYER','SELLER')
                       OR status IS NULL OR status NOT IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID','CANCELLED','DECLINED')
                       OR quantity_mt IS NULL OR quantity_mt < 0.01 OR quantity_mt > 100000
                       OR price_per_mt_usd IS NULL OR price_per_mt_usd < 0.01 OR price_per_mt_usd > 1000000
                       OR commission_rate_pct IS NULL OR commission_rate_pct < 0 OR commission_rate_pct > 100
                       OR final_quantity_mt < 0.01 OR final_quantity_mt > quantity_mt OR final_quantity_mt > 100000
                       OR final_price_per_mt < 0.01 OR final_price_per_mt > 1000000
                       OR final_total_usd < 0 OR final_total_usd > 100000000000
                       OR commission_amount_usd < 0 OR commission_amount_usd > 100000000000
                       OR NOT (
                            (status = 'PENDING_CONFIRMATION' AND confirmed_at IS NULL AND delivered_at IS NULL AND paid_at IS NULL
                             AND final_quantity_mt IS NULL AND final_price_per_mt IS NULL AND final_total_usd IS NULL)
                         OR (status = 'CONFIRMED' AND confirmed_at IS NOT NULL AND delivered_at IS NULL AND paid_at IS NULL
                             AND final_quantity_mt IS NULL AND final_price_per_mt IS NULL AND final_total_usd IS NULL)
                         OR (status = 'DELIVERED' AND confirmed_at IS NOT NULL AND delivered_at IS NOT NULL AND paid_at IS NULL
                             AND final_quantity_mt IS NOT NULL AND final_price_per_mt IS NOT NULL AND final_total_usd IS NOT NULL)
                         OR (status = 'PAID' AND confirmed_at IS NOT NULL AND delivered_at IS NOT NULL AND paid_at IS NOT NULL
                             AND final_quantity_mt IS NOT NULL AND final_price_per_mt IS NOT NULL AND final_total_usd IS NOT NULL)
                         OR (status IN ('CANCELLED','DECLINED') AND delivered_at IS NULL AND paid_at IS NULL
                             AND final_quantity_mt IS NULL AND final_price_per_mt IS NULL AND final_total_usd IS NULL)
                       )
                    ORDER BY id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid trade rows (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT t.id FROM trades t
                    JOIN orderbook_orders bid ON bid.id = t.bid_order_id
                    JOIN orderbook_orders ask ON ask.id = t.ask_order_id
                    WHERE bid.product_id IS DISTINCT FROM ask.product_id
                       OR bid.delivery_point_id IS DISTINCT FROM ask.delivery_point_id
                       OR bid.availability_window IS DISTINCT FROM ask.availability_window
                    ORDER BY t.id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: trades reference mismatched order slices (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT id FROM inventory_items
                    WHERE current_stock_mt IS NULL OR current_stock_mt < 0 OR current_stock_mt > 100000
                       OR incoming_stock_mt IS NULL OR incoming_stock_mt < 0 OR incoming_stock_mt > 100000
                       OR reserved_stock_mt IS NULL OR reserved_stock_mt < 0 OR reserved_stock_mt > 100000
                       OR price_per_mt_usd <= 0 OR price_per_mt_usd > 1000000
                       OR energy_density_mj_kg < 0 OR energy_density_mj_kg > 10000
                       OR carbon_intensity_gco2_mj < 0 OR carbon_intensity_gco2_mj > 10000
                    ORDER BY id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid inventory rows (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT id FROM rfqs
                    WHERE status IS NULL OR status NOT IN ('OPEN','QUOTED','ACCEPTED','EXPIRED','CANCELLED')
                       OR status = 'ACCEPTED'
                       OR quantity_mt IS NULL OR quantity_mt < 0.01 OR quantity_mt > 100000
                       OR target_price_per_mt <= 0 OR target_price_per_mt > 1000000
                       OR length(availability_window) > 16
                    ORDER BY id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid or already-accepted RFQs require explicit remediation (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT q.id FROM rfq_quotes q
                    JOIN rfqs r ON r.id = q.rfq_id
                    WHERE q.status IS NULL OR q.status NOT IN ('PENDING','ACCEPTED','DECLINED','WITHDRAWN')
                       OR q.price_per_mt_usd IS NULL OR q.price_per_mt_usd < 0.01 OR q.price_per_mt_usd > 1000000
                       OR q.seller_org_id = r.buyer_org_id
                    ORDER BY q.id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid RFQ quotes (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT min(id::text) AS id FROM rfq_quotes
                    GROUP BY rfq_id, seller_org_id
                    HAVING count(*) > 1
                    ORDER BY min(id::text) LIMIT 20
                ) duplicates;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: duplicate RFQ seller quotes (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT id FROM negotiations
                    WHERE status IS NULL OR status NOT IN ('OPEN','COUNTERED','AGREED','DECLINED','EXPIRED')
                       OR initiator_side IS NULL OR initiator_side NOT IN ('BUYER','SELLER')
                       OR initiator_org_id = counterparty_org_id
                       OR quantity_mt IS NULL OR quantity_mt < 0.01 OR quantity_mt > 100000
                       OR current_price IS NULL OR current_price < 0.01 OR current_price > 1000000
                       OR (status = 'AGREED') IS DISTINCT FROM (trade_id IS NOT NULL)
                    ORDER BY id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid negotiations (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT min(id::text) AS id
                    FROM negotiations
                    WHERE status IN ('OPEN','COUNTERED')
                    GROUP BY LEAST(initiator_org_id, counterparty_org_id),
                             GREATEST(initiator_org_id, counterparty_org_id),
                             product_id
                    HAVING count(*) > 1
                    ORDER BY min(id::text) LIMIT 20
                ) duplicates;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: duplicate active canonical negotiations (up to 20 ids): %', bad_ids;
                END IF;

                SELECT string_agg(id::text, ',') INTO bad_ids FROM (
                    SELECT id FROM negotiation_rounds
                    WHERE proposed_price IS NULL OR proposed_price < 0.01 OR proposed_price > 1000000
                    ORDER BY id LIMIT 20
                ) invalid;
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: invalid negotiation rounds (up to 20 ids): %', bad_ids;
                END IF;

            END $$;
            """
        )
    )


def _alter_numeric(table: str, column: str, existing: sa.Numeric) -> None:
    op.alter_column(table, column, existing_type=existing, type_=sa.Numeric())


def upgrade() -> None:
    _preflight_sentinel()
    _preflight()

    op.create_table(
        "market_event_outbox",
        sa.Column("id", sa.UUID(), nullable=False),
        sa.Column("event_type", sa.String(length=80), nullable=False),
        sa.Column("aggregate_type", sa.String(length=64), nullable=False),
        sa.Column("aggregate_id", sa.String(length=100), nullable=False),
        sa.Column("participant_org_ids", sa.JSON(), nullable=False),
        sa.Column("payload", sa.JSON(), nullable=False),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
        sa.Column("dispatched_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("delivery_attempts", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("last_error", sa.Text(), nullable=True),
        sa.CheckConstraint(
            "json_array_length(participant_org_ids) > 0",
            name="ck_market_event_outbox_participants",
        ),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_market_event_outbox_pending",
        "market_event_outbox",
        ["dispatched_at", "created_at"],
    )

    # VERIFIED was the legacy spelling of the same account-approval state.
    # This normalization does not infer market provenance.
    op.execute(sa.text("UPDATE organizations SET verification_status = 'APPROVED' WHERE upper(verification_status) = 'VERIFIED'"))
    op.add_column(
        "organizations",
        sa.Column("provenance", sa.String(length=7), nullable=False, server_default="UNKNOWN"),
    )
    op.execute(
        sa.text(
            "SELECT member.id FROM users AS member "
            "WHERE member.organization_id IN ("
            "SELECT organization_id FROM organization_market_approvals"
            ") ORDER BY member.organization_id, member.id FOR UPDATE"
        )
    )
    op.execute(
        sa.text(
            f"""
            DO $$
            DECLARE bad_ids text;
            BEGIN
                SELECT string_agg(approval.organization_id::text, ', ' ORDER BY approval.organization_id)
                INTO bad_ids
                FROM organization_market_approvals AS approval
                JOIN organizations AS organization ON organization.id = approval.organization_id
                WHERE approval.database_name <> current_database()
                   OR NOT (
                        (current_database() = 'verdaxis' AND approval.environment = 'production')
                        OR (current_database() = 'verdaxis_staging' AND approval.environment = 'staging')
                        OR (right(current_database(), length('_market_integrity_test'))
                            = '_market_integrity_test'
                            AND approval.environment = 'test')
                   )
                   OR approval.organization_id IN ({_quoted(_DEMO_IDS + _TEST_IDS)})
                   OR approval.reviewed_snapshot::jsonb IS DISTINCT FROM jsonb_build_object(
                        'organization', jsonb_build_object(
                            'id', organization.id::text,
                            'name', organization.name,
                            'domain', organization.domain,
                            'type', organization.type,
                            'country_code', organization.country_code,
                            'tax_id', organization.tax_id,
                            'verification_status', organization.verification_status
                        ),
                        'eligible_user_ids', coalesce((
                            SELECT jsonb_agg(member.id::text ORDER BY member.id::text)
                            FROM users AS member
                            WHERE member.organization_id = organization.id
                              AND member.role IN ('BUYER', 'SUPPLIER')
                              AND member.status = 'APPROVED'
                              AND member.email_verified IS TRUE
                              AND member.must_change_password IS FALSE
                              AND coalesce(member.kyc_status, 'PENDING') <> 'REJECTED'
                              AND (member.kyc_organization_id IS NULL
                                   OR member.kyc_organization_id = organization.id)
                        ), '[]'::jsonb)
                    );
                IF bad_ids IS NOT NULL THEN
                    RAISE EXCEPTION 'market-integrity preflight: organization approval snapshot drift (ids): %', bad_ids;
                END IF;
            END $$
            """
        )
    )
    op.execute(
        sa.text(
            "UPDATE organizations AS organization SET provenance = 'REAL' "
            "FROM organization_market_approvals AS approval "
            "WHERE approval.organization_id = organization.id "
            "AND organization.verification_status = 'APPROVED'"
        )
    )
    op.execute(sa.text(f"UPDATE organizations SET provenance = 'DEMO' WHERE id IN ({_quoted(_DEMO_IDS)})"))
    op.execute(sa.text(f"UPDATE organizations SET provenance = 'TEST' WHERE id IN ({_quoted(_TEST_IDS)})"))

    op.add_column(
        "orderbook_orders",
        sa.Column("provenance", sa.String(length=7), nullable=False, server_default="UNKNOWN"),
    )
    # orderbook_orders.inventory_item_id, its fk_orderbook_orders_inventory_item_id
    # foreign key, and ix_orderbook_orders_inventory_item_id are created by
    # sec_20260720_fresh earlier in the linearized chain; this revision must
    # not re-create them.
    op.add_column("orderbook_orders", sa.Column("idempotency_key", sa.String(length=255), nullable=True))
    op.add_column("orderbook_orders", sa.Column("idempotency_operation", sa.String(length=64), nullable=True))
    op.add_column("orderbook_orders", sa.Column("idempotency_request_hash", sa.String(length=64), nullable=True))
    op.execute(
        sa.text(
            "UPDATE orderbook_orders AS orders SET provenance = organizations.provenance "
            "FROM organizations WHERE organizations.id = orders.organization_id"
        )
    )

    op.add_column("inventory_items", sa.Column("owner_user_id", sa.UUID(), nullable=True))
    op.create_foreign_key(
        "fk_inventory_items_owner_user_id",
        "inventory_items",
        "users",
        ["owner_user_id"],
        ["id"],
        ondelete="SET NULL",
    )

    op.add_column("negotiations", sa.Column("delivery_point_id", sa.UUID(), nullable=True))
    op.add_column(
        "negotiations",
        sa.Column("availability_window", sa.String(length=16), nullable=False, server_default="SPOT"),
    )
    op.create_foreign_key(
        "fk_negotiations_delivery_point_id",
        "negotiations",
        "delivery_points",
        ["delivery_point_id"],
        ["id"],
    )
    op.execute(
        sa.text(
            """
            UPDATE negotiations AS n
            SET delivery_point_id = o.delivery_point_id,
                availability_window = o.availability_window
            FROM orderbook_orders AS o
            WHERE o.id = COALESCE(n.ask_order_id, n.bid_order_id)
            """
        )
    )

    op.add_column("rfqs", sa.Column("accepted_quote_id", sa.UUID(), nullable=True))
    op.add_column("rfqs", sa.Column("trade_id", sa.UUID(), nullable=True))
    op.create_foreign_key(
        "fk_rfqs_accepted_quote_id", "rfqs", "rfq_quotes", ["accepted_quote_id"], ["id"]
    )
    op.create_foreign_key("fk_rfqs_trade_id", "rfqs", "trades", ["trade_id"], ["id"])

    trade_columns = (
        sa.Column("buyer_provenance", sa.String(length=7), nullable=False, server_default="UNKNOWN"),
        sa.Column("seller_provenance", sa.String(length=7), nullable=False, server_default="UNKNOWN"),
        sa.Column("initiator_org_id", sa.UUID(), nullable=True),
        sa.Column("product_id", sa.UUID(), nullable=True),
        sa.Column("product_name", sa.String(length=120), nullable=True),
        sa.Column("fuel_type", sa.String(length=64), nullable=True),
        sa.Column("fuel_grade", sa.String(length=64), nullable=True),
        sa.Column("market_product", sa.String(length=64), nullable=True),
        sa.Column("delivery_point_id", sa.UUID(), nullable=True),
        sa.Column("delivery_point_name", sa.String(length=120), nullable=True),
        sa.Column("delivery_point_region", sa.String(length=120), nullable=True),
        sa.Column("availability_window", sa.String(length=16), nullable=True),
        sa.Column("market_snapshot_version", sa.SmallInteger(), nullable=True),
        sa.Column("idempotency_key", sa.String(length=255), nullable=True),
        sa.Column("idempotency_operation", sa.String(length=64), nullable=True),
        sa.Column("idempotency_request_hash", sa.String(length=64), nullable=True),
    )
    for column in trade_columns:
        op.add_column("trades", column)
    op.create_foreign_key(
        "fk_trades_initiator_org_id", "trades", "organizations", ["initiator_org_id"], ["id"]
    )
    op.create_foreign_key("fk_trades_product_id", "trades", "products", ["product_id"], ["id"])
    op.create_foreign_key(
        "fk_trades_delivery_point_id", "trades", "delivery_points", ["delivery_point_id"], ["id"]
    )
    op.execute(
        sa.text(
            """
            UPDATE trades AS t
            SET buyer_provenance = buyer.provenance,
                seller_provenance = seller.provenance,
                initiator_org_id = CASE
                    WHEN t.initiated_by = 'BUYER' THEN t.buyer_id
                    ELSE t.seller_id
                END
            FROM organizations AS buyer, organizations AS seller
            WHERE buyer.id = t.buyer_id AND seller.id = t.seller_id
            """
        )
    )
    op.execute(
        sa.text(
            f"""
            UPDATE trades AS t
            SET product_id = o.product_id,
                product_name = p.name,
                fuel_type = p.fuel_type,
                fuel_grade = p.fuel_grade,
                market_product = {_product_row_case("p")},
                delivery_point_id = o.delivery_point_id,
                delivery_point_name = dp.name,
                delivery_point_region = dp.region,
                availability_window = o.availability_window
            FROM orderbook_orders AS o
            JOIN products AS p ON p.id = o.product_id
            LEFT JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
            WHERE o.id = COALESCE(t.ask_order_id, t.bid_order_id)
            """
        )
    )
    op.execute(
        sa.text(
            f"""
            UPDATE trades AS t
            SET market_snapshot_version = 1
            WHERE {_product_snapshot_predicate("t")}
              AND {_delivery_snapshot_predicate("t")}
              AND (t.availability_window = 'SPOT'
                   OR t.availability_window ~ '^[0-9]{{4}}-(0[1-9]|1[0-2])$'
                   OR t.availability_window ~ '^[0-9]{{4}}-Q[1-4]$'
                   OR t.availability_window ~ '^[0-9]{{4}}-CAL$')
              AND ((buyer_provenance = 'REAL' AND seller_provenance = 'REAL')
                   OR (buyer_provenance = 'DEMO' AND seller_provenance = 'DEMO'))
            """
        )
    )

    # Convert typmod NUMERIC columns before adding scale checks.  PostgreSQL
    # applies typmod rounding before CHECK evaluation; unbounded NUMERIC lets
    # the checks reject, rather than silently round, direct SQL inputs.
    for table, column, existing in (
        ("orderbook_orders", "quantity_mt", sa.Numeric(12, 2)),
        ("orderbook_orders", "remaining_quantity_mt", sa.Numeric(12, 2)),
        ("orderbook_orders", "price_per_mt_usd", sa.Numeric(10, 2)),
        ("orderbook_orders", "carbon_intensity_gco2_mj", sa.Numeric(8, 2)),
        ("orderbook_orders", "energy_density_mj_kg", sa.Numeric(6, 2)),
        ("trades", "quantity_mt", sa.Numeric(12, 2)),
        ("trades", "price_per_mt_usd", sa.Numeric(10, 2)),
        ("trades", "final_quantity_mt", sa.Numeric(12, 2)),
        ("trades", "final_price_per_mt", sa.Numeric(10, 2)),
        ("trades", "final_total_usd", sa.Numeric(14, 2)),
        ("trades", "commission_rate_pct", sa.Numeric(5, 3)),
        ("trades", "commission_amount_usd", sa.Numeric(12, 2)),
        ("inventory_items", "current_stock_mt", sa.Numeric(10, 2)),
        ("inventory_items", "incoming_stock_mt", sa.Numeric(10, 2)),
        ("inventory_items", "reserved_stock_mt", sa.Numeric(10, 2)),
        ("inventory_items", "price_per_mt_usd", sa.Numeric(10, 2)),
        ("inventory_items", "energy_density_mj_kg", sa.Numeric(5, 2)),
        ("inventory_items", "carbon_intensity_gco2_mj", sa.Numeric(8, 2)),
        ("rfqs", "quantity_mt", sa.Numeric(12, 2)),
        ("rfqs", "target_price_per_mt", sa.Numeric(10, 2)),
        ("rfq_quotes", "price_per_mt_usd", sa.Numeric(10, 2)),
        ("negotiations", "quantity_mt", sa.Numeric(12, 2)),
        ("negotiations", "current_price", sa.Numeric(10, 2)),
        ("negotiation_rounds", "proposed_price", sa.Numeric(10, 2)),
    ):
        _alter_numeric(table, column, existing)

    # Runtime-owned index cleanup, metadata defaults/nullability, and the
    # inventory fuel-width change deliberately remain in runtime-v2.  Final
    # integration linearizes that revision before this market-owned DDL.
    op.alter_column("orderbook_orders", "side", existing_type=sa.String(10), type_=sa.String(3))
    op.alter_column(
        "orderbook_orders",
        "status",
        existing_type=sa.String(20),
        type_=sa.String(16),
    )
    op.alter_column(
        "orderbook_orders", "availability_window", existing_type=sa.String(50),
        type_=sa.String(16),
    )

    op.alter_column("trades", "initiated_by", existing_type=sa.String(10), type_=sa.String(6))
    op.alter_column(
        "trades",
        "status",
        existing_type=sa.String(30),
        type_=sa.String(20),
    )
    op.alter_column("trades", "initiator_org_id", existing_type=sa.UUID(), nullable=False)
    op.alter_column(
        "trades", "market_snapshot_version", existing_type=sa.SmallInteger(),
        nullable=True, server_default="1",
    )
    op.alter_column("rfqs", "status", existing_type=sa.String(), type_=sa.String(9))
    op.alter_column("rfqs", "availability_window", existing_type=sa.String(), type_=sa.String(16))
    op.alter_column("rfq_quotes", "status", existing_type=sa.String(), type_=sa.String(9))
    op.alter_column("negotiations", "status", existing_type=sa.String(), type_=sa.String(9))
    op.alter_column(
        "negotiations",
        "initiator_side",
        existing_type=sa.String(10),
        type_=sa.String(6),
    )
    # Integration note: the market InventoryItem model declares server-side
    # zero defaults on the stock counters and an explicit supplier index; the
    # market-v2 branch deferred this DDL across the runtime seam, so the
    # linearized chain owns it here.
    op.alter_column("inventory_items", "incoming_stock_mt", existing_type=sa.Numeric(), server_default="0")
    op.alter_column("inventory_items", "reserved_stock_mt", existing_type=sa.Numeric(), server_default="0")
    op.create_index("ix_inventory_items_supplier_id", "inventory_items", ["supplier_id"])
    # sec_20260720_fresh created fk_orderbook_orders_inventory_item_id with
    # ON DELETE SET NULL; market integrity requires RESTRICT (inventory rows
    # with linked listings are conserved, never silently detached). Replace
    # the constraint in place instead of re-adding the column, which
    # sec_20260720_fresh already owns.
    op.drop_constraint(
        "fk_orderbook_orders_inventory_item_id",
        "orderbook_orders",
        type_="foreignkey",
    )
    op.create_foreign_key(
        "fk_orderbook_orders_inventory_item_id",
        "orderbook_orders",
        "inventory_items",
        ["inventory_item_id"],
        ["id"],
        ondelete="RESTRICT",
    )
    op.create_unique_constraint(
        "uq_orderbook_orders_org_operation_idempotency",
        "orderbook_orders",
        ["organization_id", "idempotency_operation", "idempotency_key"],
    )
    # The actual parent schema has no uq_trades_party_idempotency_key.
    op.create_unique_constraint(
        "uq_trades_initiator_operation_idempotency",
        "trades",
        ["initiator_org_id", "idempotency_operation", "idempotency_key"],
    )

    checks = (
        ("organizations", "ck_organizations_provenance", ORGANIZATION_PROVENANCE_DOMAIN),
        ("orderbook_orders", "ck_orderbook_orders_domain", ORDER_DOMAIN),
        ("orderbook_orders", "ck_orderbook_orders_numeric_values", ORDER_NUMERIC_VALUES),
        ("orderbook_orders", "ck_orderbook_orders_lifecycle", ORDER_LIFECYCLE),
        ("trades", "ck_trades_domain", TRADE_DOMAIN),
        ("trades", "ck_trades_numeric_values", TRADE_NUMERIC_VALUES),
        ("trades", "ck_trades_lifecycle", TRADE_LIFECYCLE),
        ("trades", "ck_trades_snapshot", TRADE_SNAPSHOT),
        ("inventory_items", "ck_inventory_items_numeric_values", INVENTORY_NUMERIC_VALUES),
        ("rfqs", "ck_rfqs_domain", RFQ_DOMAIN),
        ("rfqs", "ck_rfqs_numeric_values", RFQ_NUMERIC_VALUES),
        ("rfqs", "ck_rfqs_lifecycle", RFQ_LIFECYCLE),
        ("rfq_quotes", "ck_rfq_quotes_domain", RFQ_QUOTE_DOMAIN),
        ("rfq_quotes", "ck_rfq_quotes_numeric_values", RFQ_QUOTE_NUMERIC_VALUES),
        ("negotiations", "ck_negotiations_domain", NEGOTIATION_DOMAIN),
        ("negotiations", "ck_negotiations_numeric_values", NEGOTIATION_NUMERIC_VALUES),
        ("negotiations", "ck_negotiations_lifecycle", NEGOTIATION_LIFECYCLE),
        ("negotiation_rounds", "ck_negotiation_rounds_numeric_values", NEGOTIATION_ROUND_NUMERIC_VALUES),
    )
    for table, name, expression in checks:
        op.create_check_constraint(name, table, expression)

    trigger_statements = (
        """
            CREATE FUNCTION verdaxis_immutable_org_provenance()
            RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
                IF TG_OP = 'INSERT' THEN
                    IF NEW.provenance = 'REAL' THEN
                        RAISE EXCEPTION 'REAL organization provenance requires a later operator-only approval migration';
                    ELSIF NEW.provenance = 'DEMO' AND NEW.id NOT IN (
                        SELECT value::uuid FROM unnest(ARRAY[
                            '4da7b285-34ee-5443-9406-f96b4ed1a251',
                            '0dbce576-2026-5925-ab66-674d505e98ad',
                            '3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a',
                            '277491df-cb0d-5f2d-a2cf-5746829c6da6',
                            '3b302066-d65c-5c3e-8fcc-70b3da3bcafd',
                            '79609f48-0a3e-560e-a1e1-63d90601d84a',
                            '2c4e387e-de22-5adb-ad88-9274ba84ebe1',
                            '612953c7-567a-58b3-bc42-ee817d2bbe74',
                            '93ccda09-54b3-53ee-afc0-759d3048161f',
                            '82426590-0963-5486-9b05-f81e97afe6ef',
                            'acc3f20a-fe94-4463-9029-a55e35634eb7',
                            'c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4',
                            '7cc77115-0a9f-4ec4-8c74-05aa10050111',
                            'd1e43e55-3fb0-4b5e-9f0b-93aa10050222'
                        ]) AS value
                    ) THEN
                        RAISE EXCEPTION 'DEMO provenance is restricted to deterministic seed identities';
                    ELSIF NEW.provenance = 'TEST' AND NEW.id NOT IN (
                        SELECT value::uuid FROM unnest(ARRAY[
                            '9e63f7a1-0000-4000-8000-000000000001',
                            '9e63f7a1-0000-4000-8000-000000000002',
                            '9e63f7a1-0000-4000-8000-000000000003',
                            '9e63f7a1-0000-4000-8000-000000000004',
                            '9e63f7a1-0000-4000-8000-000000000011',
                            '9e63f7a1-0000-4000-8000-000000000012',
                            '9e63f7a1-0000-4000-8000-000000000013',
                            '9e63f7a1-0000-4000-8000-000000000014'
                        ]) AS value
                    ) THEN
                        RAISE EXCEPTION 'TEST provenance is restricted to deterministic test identities';
                    ELSIF NEW.provenance = 'CANARY' THEN
                        RAISE EXCEPTION 'CANARY provenance requires an external security-owned registry';
                    END IF;
                ELSIF OLD.provenance IS DISTINCT FROM NEW.provenance THEN
                    IF NOT (
                        OLD.provenance = 'UNKNOWN'
                        AND NEW.provenance = 'REAL'
                        AND NEW.verification_status = 'APPROVED'
                        AND EXISTS (
                            SELECT 1 FROM organization_market_approvals
                            WHERE organization_id = NEW.id
                        )
                    ) THEN
                        RAISE EXCEPTION 'organization provenance is immutable';
                    END IF;
                END IF;
                RETURN NEW;
            END;
            $$
        """,
        """
            CREATE TRIGGER organizations_provenance_immutable
            BEFORE INSERT OR UPDATE ON organizations
            FOR EACH ROW EXECUTE FUNCTION verdaxis_immutable_org_provenance()
        """,
        """
            CREATE FUNCTION verdaxis_validate_order_provenance()
            RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE expected_provenance text;
            BEGIN
                SELECT provenance INTO STRICT expected_provenance
                FROM organizations WHERE id = NEW.organization_id;
                IF NEW.provenance IS DISTINCT FROM expected_provenance THEN
                    RAISE EXCEPTION 'order provenance must equal owning organization provenance';
                END IF;
                IF TG_OP = 'UPDATE' AND (
                    OLD.organization_id IS DISTINCT FROM NEW.organization_id
                    OR OLD.provenance IS DISTINCT FROM NEW.provenance
                    OR OLD.side IS DISTINCT FROM NEW.side
                    OR OLD.product_id IS DISTINCT FROM NEW.product_id
                    OR OLD.delivery_point_id IS DISTINCT FROM NEW.delivery_point_id
                    OR OLD.availability_window IS DISTINCT FROM NEW.availability_window
                    OR OLD.inventory_item_id IS DISTINCT FROM NEW.inventory_item_id
                ) THEN
                    RAISE EXCEPTION 'order ownership, provenance, slice, and inventory snapshot are immutable';
                END IF;
                RETURN NEW;
            END;
            $$
        """,
        """
            CREATE TRIGGER orderbook_orders_provenance_guard
            BEFORE INSERT OR UPDATE OF organization_id, provenance, side, product_id,
                delivery_point_id, availability_window, inventory_item_id ON orderbook_orders
            FOR EACH ROW EXECUTE FUNCTION verdaxis_validate_order_provenance()
        """,
        f"""
            CREATE FUNCTION verdaxis_validate_trade_snapshot()
            RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE current_buyer_provenance text;
            DECLARE current_seller_provenance text;
            DECLARE product_row products%ROWTYPE;
            DECLARE delivery_point_row delivery_points%ROWTYPE;
            DECLARE expected_market_product text;
            BEGIN
                IF TG_OP = 'INSERT' THEN
                    SELECT provenance INTO STRICT current_buyer_provenance
                    FROM organizations WHERE id = NEW.buyer_id;
                    SELECT provenance INTO STRICT current_seller_provenance
                    FROM organizations WHERE id = NEW.seller_id;
                    IF NEW.buyer_provenance IS DISTINCT FROM current_buyer_provenance
                       OR NEW.seller_provenance IS DISTINCT FROM current_seller_provenance THEN
                        RAISE EXCEPTION 'trade provenance snapshots must equal current party provenance';
                    END IF;
                    IF NEW.market_snapshot_version IS DISTINCT FROM 1 THEN
                        RAISE EXCEPTION 'new trades require immutable market snapshot version 1';
                    END IF;
                    SELECT * INTO STRICT product_row
                    FROM products WHERE id = NEW.product_id;
                    expected_market_product := {_product_row_case("product_row")};
                    IF product_row.is_active IS DISTINCT FROM true
                       OR expected_market_product IS NULL THEN
                        RAISE EXCEPTION 'trade product snapshot must reference an active canonical product';
                    END IF;
                    IF ROW(
                        NEW.product_name, NEW.fuel_type, NEW.fuel_grade,
                        NEW.market_product
                    ) IS DISTINCT FROM ROW(
                        product_row.name, product_row.fuel_type,
                        product_row.fuel_grade, expected_market_product
                    ) THEN
                        RAISE EXCEPTION 'trade product snapshot labels do not match referenced product';
                    END IF;
                    SELECT * INTO STRICT delivery_point_row
                    FROM delivery_points WHERE id = NEW.delivery_point_id;
                    IF delivery_point_row.is_active IS DISTINCT FROM true
                       OR NOT {_delivery_row_predicate("delivery_point_row")} THEN
                        RAISE EXCEPTION 'trade delivery snapshot must reference an active canonical delivery point';
                    END IF;
                    IF ROW(
                        NEW.delivery_point_name, NEW.delivery_point_region
                    ) IS DISTINCT FROM ROW(
                        delivery_point_row.name, delivery_point_row.region
                    ) THEN
                        RAISE EXCEPTION 'trade delivery snapshot labels do not match referenced delivery point';
                    END IF;
                    IF NEW.availability_window IS NULL OR NOT (
                        NEW.availability_window = 'SPOT'
                        OR NEW.availability_window ~ '^[0-9]{{4}}-(0[1-9]|1[0-2])$'
                        OR NEW.availability_window ~ '^[0-9]{{4}}-Q[1-4]$'
                        OR NEW.availability_window ~ '^[0-9]{{4}}-CAL$'
                    ) THEN
                        RAISE EXCEPTION 'trade snapshot requires a canonical availability window';
                    END IF;
                    IF NOT (
                        (NEW.buyer_provenance = 'REAL' AND NEW.seller_provenance = 'REAL')
                        OR (NEW.buyer_provenance = 'DEMO' AND NEW.seller_provenance = 'DEMO'
                            AND NEW.buyer_id IN ({_quoted(_DEMO_IDS)})
                            AND NEW.seller_id IN ({_quoted(_DEMO_IDS)}))
                    ) THEN
                        RAISE EXCEPTION 'trade execution requires REAL-REAL or an allowlisted DEMO-DEMO pair';
                    END IF;
                ELSE
                    IF ROW(
                        OLD.bid_order_id, OLD.ask_order_id,
                        OLD.buyer_id, OLD.seller_id, OLD.initiator_org_id,
                        OLD.buyer_provenance, OLD.seller_provenance,
                        OLD.product_id, OLD.product_name, OLD.fuel_type, OLD.fuel_grade,
                        OLD.market_product, OLD.delivery_point_id, OLD.delivery_point_name,
                        OLD.delivery_point_region, OLD.availability_window, OLD.market_snapshot_version
                    ) IS DISTINCT FROM ROW(
                        NEW.bid_order_id, NEW.ask_order_id,
                        NEW.buyer_id, NEW.seller_id, NEW.initiator_org_id,
                        NEW.buyer_provenance, NEW.seller_provenance,
                        NEW.product_id, NEW.product_name, NEW.fuel_type, NEW.fuel_grade,
                        NEW.market_product, NEW.delivery_point_id, NEW.delivery_point_name,
                        NEW.delivery_point_region, NEW.availability_window, NEW.market_snapshot_version
                    ) THEN
                        RAISE EXCEPTION 'trade parties, provenance, and market snapshots are immutable';
                    END IF;
                END IF;
                RETURN NEW;
            END;
            $$
        """,
        """
            CREATE TRIGGER trades_snapshot_guard
            BEFORE INSERT OR UPDATE ON trades
            FOR EACH ROW EXECUTE FUNCTION verdaxis_validate_trade_snapshot()
        """,
        """
            CREATE FUNCTION verdaxis_validate_rfq_quote_party()
            RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE buyer uuid;
            BEGIN
                SELECT buyer_org_id INTO STRICT buyer FROM rfqs WHERE id = NEW.rfq_id;
                IF NEW.seller_org_id = buyer THEN
                    RAISE EXCEPTION 'RFQ buyer and quote seller must differ';
                END IF;
                IF TG_OP = 'UPDATE' AND EXISTS (
                    SELECT 1 FROM rfqs
                    WHERE accepted_quote_id = OLD.id AND status = 'ACCEPTED'
                      AND (NEW.id IS DISTINCT FROM OLD.id
                           OR NEW.rfq_id IS DISTINCT FROM OLD.rfq_id
                           OR NEW.seller_org_id IS DISTINCT FROM OLD.seller_org_id
                           OR NEW.price_per_mt_usd IS DISTINCT FROM OLD.price_per_mt_usd
                           OR NEW.status IS DISTINCT FROM 'ACCEPTED')
                ) THEN
                    RAISE EXCEPTION 'accepted RFQ quote identity and economics are immutable';
                END IF;
                RETURN NEW;
            END;
            $$
        """,
        """
            CREATE TRIGGER rfq_quotes_party_guard
            BEFORE INSERT OR UPDATE ON rfq_quotes
            FOR EACH ROW EXECUTE FUNCTION verdaxis_validate_rfq_quote_party()
        """,
        """
            CREATE FUNCTION verdaxis_validate_accepted_rfq()
            RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE quote_row rfq_quotes%ROWTYPE;
            DECLARE trade_row trades%ROWTYPE;
            BEGIN
                IF NEW.status = 'ACCEPTED' THEN
                    SELECT * INTO STRICT quote_row FROM rfq_quotes WHERE id = NEW.accepted_quote_id;
                    SELECT * INTO STRICT trade_row FROM trades WHERE id = NEW.trade_id;
                    IF quote_row.rfq_id IS DISTINCT FROM NEW.id
                       OR quote_row.status IS DISTINCT FROM 'ACCEPTED'
                       OR trade_row.buyer_id IS DISTINCT FROM NEW.buyer_org_id
                       OR trade_row.seller_id IS DISTINCT FROM quote_row.seller_org_id
                       OR trade_row.product_id IS DISTINCT FROM NEW.product_id
                       OR trade_row.delivery_point_id IS DISTINCT FROM NEW.delivery_point_id
                       OR trade_row.availability_window IS DISTINCT FROM NEW.availability_window
                       OR trade_row.quantity_mt > NEW.quantity_mt
                       OR trade_row.price_per_mt_usd IS DISTINCT FROM quote_row.price_per_mt_usd THEN
                        RAISE EXCEPTION 'accepted RFQ, quote, and trade are inconsistent';
                    END IF;
                END IF;
                RETURN NEW;
            END;
            $$
        """,
        """
            CREATE TRIGGER rfqs_acceptance_guard
            BEFORE INSERT OR UPDATE ON rfqs
            FOR EACH ROW EXECUTE FUNCTION verdaxis_validate_accepted_rfq()
        """,
    )
    for statement in trigger_statements:
        op.execute(sa.text(statement))

    op.create_index(
        "ix_orderbook_orders_inventory_active",
        "orderbook_orders",
        ["inventory_item_id", "status", "expires_at"],
    )
    op.create_index(
        "ix_orderbook_orders_active_slice_expiry",
        "orderbook_orders",
        ["product_id", "delivery_point_id", "availability_window", "status", "expires_at"],
    )
    op.create_index(
        "ix_orderbook_orders_public_aggregate",
        "orderbook_orders",
        [
            "status",
            "provenance",
            "product_id",
            "delivery_point_id",
            "availability_window",
            "side",
            "expires_at",
        ],
    )
    op.create_index(
        "ix_trades_snapshot_aggregation",
        "trades",
        ["product_id", "market_product", "delivery_point_id", "availability_window", "confirmed_at"],
    )
    op.create_index("ix_trades_status_confirmed_at", "trades", ["status", "confirmed_at"])
    op.create_index("ix_trades_buyer_created_at", "trades", ["buyer_id", "created_at"])
    op.create_index("ix_trades_seller_created_at", "trades", ["seller_id", "created_at"])
    op.create_index("ix_inventory_items_owner_user_id", "inventory_items", ["owner_user_id"])
    op.create_index("ix_rfq_quotes_seller_org_id", "rfq_quotes", ["seller_org_id"])
    op.create_unique_constraint(
        "uq_rfq_quotes_rfq_seller",
        "rfq_quotes",
        ["rfq_id", "seller_org_id"],
    )
    op.create_index(
        "uq_negotiations_active_pair_product",
        "negotiations",
        [
            sa.text("LEAST(initiator_org_id, counterparty_org_id)"),
            sa.text("GREATEST(initiator_org_id, counterparty_org_id)"),
            "product_id",
        ],
        unique=True,
        postgresql_where=sa.text("status IN ('OPEN', 'COUNTERED')"),
    )


def downgrade() -> None:
    # This revision changes typmods to reject-before-round semantics and makes
    # provenance/snapshots immutable.  Reconstructing the old schema would
    # silently round values and would require VERIFIED/APPROVED provenance
    # reinterpretation. Refuse before issuing any DDL or DML.
    raise RuntimeError(
        "mi_20260720_market_integrity downgrade is unsupported: restore a parent-schema backup instead"
    )
