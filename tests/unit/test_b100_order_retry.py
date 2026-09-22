"""A transaction retry must retain the caller's idempotency payload."""

from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, Mock

import pytest

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.models.orderbook import OrderBookOrder
from app.routers import orderbook
from app.schemas.orderbook import OrderCreate
from app.services.idempotency import idempotency_request_hash
from app.services.market_support import economic_order_idempotency_payload
from app.services.market_transactions import RetryableMarketTransactionError
from tests.unit.test_orderbook_router import _make_supplier_user


@pytest.mark.asyncio
async def test_fame_create_retry_preserves_original_payload_and_replays(monkeypatch):
    caller = OrderCreate(
        side="ASK",
        product_id=PRODUCT_IDS["UCOME_B100"],
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
        quantity_mt=10,
        price_per_mt_usd=900,
        certification_declared=True,
        msds_available=True,
        fame_terms={
            "side": "ASK",
            "neat_fame": True,
            "uco_mass_pct": 100,
            "standard": "EN_14214",
            "standard_edition": "2012+A2:2019",
            "sustainability_scheme": "ISCC_EU",
            "certificate_reference": "certificate",
            "certificate_holder": "holder",
            "evidence_status": "PENDING",
            "certificate_valid_until": (datetime.now(UTC) + timedelta(days=365)).date(),
            "evidence_due": "BEFORE_LOADING",
        },
    )
    original_payload = caller.model_dump(mode="json")
    expected_hash = idempotency_request_hash(economic_order_idempotency_payload(caller))
    user = _make_supplier_user()
    organization = SimpleNamespace(provenance="REAL", verification_status="APPROVED")
    monkeypatch.setattr(
        orderbook,
        "lock_and_load_market_organizations",
        AsyncMock(return_value={user.organization_id: organization}),
    )
    monkeypatch.setattr(orderbook, "acquire_idempotency_lock", AsyncMock())
    monkeypatch.setattr(orderbook, "_best_slice_price", AsyncMock(return_value=None))
    monkeypatch.setattr(
        orderbook,
        "acquire_market_slice_lock",
        AsyncMock(side_effect=[RetryableMarketTransactionError("40001"), None]),
    )
    empty = MagicMock()
    empty.scalar_one_or_none.return_value = None
    product = MagicMock()
    product.scalars.return_value.first.return_value = SimpleNamespace(
        id=caller.product_id, market_product="UCOME_B100"
    )
    delivery = MagicMock()
    delivery.scalars.return_value.first.return_value = SimpleNamespace(
        id=caller.delivery_point_id
    )
    db = AsyncMock()
    db.execute.side_effect = [empty, product, delivery, empty, product, delivery]
    captured = []
    db.add = Mock(
        side_effect=lambda value: (
            captured.append(value) if isinstance(value, OrderBookOrder) else None
        )
    )
    db.flush.side_effect = RuntimeError("capture-persisted-order")
    request = SimpleNamespace(
        headers={"Idempotency-Key": "stable-retry"},
        client=SimpleNamespace(host="127.0.0.1"),
    )
    with pytest.raises(RuntimeError, match="capture-persisted-order"):
        await orderbook.create_order(
            request=request, order_data=caller, current_user=user, db=db
        )
    db.rollback.assert_awaited_once()
    assert caller.model_dump(mode="json") == original_payload
    assert captured[0].idempotency_request_hash == expected_hash

    # Replay the original external payload against the row produced by retry.
    replay = MagicMock()
    replay.scalar_one_or_none.return_value = captured[0]
    db.execute.side_effect = None
    db.execute.return_value = replay
    monkeypatch.setattr(
        orderbook, "_replay_belongs_to_party", AsyncMock(return_value=True)
    )
    monkeypatch.setattr(
        orderbook, "_order_response", AsyncMock(return_value="same-order")
    )
    assert (
        await orderbook.create_order(
            request=request,
            order_data=OrderCreate.model_validate(original_payload),
            current_user=user,
            db=db,
        )
        == "same-order"
    )
