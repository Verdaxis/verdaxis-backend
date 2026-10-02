"""Focused tests for batched opposing-price slice keys."""

from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from sqlalchemy.dialects import postgresql

from app.models.orderbook import OrderSide
from app.models.user import OrganizationProvenance
from app.routers.orderbook import _load_best_opposing_prices


def _order(
    product_id,
    delivery_point_id,
    availability_window: str,
    provenance=OrganizationProvenance.DEMO,
):
    return SimpleNamespace(
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window=availability_window,
        provenance=provenance,
    )


async def _capture_statement(orders, rows=()):
    result = MagicMock()
    result.all.return_value = list(rows)
    db = AsyncMock()
    db.execute.return_value = result

    prices = await _load_best_opposing_prices(
        db,
        orders,
        opposing_side=OrderSide.ASK,
    )
    statement = db.execute.await_args.args[0]
    return statement, prices


@pytest.mark.asyncio
async def test_duplicate_orders_compile_as_one_slice_predicate():
    repeated_order = _order(uuid4(), uuid4(), "Spot")

    single_statement, _ = await _capture_statement([repeated_order])
    repeated_statement, _ = await _capture_statement([repeated_order] * 100)

    single = single_statement.compile(dialect=postgresql.dialect())
    repeated = repeated_statement.compile(dialect=postgresql.dialect())

    assert str(repeated) == str(single)
    assert repeated.params == single.params
    assert len(str(repeated).encode()) < 10_000


@pytest.mark.asyncio
async def test_mixed_slices_keep_each_supported_normalized_key_once():
    product_a = uuid4()
    product_b = uuid4()
    skipped_product = uuid4()
    delivery_point_a = uuid4()
    delivery_point_b = uuid4()
    orders = [
        _order(product_a, delivery_point_a, "Spot"),
        _order(product_a, delivery_point_a, "SPOT", "DEMO"),
        _order(product_a, delivery_point_a, "2027-Q1"),
        _order(product_a, None, "SPOT", OrganizationProvenance.REAL),
        _order(product_b, delivery_point_b, "SPOT"),
        _order(skipped_product, delivery_point_b, "SPOT", OrganizationProvenance.UNKNOWN),
    ]
    row = SimpleNamespace(
        product_id=product_a,
        delivery_point_id=delivery_point_a,
        availability_window="Spot",
        provenance=OrganizationProvenance.DEMO,
        best_price=Decimal("612.50"),
    )

    statement, prices = await _capture_statement(orders, [row])
    compiled = statement.compile(dialect=postgresql.dialect())
    values = list(compiled.params.values())

    assert values.count(product_a) == 3
    assert values.count(product_b) == 1
    assert skipped_product not in values
    assert values.count(delivery_point_a) == 2
    assert values.count(delivery_point_b) == 1
    assert str(compiled).count("orderbook_orders.delivery_point_id IS NULL") == 1
    assert prices == {
        (product_a, delivery_point_a, "SPOT", "DEMO"): Decimal("612.50")
    }


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "orders",
    [
        [],
        [_order(uuid4(), uuid4(), "SPOT", OrganizationProvenance.UNKNOWN)],
        [_order(uuid4(), uuid4(), "SPOT", None)],
    ],
)
async def test_empty_or_unknown_orders_skip_the_query(orders):
    db = AsyncMock()

    assert await _load_best_opposing_prices(
        db,
        orders,
        opposing_side=OrderSide.ASK,
    ) == {}
    db.execute.assert_not_awaited()
