"""B100 storage safeguards exercised with the real restricted PostgreSQL role."""
from __future__ import annotations

from copy import deepcopy
from datetime import UTC, datetime, timedelta
from decimal import Decimal
import json
from uuid import uuid4

import pytest
from sqlalchemy import JSON, insert, select, text
from sqlalchemy.exc import DBAPIError

from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.market_support import MarketSupportAuthorization
from app.models.marketplace import InventoryItem
from app.models.negotiation import Negotiation
from app.models.orderbook import OrderBookOrder, Trade
from app.schemas.fame_order import FameAskTerms, FameBidTerms
from tests.postgres.test_fame_rfq import fame_market  # noqa: F401


_TABLES = {
    "orderbook_orders": OrderBookOrder.__table__,
    "trades": Trade.__table__,
    "negotiations": Negotiation.__table__,
    "inventory_items": InventoryItem.__table__,
    "market_support_authorizations": MarketSupportAuthorization.__table__,
}
_TERM_COLUMNS = {
    "orderbook_orders": "fame_terms",
    "trades": "fame_terms_snapshot",
    "negotiations": "fame_terms_snapshot",
    "inventory_items": "fame_terms",
    "market_support_authorizations": "fame_terms",
}
_CONSTRAINTS = {
    table: f"ck_{table}_fame_terms" for table in _TABLES
}
_CONSTRAINTS["market_support_authorizations"] = "ck_market_support_auth_fame_terms"


@pytest.fixture
async def b100_database(fame_market):
    _, seeded = fame_market
    product = PRODUCTS_BY_CODE["BIO_METHANOL"]
    point = DELIVERY_POINTS_BY_NAME["Dalian"]
    async with seeded["owner_factory"]() as session:
        session.add_all([
            Product(
                id=product.id, name=product.name, fuel_type=product.fuel_type,
                fuel_grade=product.fuel_grade, unit=product.unit, is_active=True,
            ),
            DeliveryPoint(
                id=point.id, name=point.name, region=point.region,
                timezone=point.timezone, is_active=True,
            ),
        ])
        await session.commit()
    return seeded


def _terms(side):
    common = {
        "side": side, "neat_fame": True, "standard": "EN_14214",
        "standard_edition": "2012+A2:2019", "sustainability_scheme": "ISCC_EU",
    }
    if side == "BID":
        return FameBidTerms.model_validate(common).model_dump(mode="json")
    return FameAskTerms.model_validate({
        **common, "uco_mass_pct": 100, "certificate_reference": "DECLARED-CERT",
        "certificate_holder": "Declared supplier", "evidence_status": "PENDING",
        "evidence_due": "BEFORE_LOADING",
        "certificate_valid_until": datetime.now(UTC).date() + timedelta(days=365),
    }).model_dump(mode="json")


def _values(table, seeded, *, side="BID", product_code="UCOME_B100", point_name="Singapore"):
    product = PRODUCTS_BY_CODE[product_code]
    point = DELIVERY_POINTS_BY_NAME[point_name]
    bid_terms, ask_terms = _terms("BID"), _terms("ASK")
    pair = {"schema_version": 1, "bid": bid_terms, "ask": ask_terms}
    common = {
        "id": uuid4(), "product_id": product.id, "delivery_point_id": point.id,
        "availability_window": "SPOT", "quantity_mt": Decimal("1.00"),
    }
    if table == "orderbook_orders":
        actor = "buyer" if side == "BID" else "seller"
        values = {
            **common, "organization_id": seeded[f"{actor}_org_id"],
            "owner_user_id": seeded[f"{actor}_id"], "provenance": "REAL", "side": side,
            "remaining_quantity_mt": Decimal("1.00"), "price_per_mt_usd": Decimal("1100.00"),
            "status": "OPEN", "fame_terms": bid_terms if side == "BID" else ask_terms,
        }
    elif table == "trades":
        values = {
            **common, "buyer_id": seeded["buyer_org_id"], "seller_id": seeded["seller_org_id"],
            "buyer_user_id": seeded["buyer_id"], "seller_user_id": seeded["seller_id"],
            "initiator_org_id": seeded["buyer_org_id"], "initiated_by": "BUYER",
            "buyer_provenance": "REAL", "seller_provenance": "REAL",
            "product_name": product.name, "fuel_type": product.fuel_type,
            "fuel_grade": product.fuel_grade, "market_product": product_code,
            "delivery_point_name": point.name, "delivery_point_region": point.region,
            "market_snapshot_version": 1, "price_per_mt_usd": Decimal("1100.00"),
            "status": "PENDING_CONFIRMATION", "fame_terms_snapshot": pair,
        }
    elif table == "negotiations":
        values = {
            **common, "initiator_org_id": seeded["buyer_org_id"],
            "counterparty_org_id": seeded["seller_org_id"],
            "initiator_user_id": seeded["buyer_id"], "counterparty_user_id": seeded["seller_id"],
            "last_actor_org_id": seeded["buyer_org_id"], "initiator_side": "BUYER",
            "current_price": Decimal("1100.00"), "status": "OPEN",
            "expires_at": datetime.now(UTC) + timedelta(hours=2), "fame_terms_snapshot": pair,
        }
    elif table == "inventory_items":
        values = {
            "id": common["id"], "supplier_id": seeded["seller_org_id"],
            "owner_user_id": seeded["seller_id"], "fuel_type": product.fuel_type,
            "product_name": product.name, "current_stock_mt": Decimal("1.00"),
            "price_per_mt_usd": Decimal("1100.00"), "fame_terms": ask_terms,
        }
    else:
        actor = "buyer" if side == "BID" else "seller"
        values = {
            **common, "organization_id": seeded[f"{actor}_org_id"],
            "accountable_user_id": seeded[f"{actor}_id"], "order_side": side,
            "created_by_actor_user_id": seeded[f"{actor}_id"],
            "authorization_expires_at": datetime.now(UTC) + timedelta(hours=2),
            "price_per_mt_usd": Decimal("1100.00"), "certification_declared": True,
            "msds_available": True, "terms_digest": "a" * 64,
            "evidence_reference": "fixture customer instruction",
            "commercial_consent_version": "test-v1", "commercial_consent_reference": "fixture consent",
            "idempotency_key": str(uuid4()), "idempotency_request_hash": "b" * 64,
            "fame_terms": bid_terms if side == "BID" else ask_terms,
        }
    if product_code != "UCOME_B100":
        values[_TERM_COLUMNS[table]] = None
    return values


async def _insert(engine, table, values):
    # Core INSERT bypasses request validation while retaining model defaults.
    # Each terms column uses none_as_null=True; JSON.NULL is tested separately.
    async with engine.begin() as connection:
        return (await connection.execute(
            insert(_TABLES[table]).values(**values).returning(_TABLES[table])
        )).mappings().one()


async def _reject_terms(engine, table, values):
    with pytest.raises(DBAPIError) as error:
        await _insert(engine, table, values)
    assert error.value.orig.sqlstate == "23514", str(error.value.orig)
    assert _CONSTRAINTS[table] in str(error.value.orig)
    async with engine.connect() as connection:
        assert await connection.scalar(select(_TABLES[table].c.id).where(
            _TABLES[table].c.id == values["id"]
        )) is None


@pytest.mark.asyncio
@pytest.mark.parametrize(("table", "side"), [
    ("orderbook_orders", "BID"), ("orderbook_orders", "ASK"),
    ("market_support_authorizations", "BID"), ("market_support_authorizations", "ASK"),
    ("inventory_items", "ASK"), ("trades", "BID"), ("negotiations", "BID"),
])
async def test_runtime_stores_valid_b100_terms_at_one_mt(b100_database, table, side):
    seeded = b100_database
    values = _values(table, seeded, side=side)
    row = await _insert(seeded["runtime_engine"], table, values)
    column = _TERM_COLUMNS[table]
    assert row["id"] == values["id"]
    assert row[column] == values[column]
    quantity_column = "current_stock_mt" if table == "inventory_items" else "quantity_mt"
    assert row[quantity_column] == Decimal("1.00")


@pytest.mark.asyncio
@pytest.mark.parametrize("table", _TABLES)
async def test_sql_rejects_missing_or_malformed_b100_terms(b100_database, table):
    seeded = b100_database
    values = _values(table, seeded)
    column = _TERM_COLUMNS[table]
    wrong_version = {**values[column], "schema_version": "1"}
    wrong_side = deepcopy(values[column])
    if table in ("trades", "negotiations"):
        wrong_side["ask"]["side"] = "BID"
        missing_side = {**values[column], "bid": {"schema_version": 1}}
    else:
        wrong_side["side"] = "BID" if table == "inventory_items" else "ASK"
        missing_side = {"schema_version": 1}
    for invalid in (None, JSON.NULL, {}, [], wrong_version, wrong_side, missing_side):
        await _reject_terms(seeded["runtime_engine"], table, {**values, column: invalid})
    if table in ("trades", "negotiations"):
        wrong_nested_version = deepcopy(values[column])
        wrong_nested_version["bid"]["schema_version"] = "1"
        await _reject_terms(seeded["runtime_engine"], table, {
            **values, column: wrong_nested_version,
        })


@pytest.mark.asyncio
@pytest.mark.parametrize("table", _TABLES)
async def test_non_b100_requires_sql_null_terms(b100_database, table):
    seeded = b100_database
    values = _values(table, seeded, product_code="BIO_METHANOL")
    column = _TERM_COLUMNS[table]
    # JSON null is not SQL NULL and must not bypass product discrimination.
    for invalid in (JSON.NULL, _values(table, seeded)[column]):
        await _reject_terms(seeded["runtime_engine"], table, {**values, column: invalid})
    row = await _insert(seeded["runtime_engine"], table, values)
    assert row[column] is None
    async with seeded["runtime_engine"].connect() as connection:
        assert await connection.scalar(select(_TABLES[table].c[column].is_(None)).where(
            _TABLES[table].c.id == row["id"]
        )) is True


@pytest.mark.asyncio
@pytest.mark.parametrize("table", [
    "orderbook_orders", "trades", "negotiations", "market_support_authorizations",
])
async def test_b100_sql_requires_singapore_and_standing_order_minimum(b100_database, table):
    seeded = b100_database
    await _reject_terms(seeded["runtime_engine"], table, _values(table, seeded, point_name="Dalian"))
    if table in ("orderbook_orders", "market_support_authorizations"):
        below_minimum = {**_values(table, seeded), "quantity_mt": Decimal("0.50")}
        if table == "orderbook_orders":
            below_minimum["remaining_quantity_mt"] = Decimal("0.50")
        await _reject_terms(seeded["runtime_engine"], table, below_minimum)
    # These two legacy nullable lanes need an explicit NULL guard. Trade and
    # authorization lanes already have independent mandatory-snapshot rules.
    if table in ("orderbook_orders", "negotiations"):
        await _reject_terms(seeded["runtime_engine"], table, {
            **_values(table, seeded), "delivery_point_id": None,
        })


@pytest.mark.asyncio
@pytest.mark.parametrize("table", ["trades", "negotiations"])
async def test_b100_execution_snapshots_allow_partial_quantity_below_one_mt(b100_database, table):
    values = {**_values(table, b100_database), "quantity_mt": Decimal("0.50")}
    row = await _insert(b100_database["runtime_engine"], table, values)
    assert row["quantity_mt"] == Decimal("0.50")
    assert row["delivery_point_id"] == DELIVERY_POINTS_BY_NAME["Singapore"].id
    assert row["fame_terms_snapshot"] == values["fame_terms_snapshot"]


@pytest.mark.asyncio
async def test_inventory_b100_requires_fame_fuel_type(b100_database):
    seeded = b100_database
    await _reject_terms(seeded["runtime_engine"], "inventory_items", {
        **_values("inventory_items", seeded), "fuel_type": "Biofuel",
    })


@pytest.mark.asyncio
async def test_inventory_without_product_name_cannot_carry_b100_terms(b100_database):
    engine = b100_database["runtime_engine"]
    values = {**_values("inventory_items", b100_database), "product_name": None}
    await _reject_terms(engine, "inventory_items", values)
    row = await _insert(engine, "inventory_items", {**values, "fame_terms": None})
    assert row["product_name"] is None
    assert row["fame_terms"] is None


@pytest.mark.asyncio
async def test_trade_fame_snapshot_cannot_change_after_insert(b100_database):
    engine = b100_database["runtime_engine"]
    values = _values("trades", b100_database)
    row = await _insert(engine, "trades", values)
    changed = deepcopy(values["fame_terms_snapshot"])
    changed["ask"]["certificate_reference"] = "REPLACEMENT-CERT"
    with pytest.raises(DBAPIError) as error:
        async with engine.begin() as connection:
            await connection.execute(text(
                "UPDATE trades SET fame_terms_snapshot = CAST(:terms AS json) WHERE id = :id"
            ), {"id": row["id"], "terms": json.dumps(changed)})
    assert error.value.orig.sqlstate == "P0001"
    assert "trade parties, provenance, and market snapshots are immutable" in str(error.value.orig)
    async with engine.begin() as connection:
        # Lifecycle updates must remain possible with the captured terms intact.
        stored = (await connection.execute(text(
            "UPDATE trades SET status = 'CONFIRMED', confirmed_at = now() "
            "WHERE id = :id RETURNING status, fame_terms_snapshot"
        ), {"id": row["id"]})).one()
    assert stored == ("CONFIRMED", values["fame_terms_snapshot"])


@pytest.mark.asyncio
@pytest.mark.parametrize("table", ["negotiations", "market_support_authorizations"])
async def test_runtime_can_insert_but_not_update_captured_fame_terms(b100_database, table):
    engine = b100_database["runtime_engine"]
    values = _values(table, b100_database)
    column = _TERM_COLUMNS[table]
    row = await _insert(engine, table, values)
    async with engine.connect() as connection:
        privileges = (await connection.execute(text(
            "SELECT has_table_privilege(current_user, :table, 'UPDATE'), "
            "has_column_privilege(current_user, :table, :column, 'INSERT'), "
            "has_column_privilege(current_user, :table, :column, 'UPDATE')"
        ), {"table": table, "column": column})).one()
    assert privileges == (False, True, False)
    with pytest.raises(DBAPIError) as error:
        async with engine.begin() as connection:
            await connection.execute(text(
                f"UPDATE {table} SET {column} = CAST(:terms AS json) WHERE id = :id"
            ), {"id": row["id"], "terms": json.dumps(values[column])})
    assert error.value.orig.sqlstate == "42501"
    lifecycle = (
        "current_price = 1090, status = 'COUNTERED'" if table == "negotiations"
        else "status = 'REVOKED', revoked_at = now()"
    )
    async with engine.begin() as connection:
        stored = (await connection.execute(text(
            f"UPDATE {table} SET {lifecycle} WHERE id = :id RETURNING status, {column}"
        ), {"id": row["id"]})).one()
    expected_status = "COUNTERED" if table == "negotiations" else "REVOKED"
    assert stored == (expected_status, values[column])
