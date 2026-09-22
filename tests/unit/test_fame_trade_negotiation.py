"""Fuel snapshots must not drift or reveal supplier identity before handoff."""

from copy import deepcopy
from datetime import UTC, datetime
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError

from app.market_catalog import PRODUCT_IDS
from app.models.orderbook import OrderSide, TradeStatus
from app.routers import negotiations, trades
from app.schemas.negotiation import NegotiationCounterRequest, NegotiationCreateRequest
from tests.unit.test_fame_manual_trade_routes import (
    fuel_pair as fuel_pair,  # noqa: PLC0414
)
from tests.unit.test_trade_anonymous import _trade


def test_confirmation_rejects_changed_resting_fuel_terms(monkeypatch):
    snapshot = {
        "schema_version": 1,
        "bid": {"side": "BID"},
        "ask": {"side": "ASK", "cfpp_c": -10},
    }
    trade = SimpleNamespace(
        product_id=uuid4(), availability_window="SPOT", fame_terms_snapshot=snapshot
    )
    order = SimpleNamespace(
        product_id=trade.product_id,
        availability_window="SPOT",
        side=OrderSide.ASK,
        fame_terms={"side": "ASK", "cfpp_c": -5},
    )
    monkeypatch.setattr(
        trades,
        "validate_fame_trade_snapshot",
        lambda *args: deepcopy(snapshot),
        raising=False,
    )
    monkeypatch.setattr(
        trades,
        "validate_fame_order_terms",
        lambda *args: deepcopy(order.fame_terms),
        raising=False,
    )
    monkeypatch.setattr(trades, "order_is_execution_qualified", lambda _: True)

    with pytest.raises(HTTPException) as error:
        trades._revalidate_trade_fame_terms(trade, order)

    assert error.value.status_code == 409
    assert trade.fame_terms_snapshot == snapshot


def test_confirmation_rechecks_current_certificate_validity(monkeypatch):
    trade = SimpleNamespace(
        product_id=uuid4(), availability_window="SPOT", fame_terms_snapshot={}
    )

    def expired_certificate(*args):
        raise HTTPException(
            status_code=422, detail="Certificate no longer covers delivery"
        )

    monkeypatch.setattr(
        trades, "validate_fame_trade_snapshot", expired_certificate, raising=False
    )
    with pytest.raises(HTTPException) as error:
        trades._revalidate_trade_fame_terms(trade, None)
    assert error.value.status_code == 409


@pytest.mark.parametrize(
    "status,viewer_side,revealed",
    [
        (TradeStatus.PENDING_CONFIRMATION, "buyer", False),
        (TradeStatus.PENDING_CONFIRMATION, "seller", True),
        (TradeStatus.CONFIRMED, "buyer", True),
        (TradeStatus.DECLINED, "buyer", False),
        (TradeStatus.CONFIRMED, "stranger", False),
    ],
)
def test_trade_fuel_uses_the_existing_identity_handoff(
    monkeypatch, status, viewer_side, revealed
):
    trade, buyer_id, seller_id = _trade(status)
    trade.fame_terms_snapshot = {"private": "supplier certificate holder"}
    calls = []

    def project(snapshot, *, reveal_supplier_identity):
        calls.append((snapshot, reveal_supplier_identity))

    monkeypatch.setattr(trades, "redact_fame_trade_snapshot", project, raising=False)
    viewer_id = {"buyer": buyer_id, "seller": seller_id, "stranger": uuid4()}[
        viewer_side
    ]
    trades.build_trade_response(trade, viewer_org_id=viewer_id)
    assert calls == [(trade.fame_terms_snapshot, revealed)]


def test_one_order_negotiation_requires_the_missing_side_terms(monkeypatch):
    order = SimpleNamespace(side=OrderSide.ASK, product_id=uuid4())
    payload = SimpleNamespace(fame_terms=None)

    def require_missing_side(resting_order, terms):
        assert resting_order is order
        assert terms is None
        raise HTTPException(
            status_code=422, detail="B100 buyer fuel terms are required"
        )

    monkeypatch.setattr(
        negotiations,
        "require_fame_counterparty_terms",
        require_missing_side,
        raising=False,
    )
    with pytest.raises(HTTPException) as error:
        negotiations._negotiation_fame_snapshot(payload, None, order)
    assert error.value.status_code == 422


def test_two_order_negotiation_cannot_override_linked_fuel_terms():
    payload = SimpleNamespace(fame_terms={"side": "BID"})
    with pytest.raises(HTTPException) as error:
        negotiations._negotiation_fame_snapshot(
            payload, SimpleNamespace(product_id=uuid4()), SimpleNamespace()
        )
    assert error.value.status_code == 422


def test_two_order_negotiation_uses_the_locked_ask_as_counterparty_terms(monkeypatch):
    bid = SimpleNamespace(side=OrderSide.BID, product_id=uuid4())
    ask = SimpleNamespace(side=OrderSide.ASK, fame_terms={"side": "ASK", "cfpp_c": -10})
    expected = {"schema_version": 1, "bid": {"side": "BID"}, "ask": ask.fame_terms}

    def snapshot(resting_order, terms):
        assert resting_order is bid
        assert terms == ask.fame_terms
        return deepcopy(expected)

    monkeypatch.setattr(
        negotiations, "require_fame_counterparty_terms", snapshot, raising=False
    )
    actual = negotiations._negotiation_fame_snapshot(
        SimpleNamespace(fame_terms=None), bid, ask
    )
    assert actual == expected


def test_supplier_negotiation_requires_declarations_for_an_unlinked_ask(fuel_pair):
    bid_terms, ask_terms = fuel_pair
    bid_order = SimpleNamespace(
        product_id=PRODUCT_IDS["UCOME_B100"],
        side=OrderSide.BID,
        availability_window="SPOT",
        fame_terms=bid_terms.model_dump(mode="json"),
    )
    payload = NegotiationCreateRequest(
        bid_order_id=uuid4(),
        counterparty_org_id=uuid4(),
        product_id=bid_order.product_id,
        delivery_point_id=uuid4(),
        quantity_mt="0.50",
        proposed_price=900,
        fame_terms=ask_terms,
    )
    with pytest.raises(HTTPException) as error:
        negotiations._negotiation_fame_snapshot(payload, bid_order, None)
    assert error.value.status_code == 422
    acknowledged = payload.model_copy(
        update={"certification_declared": True, "msds_available": True}
    )
    snapshot = negotiations._negotiation_fame_snapshot(acknowledged, bid_order, None)
    assert snapshot["bid"] == bid_terms.model_dump(mode="json")
    assert snapshot["ask"] == ask_terms.model_dump(mode="json")


def test_counter_offer_cannot_change_frozen_fuel_terms(fuel_pair):
    with pytest.raises(ValidationError, match="Extra inputs"):
        NegotiationCounterRequest(proposed_price=900, fame_terms=fuel_pair[0])


@pytest.mark.asyncio
async def test_negotiation_fuel_only_reveals_the_suppliers_own_declaration(monkeypatch):
    buyer, seller = uuid4(), uuid4()
    now = datetime.now(UTC)
    neg = SimpleNamespace(
        id=uuid4(),
        bid_order_id=None,
        ask_order_id=uuid4(),
        initiator_org_id=buyer,
        counterparty_org_id=seller,
        initiator_user_id=uuid4(),
        counterparty_user_id=uuid4(),
        accepted_by_user_id=None,
        initiator_side="BUYER",
        product_id=uuid4(),
        delivery_point_id=uuid4(),
        availability_window="SPOT",
        quantity_mt=100,
        current_price=900,
        status=negotiations.NegotiationStatus.OPEN,
        last_actor_org_id=buyer,
        trade_id=None,
        expires_at=now,
        created_at=now,
        updated_at=now,
        rounds=[],
        fame_terms_snapshot={"private": "site"},
    )
    result = MagicMock()
    result.scalar_one_or_none.return_value = "B100"
    db = AsyncMock()
    db.execute.return_value = result
    monkeypatch.setattr(negotiations, "_batch_org_names", AsyncMock(return_value={}))
    calls = []

    def project(snapshot, *, reveal_supplier_identity):
        calls.append(reveal_supplier_identity)

    monkeypatch.setattr(
        negotiations, "redact_fame_trade_snapshot", project, raising=False
    )
    await negotiations._build_response(db, neg, viewer_org_id=buyer)
    await negotiations._build_response(db, neg, viewer_org_id=seller)
    assert calls == [False, True]
