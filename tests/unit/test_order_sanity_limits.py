"""Unit tests for the fat-finger guard (price band and volume cap)."""
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, Mock
from uuid import uuid4

import pytest
from app.models.user import UserRole
from app.models.orderbook import OrderBookStatus
from app.routers.orderbook import create_order, update_order
from app.schemas.orderbook import OrderCreate, OrderSide, OrderUpdate
from app.services import order_sanity_limits
from app.services.order_sanity_limits import (
    PRICE_OUT_OF_BAND_CODE,
    QUANTITY_LIMIT_EXCEEDED_CODE,
    check_price_band,
    check_quantity_limit,
    enforce_order_sanity_limits,
)
from fastapi import HTTPException

REFERENCE = Decimal("1000.00")
BAND_PCT = Decimal(50)


def _price_check(price: str, *, reference=REFERENCE, pct=BAND_PCT):
    check_price_band(
        price_per_mt_usd=Decimal(price),
        reference_price_per_mt_usd=reference,
        reference_source="manual_override",
        max_deviation_pct=pct,
    )


# --- price band -------------------------------------------------------------


@pytest.mark.parametrize("price", ["1000.00", "1200.00", "800.00", "1500.00", "500.00"])
def test_price_within_band_is_accepted_including_exact_boundaries(price):
    _price_check(price)


@pytest.mark.parametrize(
    "price, side",
    [
        ("1500.01", "above"),
        ("10000.00", "above"),  # extra zero
        ("499.99", "below"),
        ("100.00", "below"),  # dropped zero
        ("0.01", "below"),
    ],
)
def test_price_outside_band_is_rejected_on_both_sides(price, side):
    with pytest.raises(HTTPException) as failure:
        _price_check(price)

    assert failure.value.status_code == 400
    detail = failure.value.detail
    assert detail["code"] == PRICE_OUT_OF_BAND_CODE
    assert detail["submitted_price_per_mt_usd"] == price
    assert detail["reference_price_per_mt_usd"] == "1000.00"
    assert detail["reference_source"] == "manual_override"
    assert detail["max_deviation_pct"] == "50"
    assert detail["min_allowed_price_per_mt_usd"] == "500.00"
    assert detail["max_allowed_price_per_mt_usd"] == "1500.00"
    assert "500.00" in detail["message"] and "1500.00" in detail["message"]


def test_reported_limits_are_themselves_accepted_prices():
    reference = Decimal("962.50")
    pct = Decimal(33)
    with pytest.raises(HTTPException) as failure:
        _price_check("5000", reference=reference, pct=pct)
    detail = failure.value.detail

    _price_check(detail["min_allowed_price_per_mt_usd"], reference=reference, pct=pct)
    _price_check(detail["max_allowed_price_per_mt_usd"], reference=reference, pct=pct)


def test_band_of_100_percent_or_more_never_reports_a_non_positive_minimum():
    with pytest.raises(HTTPException) as failure:
        _price_check("5000", pct=Decimal(120))
    assert failure.value.detail["min_allowed_price_per_mt_usd"] == "0.01"


@pytest.mark.parametrize("reference", [None, Decimal(0), Decimal(-5)])
def test_no_usable_reference_price_does_not_block_the_order(reference):
    _price_check("999999.00", reference=reference)


def test_zero_deviation_setting_disables_the_price_rule():
    _price_check("999999.00", pct=Decimal(0))


# --- volume cap -------------------------------------------------------------


def test_quantity_at_or_below_cap_is_accepted():
    check_quantity_limit(quantity_mt=Decimal(50000), max_quantity_mt=Decimal(50000))
    check_quantity_limit(quantity_mt=Decimal(1), max_quantity_mt=Decimal(50000))


def test_quantity_over_cap_is_rejected_with_threshold():
    with pytest.raises(HTTPException) as failure:
        check_quantity_limit(
            quantity_mt=Decimal("50000.01"), max_quantity_mt=Decimal(50000)
        )

    assert failure.value.status_code == 400
    detail = failure.value.detail
    assert detail["code"] == QUANTITY_LIMIT_EXCEEDED_CODE
    assert detail["max_quantity_mt"] == "50000"
    assert detail["submitted_quantity_mt"] == "50000.01"
    assert "50000" in detail["message"]


def test_zero_cap_disables_the_volume_rule():
    check_quantity_limit(quantity_mt=Decimal(99999999), max_quantity_mt=Decimal(0))


# --- enforce_order_sanity_limits (settings + reference lookup) -----------------


def _quote(price: str | None, source: str = "manual_override"):
    if price is None:
        return None
    return SimpleNamespace(benchmark_price_per_mt_usd=Decimal(price), source=source)


@pytest.fixture
def limits(monkeypatch):
    monkeypatch.setattr(order_sanity_limits.settings, "ORDER_MAX_PRICE_DEVIATION_PCT", BAND_PCT)
    monkeypatch.setattr(order_sanity_limits.settings, "ORDER_MAX_QUANTITY_MT", Decimal(50000))


def _enforce(monkeypatch, *, quote, quantity="1000", price="1000", lookup=None):
    lookup = lookup or AsyncMock(return_value=quote)
    monkeypatch.setattr(order_sanity_limits, "get_benchmark_quote", lookup)
    return lookup, enforce_order_sanity_limits(
        AsyncMock(),
        market_product="BIO_METHANOL",
        delivery_point_id=uuid4(),
        availability_window="SPOT",
        quantity_mt=None if quantity is None else Decimal(quantity),
        price_per_mt_usd=None if price is None else Decimal(price),
    )


@pytest.mark.asyncio
async def test_enforce_accepts_order_within_both_limits(monkeypatch, limits):
    _, call = _enforce(monkeypatch, quote=_quote("1000.00"), quantity="50000", price="1500")
    await call


@pytest.mark.asyncio
async def test_enforce_rejects_price_over_band(monkeypatch, limits):
    _, call = _enforce(monkeypatch, quote=_quote("1000.00"), price="10000")
    with pytest.raises(HTTPException) as failure:
        await call
    assert failure.value.detail["code"] == PRICE_OUT_OF_BAND_CODE


@pytest.mark.asyncio
async def test_enforce_rejects_volume_over_cap_without_a_reference_lookup(monkeypatch, limits):
    lookup, call = _enforce(monkeypatch, quote=_quote("1000.00"), quantity="50001")
    with pytest.raises(HTTPException) as failure:
        await call
    assert failure.value.detail["code"] == QUANTITY_LIMIT_EXCEEDED_CODE
    lookup.assert_not_awaited()


@pytest.mark.asyncio
async def test_enforce_without_reference_price_skips_price_rule_but_keeps_volume_rule(
    monkeypatch, limits
):
    _, call = _enforce(monkeypatch, quote=None, price="999999")
    await call  # no reference: price rule is skipped

    _, call = _enforce(monkeypatch, quote=None, price="999999", quantity="50001")
    with pytest.raises(HTTPException) as failure:
        await call
    assert failure.value.detail["code"] == QUANTITY_LIMIT_EXCEEDED_CODE


@pytest.mark.asyncio
async def test_enforce_skips_fields_that_are_not_being_set(monkeypatch, limits):
    # A price-only amendment is not judged on quantity, and vice versa.
    _, call = _enforce(monkeypatch, quote=_quote("1000.00"), quantity=None, price="1000")
    await call
    lookup, call = _enforce(monkeypatch, quote=_quote("1000.00"), quantity="1000", price=None)
    await call
    lookup.assert_not_awaited()


@pytest.mark.asyncio
async def test_enforce_uses_the_real_seed_benchmark_when_no_override_exists(monkeypatch, limits):
    # Bio Methanol / Rotterdam seed matrix midpoint is 962.50 USD/MT.
    db = AsyncMock()
    db.get.return_value = SimpleNamespace(name="Rotterdam")
    no_override = MagicMock()
    no_override.scalar_one_or_none.return_value = None
    db.execute.return_value = no_override

    kwargs = dict(
        market_product="BIO_METHANOL",
        delivery_point_id=uuid4(),
        availability_window="SPOT",
        quantity_mt=Decimal(1000),
    )
    await enforce_order_sanity_limits(db, price_per_mt_usd=Decimal(950), **kwargs)
    with pytest.raises(HTTPException) as failure:
        await enforce_order_sanity_limits(db, price_per_mt_usd=Decimal(9500), **kwargs)
    assert failure.value.detail["reference_price_per_mt_usd"] == "962.50"
    assert failure.value.detail["reference_source"] == "seed_matrix"


# --- create_order: rejection happens before the slice lock -----------------------


def _buyer():
    user = MagicMock()
    user.id = uuid4()
    user.role = UserRole.BUYER
    user.organization_id = uuid4()
    return user


def _create_order_db():
    product = MagicMock(
        id=uuid4(),
        market_product="BIO_METHANOL",
        min_lot_size=Decimal("200.00"),
        unit="MT",
        name="Bio Methanol",
    )
    product_result = MagicMock()
    product_result.scalars.return_value.first.return_value = product
    delivery_point_result = MagicMock()
    delivery_point_result.scalars.return_value.first.return_value = MagicMock()
    db = AsyncMock()
    db.execute.side_effect = [product_result, delivery_point_result]
    db.add = Mock()
    return product, db


def _order_request(product, *, quantity="1000", price="1000"):
    return OrderCreate(
        side=OrderSide.BID,
        product_id=product.id,
        delivery_point_id=uuid4(),
        quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
    )


@pytest.fixture
def lock(monkeypatch, limits):
    acquire = AsyncMock()
    monkeypatch.setattr("app.routers.orderbook.acquire_market_slice_lock", acquire)
    monkeypatch.setattr(order_sanity_limits, "get_benchmark_quote", AsyncMock(return_value=_quote("1000.00")))
    return acquire


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "quantity, price, code",
    [
        ("1000", "10000", PRICE_OUT_OF_BAND_CODE),
        ("1000", "100", PRICE_OUT_OF_BAND_CODE),
        ("60000", "1000", QUANTITY_LIMIT_EXCEEDED_CODE),
    ],
)
async def test_rejected_order_never_acquires_the_market_slice_lock(lock, quantity, price, code):
    product, db = _create_order_db()
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))

    with pytest.raises(HTTPException) as failure:
        await create_order(
            request=request,
            order_data=_order_request(product, quantity=quantity, price=price),
            current_user=_buyer(),
            db=db,
        )

    assert failure.value.status_code == 400
    assert failure.value.detail["code"] == code
    lock.assert_not_awaited()
    db.flush.assert_not_awaited()


@pytest.mark.asyncio
async def test_accepted_order_proceeds_to_the_market_slice_lock(lock, monkeypatch):
    # Control for the test above: the same path does reach the lock when the
    # order is within limits, so "not awaited" is meaningful.
    product, db = _create_order_db()
    lock.side_effect = RuntimeError("stop-at-lock")
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))

    with pytest.raises(RuntimeError, match="stop-at-lock"):
        await create_order(
            request=request,
            order_data=_order_request(product, quantity="1000", price="1400"),
            current_user=_buyer(),
            db=db,
        )

    lock.assert_awaited_once()


# --- update_order: amendments are guarded too, before the slice locks ----------


def _amendable_order(user):
    return SimpleNamespace(
        id=uuid4(),
        owner_user_id=user.id,
        organization_id=user.organization_id,
        creation_method=None,
        expires_at=None,
        status=OrderBookStatus.OPEN,
        side=OrderSide.BID,
        product_id=uuid4(),
        delivery_point_id=uuid4(),
        availability_window="SPOT",
        market_product="BIO_METHANOL",
        certification_declared=False,
        certification_scheme=None,
    )


async def _amend(monkeypatch, update: OrderUpdate):
    user = _buyer()
    order = _amendable_order(user)
    locks = AsyncMock(side_effect=RuntimeError("stop-at-lock"))
    monkeypatch.setattr("app.routers.orderbook.acquire_market_slice_locks", locks)
    monkeypatch.setattr(
        "app.routers.orderbook.prepare_command_attempt", AsyncMock(return_value=None)
    )
    monkeypatch.setattr(
        "app.routers.orderbook.require_execution_eligible_user", AsyncMock()
    )
    monkeypatch.setattr(
        order_sanity_limits, "get_benchmark_quote", AsyncMock(return_value=_quote("1000.00"))
    )
    result = MagicMock()
    result.scalars.return_value.first.return_value = order
    db = AsyncMock()
    db.execute.return_value = result
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))
    await update_order(
        order_id=order.id, request=request, update_data=update, current_user=user, db=db
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "changes, code",
    [
        ({"price_per_mt_usd": Decimal("10000")}, PRICE_OUT_OF_BAND_CODE),
        ({"quantity_mt": Decimal("60000")}, QUANTITY_LIMIT_EXCEEDED_CODE),
    ],
)
async def test_amendment_over_limits_is_rejected_before_the_slice_locks(
    monkeypatch, limits, changes, code
):
    with pytest.raises(HTTPException) as failure:
        await _amend(monkeypatch, OrderUpdate(**changes))
    assert failure.value.detail["code"] == code
    # The patched locks raise RuntimeError if reached, so an HTTPException with
    # the guard's code proves the rejection came first.


@pytest.mark.asyncio
async def test_amendment_within_limits_reaches_the_slice_locks(monkeypatch, limits):
    with pytest.raises(RuntimeError, match="stop-at-lock"):
        await _amend(monkeypatch, OrderUpdate(price_per_mt_usd=Decimal("1400")))
