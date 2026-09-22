"""B100 shared-orderbook routes with the restricted PostgreSQL application role."""

from copy import deepcopy
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import UUID, uuid4

import pytest
from sqlalchemy import func, select

from app.market_catalog import PRODUCTS_BY_CODE
from app.models.negotiation import Negotiation
from app.models.orderbook import OrderBookOrder, Trade, TradeStatus
from app.models.user import Organization, User, UserRole, UserStatus
from app.schemas.fame_order import FameAskTerms, FameBidTerms
from tests.postgres.test_fame_rfq import _headers
from tests.postgres.test_fame_rfq import fame_market as fame_market  # noqa: PLC0414
from tests.postgres.test_supplier_offers import _payload as _supplier_payload


def _ask_terms(seeded):
    return FameAskTerms.model_validate(
        {
            **_supplier_payload(seeded)["fuel_terms"],
            "side": "ASK",
            "evidence_due": "BEFORE_LOADING",
        }
    ).model_dump(mode="json")


def _bid_terms(**changes):
    return FameBidTerms.model_validate(
        {
            "side": "BID",
            "neat_fame": True,
            "standard": "EN_14214",
            "standard_edition": "2012+A2:2019",
            "max_cfpp_c": "0",
            "sustainability_scheme": "ISCC_EU",
            **changes,
        }
    ).model_dump(mode="json")


def _order_payload(seeded, *, side="ASK", quantity="100.00", price="1000.00"):
    payload = {
        "side": side,
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": quantity,
        "price_per_mt_usd": price,
        "availability_window": "SPOT",
        "expires_at": (datetime.now(UTC) + timedelta(hours=24)).isoformat(),
        "fame_terms": _ask_terms(seeded) if side == "ASK" else _bid_terms(),
    }
    if side == "ASK":
        payload.update(certification_declared=True, msds_available=True)
    return payload


async def _post_order(client, seeded, *, payload=None, user_id=None, key=None):
    payload = payload or _order_payload(seeded)
    response = await client.post(
        "/api/orderbook",
        json=payload,
        headers={
            **_headers(
                user_id
                or seeded["seller_id" if payload["side"] == "ASK" else "buyer_id"]
            ),
            "Idempotency-Key": key or str(uuid4()),
        },
    )
    assert response.status_code == 201, response.text
    return response.json()


async def _stored_orders_and_trades(seeded):
    async with seeded["owner_factory"]() as session:
        return (
            {
                str(order.id): order
                for order in (await session.scalars(select(OrderBookOrder))).all()
            },
            list((await session.scalars(select(Trade))).all()),
        )


def _assert_public_fuel(fuel):
    assert fuel["side"] == "ASK"
    assert fuel["standard"] == "EN_14214"
    assert "PRIVATE-" not in str(fuel)
    assert "certificate_holder" not in fuel
    assert "batch_reference" not in fuel
    assert "laboratory" not in fuel["quality_evidence"]
    assert "reference" not in fuel["sustainability_evidence"]


@pytest.mark.asyncio
async def test_b100_post_public_discovery_and_owner_edit_preserve_typed_terms(
    fame_market,
):
    client, seeded = fame_market
    payload = _order_payload(seeded)
    created = await _post_order(client, seeded, payload=payload)
    _assert_public_fuel(created["fame_terms"])
    public = await client.get(
        "/api/orderbook/asks", params={"product_id": str(seeded["product_id"])}
    )
    assert public.status_code == 200, public.text
    assert public.json()["total"] == 1
    _assert_public_fuel(public.json()["items"][0]["fame_terms"])
    mine = await client.get("/api/orderbook/my", headers=_headers(seeded["seller_id"]))
    assert mine.status_code == 200, mine.text
    assert mine.json()[0]["fame_terms"] == payload["fame_terms"]
    changed = deepcopy(payload["fame_terms"])
    changed["cfpp_c"] = "-5"
    changed["certificate_reference"] = "PRIVATE-REVISED-CERTIFICATE"
    updated = await client.put(
        f"/api/orderbook/{created['id']}",
        json={"fame_terms": changed},
        headers=_headers(seeded["seller_id"]),
    )
    assert updated.status_code == 200, updated.text
    _assert_public_fuel(updated.json()["fame_terms"])
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders[created["id"]].fame_terms == changed
    assert trades == []


@pytest.mark.asyncio
@pytest.mark.parametrize("invalid_kind", ["missing", "wrong_side", "other_fuel"])
async def test_b100_order_terms_fail_closed_before_any_write(fame_market, invalid_kind):
    client, seeded = fame_market
    payload = _order_payload(seeded)
    if invalid_kind == "missing":
        payload.pop("fame_terms")
    elif invalid_kind == "wrong_side":
        payload["fame_terms"] = _bid_terms()
    else:
        payload["product_id"] = str(PRODUCTS_BY_CODE["BIO_METHANOL"].id)
    response = await client.post(
        "/api/orderbook", json=payload, headers=_headers(seeded["seller_id"])
    )
    assert response.status_code == 422, response.text
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders == {}
    assert trades == []


@pytest.mark.asyncio
async def test_b100_matching_skips_incompatible_bid_and_freezes_compatible_fill(
    fame_market,
):
    client, seeded = fame_market
    ask = await _post_order(client, seeded)
    incompatible = _order_payload(seeded, side="BID", quantity="50.00", price="1200.00")
    incompatible["fame_terms"] = _bid_terms(max_cfpp_c="-20")
    rejected_match = await _post_order(client, seeded, payload=incompatible)
    orders, trades = await _stored_orders_and_trades(seeded)
    assert trades == []
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(100)
    compatible = _order_payload(seeded, side="BID", quantity="40.00", price="1100.00")
    filled = await _post_order(client, seeded, payload=compatible)
    orders, trades = await _stored_orders_and_trades(seeded)
    assert len(trades) == 1
    trade = trades[0]
    assert trade.status == TradeStatus.CONFIRMED
    assert trade.quantity_mt == Decimal(40)
    assert trade.price_per_mt_usd == Decimal(1000)
    assert trade.fame_terms_snapshot == {
        "schema_version": 1,
        "bid": compatible["fame_terms"],
        "ask": _ask_terms(seeded),
    }
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(60)
    assert orders[filled["id"]].remaining_quantity_mt == Decimal(0)
    assert orders[rejected_match["id"]].remaining_quantity_mt == Decimal(50)


@pytest.mark.asyncio
async def test_b100_manual_take_replays_once_and_rejects_changed_source_at_confirmation(
    fame_market,
):
    client, seeded = fame_market
    ask = await _post_order(client, seeded)
    headers = {**_headers(seeded["buyer_id"]), "Idempotency-Key": str(uuid4())}
    missing = await client.post(
        "/api/trades/",
        json={
            "order_id": ask["id"],
            "quantity_mt": "25.00",
            "expected_order_version": ask["version"],
        },
        headers=_headers(seeded["buyer_id"]),
    )
    assert missing.status_code == 422, missing.text
    payload = {
        "order_id": ask["id"],
        "quantity_mt": "25.00",
        "fame_terms": _bid_terms(),
        "expected_order_version": ask["version"],
    }
    created = await client.post("/api/trades/", json=payload, headers=headers)
    assert created.status_code == 200, created.text
    body = created.json()
    assert body["seller_id"] is None
    _assert_public_fuel(body["fame_terms_snapshot"]["ask"])
    replay = await client.post("/api/trades/", json=payload, headers=headers)
    assert replay.status_code == 200, replay.text
    assert replay.json()["id"] == body["id"]
    stale_new_request = await client.post(
        "/api/trades/",
        json=payload,
        headers={**_headers(seeded["buyer_id"]), "Idempotency-Key": str(uuid4())},
    )
    assert stale_new_request.status_code == 409, stale_new_request.text
    conflict = await client.post(
        "/api/trades/", json={**payload, "quantity_mt": "26.00"}, headers=headers
    )
    assert conflict.status_code == 409, conflict.text
    orders, trades = await _stored_orders_and_trades(seeded)
    assert len(trades) == 1
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(75)
    changed = _ask_terms(seeded)
    changed["cfpp_c"] = "-5"
    update = await client.put(
        f"/api/orderbook/{ask['id']}",
        json={"fame_terms": changed},
        headers=_headers(seeded["seller_id"]),
    )
    assert update.status_code == 200, update.text
    confirmed = await client.put(
        f"/api/trades/{body['id']}/confirm", headers=_headers(seeded["seller_id"])
    )
    assert confirmed.status_code == 409, confirmed.text
    _, trades = await _stored_orders_and_trades(seeded)
    assert trades[0].status == TradeStatus.PENDING_CONFIRMATION
    assert trades[0].fame_terms_snapshot["ask"]["cfpp_c"] == "-2"


@pytest.mark.asyncio
async def test_b100_supplier_manual_take_requires_acknowledgements_then_confirms(
    fame_market,
):
    client, seeded = fame_market
    bid = await _post_order(client, seeded, payload=_order_payload(seeded, side="BID"))
    payload = {
        "order_id": bid["id"],
        "quantity_mt": "25.00",
        "fame_terms": _ask_terms(seeded),
        "expected_order_version": bid["version"],
    }
    rejected = await client.post(
        "/api/trades/", json=payload, headers=_headers(seeded["seller_id"])
    )
    assert rejected.status_code == 422, rejected.text
    created = await client.post(
        "/api/trades/",
        json={**payload, "certification_declared": True, "msds_available": True},
        headers=_headers(seeded["seller_id"]),
    )
    assert created.status_code == 200, created.text
    assert created.json()["fame_terms_snapshot"]["ask"] == payload["fame_terms"]
    confirmed = await client.put(
        f"/api/trades/{created.json()['id']}/confirm",
        headers=_headers(seeded["buyer_id"]),
    )
    assert confirmed.status_code == 200, confirmed.text
    assert confirmed.json()["status"] == "CONFIRMED"
    assert confirmed.json()["seller_id"] == str(seeded["seller_org_id"])
    assert confirmed.json()["fame_terms_snapshot"]["ask"] == payload["fame_terms"]


@pytest.mark.asyncio
async def test_b100_order_idempotency_and_admission_do_not_expose_private_declarations(
    fame_market,
):
    client, seeded = fame_market
    async with seeded["owner_factory"]() as session:
        peer = User(
            email=f"b100-peer-{uuid4()}@route.test",
            password_hash="unused",
            role=UserRole.SUPPLIER,
            status=UserStatus.APPROVED,
            email_verified=True,
            kyc_status="APPROVED",
            organization_id=seeded["seller_org_id"],
        )
        session.add(peer)
        await session.commit()
        peer_id = peer.id
    key = str(uuid4())
    payload = _order_payload(seeded)
    ask = await _post_order(client, seeded, payload=payload, key=key)
    replay = await _post_order(client, seeded, payload=payload, key=key)
    assert replay["id"] == ask["id"]
    _assert_public_fuel(replay["fame_terms"])
    foreign_actor = await client.post(
        "/api/orderbook",
        json=payload,
        headers={**_headers(peer_id), "Idempotency-Key": key},
    )
    assert foreign_actor.status_code == 409, foreign_actor.text
    assert "PRIVATE-" not in foreign_actor.text
    altered = deepcopy(payload)
    altered["fame_terms"]["cfpp_c"] = "-5"
    conflict = await client.post(
        "/api/orderbook",
        json=altered,
        headers={**_headers(seeded["seller_id"]), "Idempotency-Key": key},
    )
    assert conflict.status_code == 409, conflict.text
    other_tenant = await client.put(
        f"/api/orderbook/{ask['id']}",
        json={"fame_terms": altered["fame_terms"]},
        headers=_headers(seeded["other_seller_id"]),
    )
    assert other_tenant.status_code == 403, other_tenant.text
    assert "PRIVATE-" not in other_tenant.text
    async with seeded["owner_factory"]() as session:
        org = await session.get(Organization, seeded["seller_org_id"])
        org.verification_status = "REJECTED"
        await session.commit()
    public = await client.get("/api/orderbook/asks")
    assert public.status_code == 200, public.text
    assert public.json()["items"] == []
    take = await client.post(
        "/api/trades/",
        json={
            "order_id": ask["id"],
            "quantity_mt": "25.00",
            "fame_terms": _bid_terms(),
            "expected_order_version": ask["version"],
        },
        headers=_headers(seeded["buyer_id"]),
    )
    assert take.status_code == 409, take.text
    assert "PRIVATE-" not in take.text
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(100)
    assert trades == []


@pytest.mark.asyncio
async def test_b100_negotiation_freezes_pair_and_only_price_can_change(fame_market):
    client, seeded = fame_market
    ask = await _post_order(client, seeded)
    payload = {
        "ask_order_id": ask["id"],
        "counterparty_org_id": str(seeded["seller_org_id"]),
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "25.00",
        "proposed_price": "950.00",
        "fame_terms": _bid_terms(),
    }
    created = await client.post(
        "/api/negotiations", json=payload, headers=_headers(seeded["buyer_id"])
    )
    assert created.status_code == 201, created.text
    body = created.json()
    _assert_public_fuel(body["fame_terms_snapshot"]["ask"])
    counter = await client.post(
        f"/api/negotiations/{body['id']}/counter",
        json={"proposed_price": "975.00"},
        headers=_headers(seeded["seller_id"]),
    )
    assert counter.status_code == 200, counter.text
    assert counter.json()["fame_terms_snapshot"]["ask"] == _ask_terms(seeded)
    fuel_change = await client.post(
        f"/api/negotiations/{body['id']}/counter",
        json={"proposed_price": "980.00", "fame_terms": _bid_terms(max_cfpp_c="-20")},
        headers=_headers(seeded["buyer_id"]),
    )
    assert fuel_change.status_code == 422, fuel_change.text
    disabled = await client.post(
        f"/api/negotiations/{body['id']}/accept", headers=_headers(seeded["buyer_id"])
    )
    assert disabled.status_code == 409, disabled.text
    async with seeded["owner_factory"]() as session:
        negotiation = await session.get(Negotiation, UUID(body["id"]))
        assert negotiation.fame_terms_snapshot == {
            "schema_version": 1,
            "bid": payload["fame_terms"],
            "ask": _ask_terms(seeded),
        }
        assert negotiation.current_price == Decimal(975)
        assert await session.scalar(select(func.count()).select_from(Trade)) == 0


@pytest.mark.asyncio
async def test_b100_partial_fills_can_finish_a_remainder_below_one_mt(fame_market):
    client, seeded = fame_market
    ask = await _post_order(
        client, seeded, payload=_order_payload(seeded, quantity="1.50")
    )
    await _post_order(
        client,
        seeded,
        payload=_order_payload(seeded, side="BID", quantity="1.00", price="1100.00"),
    )
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders[ask["id"]].remaining_quantity_mt == Decimal("0.50")
    assert len(trades) == 1
    negotiation = await client.post(
        "/api/negotiations",
        headers=_headers(seeded["buyer_id"]),
        json={
            "ask_order_id": ask["id"],
            "counterparty_org_id": str(seeded["seller_org_id"]),
            "product_id": str(seeded["product_id"]),
            "delivery_point_id": str(seeded["point_id"]),
            "quantity_mt": "0.50",
            "proposed_price": "975.00",
            "fame_terms": _bid_terms(),
        },
    )
    assert negotiation.status_code == 201, negotiation.text
    created = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={
            "order_id": ask["id"],
            "quantity_mt": "0.50",
            "fame_terms": _bid_terms(),
            "expected_order_version": orders[ask["id"]].version,
        },
    )
    assert created.status_code == 200, created.text
    confirmed = await client.put(
        f"/api/trades/{created.json()['id']}/confirm",
        headers=_headers(seeded["seller_id"]),
    )
    assert confirmed.status_code == 200, confirmed.text
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(0)
    assert sum(trade.quantity_mt for trade in trades) == Decimal("1.50")
    assert all(
        trade.fame_terms_snapshot["ask"] == _ask_terms(seeded) for trade in trades
    )


@pytest.mark.asyncio
async def test_b100_manual_take_rejects_unreviewed_source_revision(fame_market):
    client, seeded = fame_market
    ask = await _post_order(client, seeded)
    changed = _ask_terms(seeded)
    changed["cfpp_c"] = "-5"
    revised = await client.put(
        f"/api/orderbook/{ask['id']}",
        json={"fame_terms": changed},
        headers=_headers(seeded["seller_id"]),
    )
    assert revised.status_code == 200, revised.text
    assert revised.json()["version"] > ask["version"]
    payload = {
        "order_id": ask["id"],
        "quantity_mt": "25.00",
        "fame_terms": _bid_terms(),
    }
    missing = await client.post(
        "/api/trades/", json=payload, headers=_headers(seeded["buyer_id"])
    )
    assert missing.status_code == 422, missing.text
    stale = await client.post(
        "/api/trades/",
        json={**payload, "expected_order_version": ask["version"]},
        headers=_headers(seeded["buyer_id"]),
    )
    assert stale.status_code == 409, stale.text
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(100)
    assert trades == []
    current = await client.post(
        "/api/trades/",
        json={**payload, "expected_order_version": revised.json()["version"]},
        headers=_headers(seeded["buyer_id"]),
    )
    assert current.status_code == 200, current.text
    orders, trades = await _stored_orders_and_trades(seeded)
    assert orders[ask["id"]].remaining_quantity_mt == Decimal(75)
    assert len(trades) == 1
    assert trades[0].fame_terms_snapshot["ask"] == changed
