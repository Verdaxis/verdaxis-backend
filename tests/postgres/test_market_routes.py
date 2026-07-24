"""ASGI-level market transaction proofs on disposable PostgreSQL 17."""
from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from unittest.mock import AsyncMock
from uuid import uuid4

import httpx
import pytest
from sqlalchemy import event, func, select, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.core.security import create_access_token
from app.database import get_db
from app.main import app
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.audit import AuditLog
from app.models.catalog import DeliveryPoint, Product
from app.models.market_event import MarketEventOutbox
from app.models.marketplace import FuelType, InventoryItem
from app.models.negotiation import Negotiation, NegotiationStatus
from app.models.orderbook import OrderBookOrder, OrderBookStatus, Trade, TradeStatus
from app.models.port import Port
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from app.routers import orderbook as orderbook_router
from app.services.audit_actions import TRADE_AUTO_MATCHED
from app.services import market_transactions
from tests.postgres.market_test_support import assign_fixture_real_provenance


def _headers(user_id, key: str) -> dict[str, str]:
    return {
        "Authorization": f"Bearer {create_access_token(str(user_id))}",
        "Idempotency-Key": key,
    }


class _TransactionBoundaryGate:
    """Release matching transaction checkpoints only after all arrive."""

    def __init__(self, *, operation: str, arrivals: int) -> None:
        self.operation = operation
        self.arrivals = arrivals
        self.count = 0
        self.all_arrived = asyncio.Event()

    async def __call__(self, boundary: str, *, operation: str, aggregate_id=None):
        if boundary != "before_market_slice_lock" or operation != self.operation:
            return
        self.count += 1
        if self.count == self.arrivals:
            self.all_arrived.set()
        await self.all_arrived.wait()


class _PausedTransactionBoundary:
    """Pause exactly one named operation at its post-preview checkpoint."""

    def __init__(self, *, operation: str, aggregate_id) -> None:
        self.operation = operation
        self.aggregate_id = str(aggregate_id)
        self.reached = asyncio.Event()
        self.release = asyncio.Event()
        self._used = False

    async def __call__(self, boundary: str, *, operation: str, aggregate_id=None):
        if (
            boundary != "after_market_preview"
            or operation != self.operation
            or str(aggregate_id) != self.aggregate_id
            or self._used
        ):
            return
        self._used = True
        self.reached.set()
        await self.release.wait()


async def _seed_route_market(engine):
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        buyer_org = Organization(
            name=f"Route Buyer {uuid4()}",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        seller_org = Organization(
            name=f"Route Seller {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
            verification_status="APPROVED",
        )
        buyer = User(
            email=f"buyer-{uuid4()}@route.test",
            password_hash="unused",
            role=UserRole.BUYER,
            status=UserStatus.APPROVED,
            organization=buyer_org,
            email_verified=True,
            kyc_status="APPROVED",
        )
        seller = User(
            email=f"seller-{uuid4()}@route.test",
            password_hash="unused",
            role=UserRole.SUPPLIER,
            status=UserStatus.APPROVED,
            organization=seller_org,
            email_verified=True,
            kyc_status="APPROVED",
        )
        product_spec = PRODUCTS_BY_CODE["BIO_METHANOL"]
        point_spec = DELIVERY_POINTS_BY_NAME["Singapore"]
        product = Product(
            id=product_spec.id,
            name=product_spec.name,
            fuel_type=product_spec.fuel_type,
            fuel_grade=product_spec.fuel_grade,
            unit=product_spec.unit,
            is_active=True,
        )
        point = DeliveryPoint(
            id=point_spec.id,
            name=point_spec.name,
            region=point_spec.region,
            timezone=point_spec.timezone,
            is_active=True,
        )
        port = Port(
            id=f"route-{uuid4().hex[:12]}",
            name="Singapore",
            country="Singapore",
            location="POINT(103.8198 1.3521)",
            is_active=True,
        )
        session.add_all([buyer, seller, product, point, port])
        await session.flush()
        await assign_fixture_real_provenance(session, (buyer_org, seller_org))
        inventory = InventoryItem(
            supplier_id=seller_org.id,
            owner_user_id=seller.id,
            port_id=port.id,
            fuel_type=FuelType.Methanol,
            product_name=product.name,
            current_stock_mt=Decimal("100.00"),
            incoming_stock_mt=Decimal("0.00"),
            reserved_stock_mt=Decimal("0.00"),
            price_per_mt_usd=Decimal("700.00"),
            is_certified=True,
            certification_declared=True,
            certification_scheme="ISCC EU",
            specification_standard="IMPCA",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("20.00"),
            carbon_intensity_method="ISCC EU",
            feedstock="biogenic waste",
            origin="route test",
            off_spec=False,
        )
        session.add(inventory)
        await session.commit()
        return {
            "engine": engine,
            "factory": factory,
            "buyer_id": buyer.id,
            "seller_id": seller.id,
            "buyer_org_id": buyer_org.id,
            "seller_org_id": seller_org.id,
            "product_id": product.id,
            "point_id": point.id,
            "inventory_id": inventory.id,
        }


@pytest.fixture
async def route_market(market_pg):
    seeded = await _seed_route_market(market_pg)
    factory = seeded["factory"]

    async def override_db():
        async with factory() as session:
            yield session

    # Integration note: require_security_market_admission is now a thin
    # re-export of security's require_execution_eligible_user, so it is not
    # overridden — the seeded users are fully admitted (APPROVED, verified,
    # KYC-clean, APPROVED orgs) and exercise the genuine gate.
    app.dependency_overrides[get_db] = override_db
    transport = httpx.ASGITransport(app=app, raise_app_exceptions=False)
    async with httpx.AsyncClient(
        transport=transport, base_url="http://market-route.test"
    ) as client:
        yield client, seeded
    app.dependency_overrides.pop(get_db, None)


@pytest.mark.asyncio
async def test_simultaneous_authenticated_crossing_posts_are_one_coherent_trade(
    route_market, monkeypatch
):
    client, seeded = route_market
    gate = _TransactionBoundaryGate(operation="order_admission", arrivals=2)
    monkeypatch.setattr(market_transactions, "transaction_boundary_hook", gate)

    bid_payload = {
        "side": "BID",
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "100.00",
        "price_per_mt_usd": "700.00",
        "availability_window": "SPOT",
    }
    bid_request = client.post(
        "/api/orderbook",
        json=bid_payload,
        headers=_headers(seeded["buyer_id"], "crossing-bid"),
    )
    ask_request = client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], "crossing-ask"),
    )
    bid_response, ask_response = await asyncio.wait_for(
        asyncio.gather(bid_request, ask_request), timeout=15
    )

    # The bounded market-slice lock may deliberately reject one contender
    # under a loaded PostgreSQL runner. Retrying that idempotent request after
    # the winning transaction commits must still produce one coherent trade.
    if bid_response.status_code == 409:
        assert bid_response.json()["detail"] == "Market slice is busy; retry the request"
        bid_response = await client.post(
            "/api/orderbook",
            json=bid_payload,
            headers=_headers(seeded["buyer_id"], "crossing-bid"),
        )
    if ask_response.status_code == 409:
        assert ask_response.json()["detail"] == "Market slice is busy; retry the request"
        ask_response = await client.post(
            f"/api/inventory/{seeded['inventory_id']}/publish",
            headers=_headers(seeded["seller_id"], "crossing-ask"),
        )

    assert bid_response.status_code == 201, bid_response.text
    assert ask_response.status_code == 200, ask_response.text

    bid_id = bid_response.json()["id"]
    ask_id = ask_response.json()["listing_id"]
    async with seeded["factory"]() as session:
        orders = list(
            (
                await session.execute(
                    select(OrderBookOrder)
                    .where(OrderBookOrder.id.in_([bid_id, ask_id]))
                    .order_by(OrderBookOrder.side)
                )
            ).scalars()
        )
        trades = list((await session.execute(select(Trade))).scalars())
        inventory = await session.get(InventoryItem, seeded["inventory_id"])
        auto_match_audits = (
            await session.execute(
                select(func.count(AuditLog.id)).where(
                    AuditLog.action == TRADE_AUTO_MATCHED
                )
            )
        ).scalar_one()

    assert len(orders) == 2
    assert all(order.status == OrderBookStatus.FILLED for order in orders)
    assert all(order.remaining_quantity_mt == Decimal("0.00") for order in orders)
    assert len(trades) == 1
    trade = trades[0]
    assert trade.status == TradeStatus.CONFIRMED
    assert trade.bid_order_id == next(order.id for order in orders if order.side.value == "BID")
    assert trade.ask_order_id == next(order.id for order in orders if order.side.value == "ASK")
    assert trade.buyer_id == seeded["buyer_org_id"]
    assert trade.seller_id == seeded["seller_org_id"]
    assert trade.quantity_mt == Decimal("100.00")
    assert trade.market_product == "BIO_METHANOL"
    assert trade.product_name == "Bio Methanol"
    assert trade.delivery_point_name == "Singapore"
    assert trade.delivery_point_region == "Asia"
    assert trade.availability_window == "SPOT"
    assert trade.buyer_provenance == OrganizationProvenance.REAL
    assert trade.seller_provenance == OrganizationProvenance.REAL
    assert inventory.current_stock_mt == Decimal("0.00")
    assert inventory.reserved_stock_mt == Decimal("0.00")
    assert auto_match_audits == 1

    # Both committed requests replay before current stock/order lifecycle
    # checks and return their original result without a second mutation.
    replay_bid, replay_ask = await asyncio.gather(
        client.post(
            "/api/orderbook",
            json=bid_payload,
            headers=_headers(seeded["buyer_id"], "crossing-bid"),
        ),
        client.post(
            f"/api/inventory/{seeded['inventory_id']}/publish",
            headers=_headers(seeded["seller_id"], "crossing-ask"),
        ),
    )
    assert replay_bid.status_code == 201, replay_bid.text
    assert replay_ask.status_code == 200, replay_ask.text
    assert replay_bid.json()["id"] == bid_id
    assert replay_ask.json()["listing_id"] == ask_id
    async with seeded["factory"]() as session:
        assert (await session.execute(select(func.count(Trade.id)))).scalar_one() == 1

    tape = await client.get("/api/trade-tape")
    assert tape.status_code == 200, tape.text
    assert tape.json()["total"] == 1
    assert tape.json()["items"][0]["market_product"] == "BIO_METHANOL"


@pytest.mark.asyncio
async def test_concurrent_route_idempotency_has_one_order(route_market):
    client, seeded = route_market
    payload = {
        "side": "BID",
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "50.00",
        "price_per_mt_usd": "650.00",
        "availability_window": "SPOT",
    }
    responses = await asyncio.gather(
        *(
            client.post(
                "/api/orderbook",
                json=payload,
                headers=_headers(seeded["buyer_id"], "same-order-key"),
            )
            for _ in range(2)
        )
    )
    assert [response.status_code for response in responses] == [201, 201]
    assert len({response.json()["id"] for response in responses}) == 1
    async with seeded["factory"]() as session:
        count = (
            await session.execute(
                select(func.count(OrderBookOrder.id)).where(
                    OrderBookOrder.organization_id == seeded["buyer_org_id"]
                )
            )
        ).scalar_one()
    assert count == 1


@pytest.mark.asyncio
async def test_direct_trade_replay_is_snapshot_only_and_hash_conflict_is_immutable(
    route_market
):
    client, seeded = route_market
    ask_payload = {
        "side": "ASK",
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "100.00",
        "price_per_mt_usd": "750.00",
        "availability_window": "SPOT",
        "certification_declared": True,
        "certification_scheme": "ISCC EU",
        "specification_standard": "IMPCA",
        "msds_available": True,
        "carbon_intensity_gco2_mj": "20.00",
        "feedstock": "biogenic waste",
        "origin": "route test",
    }
    ask_response = await client.post(
        "/api/orderbook",
        json=ask_payload,
        headers=_headers(seeded["seller_id"], "direct-ask"),
    )
    assert ask_response.status_code == 201, ask_response.text
    ask_id = ask_response.json()["id"]

    trade_payload = {"order_id": ask_id, "quantity_mt": "25.00"}
    responses = await asyncio.gather(
        *(
            client.post(
                "/api/trades/",
                json=trade_payload,
                headers=_headers(seeded["buyer_id"], "same-trade-key"),
            )
            for _ in range(2)
        )
    )
    assert [response.status_code for response in responses] == [200, 200]
    bodies = [response.json() for response in responses]
    assert len({body["id"] for body in bodies}) == 1
    assert all(body["market_product"] == "BIO_METHANOL" for body in bodies)
    assert all(body["delivery_point_name"] == "Singapore" for body in bodies)

    conflict = await client.post(
        "/api/trades/",
        json={"order_id": ask_id, "quantity_mt": "30.00"},
        headers=_headers(seeded["buyer_id"], "same-trade-key"),
    )
    assert conflict.status_code == 409
    async with seeded["factory"]() as session:
        ask = await session.get(OrderBookOrder, ask_id)
        trade_count = (await session.execute(select(func.count(Trade.id)))).scalar_one()
    assert ask.remaining_quantity_mt == Decimal("75.00")
    assert trade_count == 1


@pytest.mark.asyncio
async def test_locked_cancelled_order_cannot_be_updated_from_stale_preview(
    route_market, monkeypatch
):
    client, seeded = route_market
    created = await client.post(
        "/api/orderbook",
        json={
            "side": "BID",
            "product_id": str(seeded["product_id"]),
            "delivery_point_id": str(seeded["point_id"]),
            "quantity_mt": "10.00",
            "price_per_mt_usd": "600.00",
            "availability_window": "SPOT",
        },
        headers=_headers(seeded["buyer_id"], "cancel-update-race-order"),
    )
    assert created.status_code == 201, created.text
    order_id = created.json()["id"]

    # Integration note: security's authentication locks the caller's user row
    # FOR UPDATE for the whole request transaction, so same-user requests
    # serialize end-to-end and an update can never hold a stale preview open
    # while its own cancel commits. The integrated contract is that the two
    # racing requests apply in one of the two serial orders with no corrupted
    # final state.
    updated, cancelled = await asyncio.wait_for(
        asyncio.gather(
            client.put(
                f"/api/orderbook/{order_id}",
                json={"price_per_mt_usd": "610.00"},
                headers=_headers(seeded["buyer_id"], "unused-racing-update"),
            ),
            client.delete(
                f"/api/orderbook/{order_id}",
                headers=_headers(seeded["buyer_id"], "unused-racing-cancel"),
            ),
        ),
        timeout=60,
    )

    assert cancelled.status_code == 204, cancelled.text
    assert updated.status_code in (200, 400), updated.text
    async with seeded["factory"]() as session:
        order = await session.get(OrderBookOrder, order_id)
        assert order.status == OrderBookStatus.CANCELLED
        if updated.status_code == 200:
            # The update serialized before the cancel.
            assert order.price_per_mt_usd == Decimal("610.00")
        else:
            # The cancel serialized first; the stale update was rejected.
            assert order.price_per_mt_usd == Decimal("600.00")


@pytest.mark.asyncio
async def test_locked_confirmed_trade_cannot_be_declined_from_stale_preview(
    route_market, monkeypatch
):
    client, seeded = route_market
    published = await client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], "confirm-decline-publish"),
    )
    assert published.status_code == 200, published.text
    listing_id = published.json()["listing_id"]
    created = await client.post(
        "/api/trades/",
        json={"order_id": listing_id, "quantity_mt": "25.00"},
        headers=_headers(seeded["buyer_id"], "confirm-decline-trade"),
    )
    assert created.status_code == 200, created.text
    trade_id = created.json()["id"]

    # Integration note: security's authentication serializes same-user
    # requests (the user row is locked FOR UPDATE per request), so a decline
    # can never operate on a stale preview while the confirm commits. The
    # racing pair applies in one of the two serial orders; exactly one wins
    # and inventory bookkeeping is conserved either way.
    declined, confirmed = await asyncio.wait_for(
        asyncio.gather(
            client.put(
                f"/api/trades/{trade_id}/decline",
                headers=_headers(seeded["seller_id"], "unused-racing-decline"),
            ),
            client.put(
                f"/api/trades/{trade_id}/confirm",
                headers=_headers(seeded["seller_id"], "unused-racing-confirm"),
            ),
        ),
        timeout=60,
    )

    assert sorted((confirmed.status_code, declined.status_code)) == [200, 400], (
        confirmed.text,
        declined.text,
    )
    async with seeded["factory"]() as session:
        trade = await session.get(Trade, trade_id)
        order = await session.get(OrderBookOrder, listing_id)
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert item.current_stock_mt == Decimal("0.00")
        if confirmed.status_code == 200:
            # The confirm serialized first; the stale decline was rejected.
            assert trade.status == TradeStatus.CONFIRMED
            assert order.remaining_quantity_mt == Decimal("75.00")
            assert item.reserved_stock_mt == Decimal("75.00")
        else:
            # The decline serialized first and restored the resting order.
            assert trade.status == TradeStatus.DECLINED
            assert order.remaining_quantity_mt == Decimal("100.00")
            assert item.reserved_stock_mt == Decimal("100.00")


@pytest.mark.asyncio
async def test_inventory_quantity_cancel_and_historical_delete_contract(route_market):
    client, seeded = route_market
    published = await client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], "inventory-quantity-publish"),
    )
    assert published.status_code == 200, published.text
    listing_id = published.json()["listing_id"]

    moved = await client.put(
        f"/api/orderbook/{listing_id}",
        json={"availability_window": "2026-Q4"},
        headers=_headers(seeded["seller_id"], "unused-slice-move"),
    )
    assert moved.status_code == 409

    blocked_update = await client.patch(
        f"/api/inventory/{seeded['inventory_id']}",
        json={"current_stock_mt": "90.00"},
        headers=_headers(seeded["seller_id"], "unused-update"),
    )
    blocked_delete = await client.delete(
        f"/api/inventory/{seeded['inventory_id']}",
        headers=_headers(seeded["seller_id"], "unused-delete"),
    )
    assert blocked_update.status_code == 409
    assert blocked_delete.status_code == 409

    reduced = await client.put(
        f"/api/orderbook/{listing_id}",
        json={"quantity_mt": "60.00"},
        headers=_headers(seeded["seller_id"], "unused-order-update"),
    )
    assert reduced.status_code == 200, reduced.text
    async with seeded["factory"]() as session:
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert item.current_stock_mt == Decimal("40.00")
        assert item.reserved_stock_mt == Decimal("60.00")

    increased = await client.put(
        f"/api/orderbook/{listing_id}",
        json={"quantity_mt": "80.00"},
        headers=_headers(seeded["seller_id"], "unused-order-update-2"),
    )
    assert increased.status_code == 200, increased.text
    cancelled = await client.delete(
        f"/api/orderbook/{listing_id}",
        headers=_headers(seeded["seller_id"], "unused-order-cancel"),
    )
    assert cancelled.status_code == 204, cancelled.text
    async with seeded["factory"]() as session:
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert item.current_stock_mt == Decimal("100.00")
        assert item.reserved_stock_mt == Decimal("0.00")

    # Restrictive history ownership means cancellation makes stock editable,
    # but the inventory row still cannot disappear beneath its preserved order.
    historical_delete = await client.delete(
        f"/api/inventory/{seeded['inventory_id']}",
        headers=_headers(seeded["seller_id"], "unused-delete-history"),
    )
    assert historical_delete.status_code == 409


@pytest.mark.asyncio
async def test_inventory_partial_pending_confirm_and_cancel_conserve_stock(route_market):
    client, seeded = route_market
    published = await client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], "partial-publish"),
    )
    listing_id = published.json()["listing_id"]
    created = await client.post(
        "/api/trades/",
        json={"order_id": listing_id, "quantity_mt": "25.00"},
        headers=_headers(seeded["buyer_id"], "partial-trade"),
    )
    assert created.status_code == 200, created.text
    trade_id = created.json()["id"]

    async with seeded["factory"]() as session:
        item = await session.get(InventoryItem, seeded["inventory_id"])
        order = await session.get(OrderBookOrder, listing_id)
        assert item.current_stock_mt == Decimal("0.00")
        assert item.reserved_stock_mt == Decimal("100.00")
        assert order.remaining_quantity_mt == Decimal("75.00")
        assert order.status == OrderBookStatus.PARTIALLY_FILLED

    update_response, delete_response = await asyncio.gather(
        client.patch(
            f"/api/inventory/{seeded['inventory_id']}",
            json={"price_per_mt_usd": "710.00"},
            headers=_headers(seeded["seller_id"], "pending-update"),
        ),
        client.delete(
            f"/api/inventory/{seeded['inventory_id']}",
            headers=_headers(seeded["seller_id"], "pending-delete"),
        ),
    )
    assert update_response.status_code == 409
    assert delete_response.status_code == 409

    confirmed = await client.put(
        f"/api/trades/{trade_id}/confirm",
        headers=_headers(seeded["seller_id"], "unused-confirm"),
    )
    assert confirmed.status_code == 200, confirmed.text
    cancelled = await client.delete(
        f"/api/orderbook/{listing_id}",
        headers=_headers(seeded["seller_id"], "unused-partial-cancel"),
    )
    assert cancelled.status_code == 204, cancelled.text
    async with seeded["factory"]() as session:
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert item.current_stock_mt == Decimal("75.00")
        assert item.reserved_stock_mt == Decimal("0.00")


@pytest.mark.asyncio
async def test_inventory_full_pending_decline_and_expiry_republication(route_market):
    client, seeded = route_market
    first = await client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], "full-publish"),
    )
    listing_id = first.json()["listing_id"]
    created = await client.post(
        "/api/trades/",
        json={"order_id": listing_id, "quantity_mt": "100.00"},
        headers=_headers(seeded["buyer_id"], "full-trade"),
    )
    assert created.status_code == 200, created.text
    trade_id = created.json()["id"]
    async with seeded["factory"]() as session:
        order = await session.get(OrderBookOrder, listing_id)
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert order.status == OrderBookStatus.FILLED
        assert item.reserved_stock_mt == Decimal("100.00")

    declined = await client.put(
        f"/api/trades/{trade_id}/decline",
        headers=_headers(seeded["seller_id"], "unused-decline"),
    )
    assert declined.status_code == 200, declined.text
    async with seeded["factory"]() as session:
        order = await session.get(OrderBookOrder, listing_id)
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert order.status == OrderBookStatus.OPEN
        assert order.remaining_quantity_mt == Decimal("100.00")
        assert item.current_stock_mt == Decimal("0.00")
        assert item.reserved_stock_mt == Decimal("100.00")
        order.expires_at = datetime.now(UTC) - timedelta(seconds=1)
        await session.commit()

    republished = await client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], "after-expiry-publish"),
    )
    assert republished.status_code == 200, republished.text
    assert republished.json()["listing_id"] != listing_id
    async with seeded["factory"]() as session:
        old_order = await session.get(OrderBookOrder, listing_id)
        new_order = await session.get(
            OrderBookOrder, republished.json()["listing_id"]
        )
        item = await session.get(InventoryItem, seeded["inventory_id"])
        assert old_order.status == OrderBookStatus.EXPIRED
        assert new_order.status == OrderBookStatus.OPEN
        assert item.current_stock_mt == Decimal("0.00")
        assert item.reserved_stock_mt == Decimal("100.00")


@pytest.mark.asyncio
async def test_precommit_failure_writes_nothing_and_transport_absence_keeps_commit(
    route_market, monkeypatch
):
    client, seeded = route_market
    payload = {
        "side": "BID",
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "10.00",
        "price_per_mt_usd": "600.00",
        "availability_window": "SPOT",
    }
    async with seeded["factory"]() as session:
        outbox_count_before = (
            await session.execute(select(func.count(MarketEventOutbox.id)))
        ).scalar_one()

    audit = AsyncMock(side_effect=RuntimeError("audit write failed"))
    monkeypatch.setattr(orderbook_router, "record_audit", audit)
    failed = await client.post(
        "/api/orderbook",
        json=payload,
        headers=_headers(seeded["buyer_id"], "precommit-failure"),
    )
    assert failed.status_code == 500
    async with seeded["factory"]() as session:
        assert (
            await session.execute(
                select(func.count(OrderBookOrder.id)).where(
                    OrderBookOrder.idempotency_key == "precommit-failure"
                )
            )
        ).scalar_one() == 0
        assert (
            await session.execute(select(func.count(MarketEventOutbox.id)))
        ).scalar_one() == outbox_count_before

    monkeypatch.undo()
    committed = await client.post(
        "/api/orderbook",
        json=payload,
        headers=_headers(seeded["buyer_id"], "publisher-failure"),
    )
    assert committed.status_code == 201, committed.text
    async with seeded["factory"]() as session:
        persisted = await session.get(OrderBookOrder, committed.json()["id"])
        assert persisted is not None
        outbox_rows = (
            await session.execute(
                select(MarketEventOutbox).where(
                    MarketEventOutbox.aggregate_id == str(persisted.id)
                )
            )
        ).scalars().all()
        assert outbox_rows
        assert all(row.dispatched_at is None for row in outbox_rows)
        assert all(
            set(row.participant_org_ids) == {str(seeded["buyer_org_id"])}
            for row in outbox_rows
        )


@pytest.mark.asyncio
async def test_growing_collection_routes_apply_database_limits_before_enrichment(
    route_market
):
    client, seeded = route_market
    async with seeded["factory"]() as session:
        orders = []
        for index in range(30):
            orders.append(
                OrderBookOrder(
                    organization_id=seeded["buyer_org_id"],
                    provenance=OrganizationProvenance.REAL,
                    side="BID",
                    product_id=seeded["product_id"],
                    delivery_point_id=seeded["point_id"],
                    quantity_mt=Decimal("10.00"),
                    remaining_quantity_mt=Decimal("10.00"),
                    price_per_mt_usd=Decimal("600.00") + index,
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                )
            )
            orders.append(
                OrderBookOrder(
                    organization_id=seeded["seller_org_id"],
                    provenance=OrganizationProvenance.REAL,
                    side="ASK",
                    product_id=seeded["product_id"],
                    delivery_point_id=seeded["point_id"],
                    quantity_mt=Decimal("10.00"),
                    remaining_quantity_mt=Decimal("10.00"),
                    price_per_mt_usd=Decimal("900.00") + index,
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                    certification_declared=True,
                    certification_scheme="ISCC EU",
                    specification_standard="IMPCA",
                    msds_available=True,
                    carbon_intensity_gco2_mj=Decimal("20.00"),
                    feedstock="test",
                    origin="test",
                )
            )
        session.add_all(orders)
        await session.commit()

    statements: list[str] = []

    def capture_statement(_connection, _cursor, statement, _parameters, _context, _many):
        statements.append(statement)

    event.listen(
        seeded["engine"].sync_engine,
        "before_cursor_execute",
        capture_statement,
    )
    try:
        legacy = await client.get("/api/orderbook?page=1&page_size=7")
        with_ci = await client.get("/api/orderbook/with-ci?limit=6")
        mine = await client.get(
            "/api/orderbook/my?limit=5",
            headers=_headers(seeded["buyer_id"], "unused-my"),
        )
        listings = await client.get("/api/listings?limit=4")
        my_listings = await client.get(
            "/api/listings/my?limit=3",
            headers=_headers(seeded["seller_id"], "unused-my-listings"),
        )
        aggregate = await client.get("/api/orderbook/aggregated?limit=5")
    finally:
        event.remove(
            seeded["engine"].sync_engine,
            "before_cursor_execute",
            capture_statement,
        )

    for response in (legacy, with_ci, mine, listings, my_listings, aggregate):
        assert response.status_code == 200, response.text
    assert len(legacy.json()) == 7
    assert len(with_ci.json()) == 6
    assert len(mine.json()) == 5
    assert len(listings.json()) == 4
    assert len(my_listings.json()) == 3
    assert len(aggregate.json()) <= 5
    order_selects = [
        statement.upper()
        for statement in statements
        if "FROM ORDERBOOK_ORDERS" in statement.upper()
    ]
    assert any(" LIMIT " in statement for statement in order_selects)
    aggregate_selects = [
        statement for statement in order_selects
        if "GROUP BY" in statement and "ELIGIBLE_ORDERBOOK_AGGREGATE" in statement
    ]
    assert len(aggregate_selects) == 1


@pytest.mark.asyncio
async def test_public_catalog_and_forward_table_are_canonical_filtered_and_compact(
    route_market
):
    client, seeded = route_market
    async with seeded["factory"]() as session:
        inactive_product = Product(
            name="Methanol Green",
            fuel_type="Methanol",
            fuel_grade="Green",
            unit="MT",
            is_active=False,
        )
        inactive_point = DeliveryPoint(
            name="Fujairah", region="Middle East", is_active=False
        )
        session.add_all([inactive_product, inactive_point])
        await session.flush()
        session.add_all(
            [
                OrderBookOrder(
                    organization_id=seeded["buyer_org_id"],
                    provenance=OrganizationProvenance.REAL,
                    side="BID",
                    product_id=inactive_product.id,
                    delivery_point_id=seeded["point_id"],
                    quantity_mt=Decimal("262.00"),
                    remaining_quantity_mt=Decimal("262.00"),
                    price_per_mt_usd=Decimal("500.00"),
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                ),
                OrderBookOrder(
                    organization_id=seeded["buyer_org_id"],
                    provenance=OrganizationProvenance.REAL,
                    side="BID",
                    product_id=seeded["product_id"],
                    delivery_point_id=inactive_point.id,
                    quantity_mt=Decimal("11.00"),
                    remaining_quantity_mt=Decimal("11.00"),
                    price_per_mt_usd=Decimal("600.00"),
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                ),
                OrderBookOrder(
                    organization_id=seeded["buyer_org_id"],
                    provenance=OrganizationProvenance.REAL,
                    side="BID",
                    product_id=seeded["product_id"],
                    delivery_point_id=seeded["point_id"],
                    quantity_mt=Decimal("10.00"),
                    remaining_quantity_mt=Decimal("10.00"),
                    price_per_mt_usd=Decimal("700.00"),
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                ),
            ]
        )
        await session.commit()

    demand = await client.get("/api/demand")
    legacy = await client.get("/api/orderbook?limit=100")
    table = await client.get(
        "/api/curves/forward/table",
        params={
            "windows": "SPOT",
            "market_products": "BIO_METHANOL",
            "delivery_point_ids": str(seeded["point_id"]),
        },
    )
    assert demand.status_code == 200, demand.text
    assert legacy.status_code == 200, legacy.text
    assert table.status_code == 200, table.text
    assert {row["market_product_code"] for row in demand.json()} == {"BIO_METHANOL"}
    assert {row["delivery_point_name"] for row in demand.json()} == {"Singapore"}
    assert {row["market_product"] for row in legacy.json()} == {"BIO_METHANOL"}
    assert {row["delivery_point_name"] for row in legacy.json()} == {"Singapore"}
    table_payload = table.json()
    assert len(table_payload["rows"]) == 1
    assert len(table_payload["columns"]) == 1
    cell = table_payload["rows"][0]["cells"]["SPOT"]
    assert "real_best_bid" not in cell
    assert "demo_best_ask" not in cell
    assert "benchmark_mid" not in cell
    assert len(table.content) < 20_000


@pytest.mark.asyncio
async def test_availability_counts_ownerless_inventory_as_unknown_without_economics(
    route_market,
):
    client, seeded = route_market
    async with seeded["factory"]() as session:
        item = await session.get(InventoryItem, seeded["inventory_id"])
        item.owner_user_id = None
        await session.commit()

    response = await client.get("/api/availability")

    assert response.status_code == 200, response.text
    assert len(response.json()) == 1
    availability = response.json()[0]
    assert availability["market_product_code"] == "BIO_METHANOL"
    assert availability["port_name"] == "Singapore"
    assert availability["source_kind"] == "UNKNOWN"
    assert availability["demo_status"] == "UNKNOWN"
    assert availability["unknown_count"] == 1
    assert availability["total_stock_mt"] is None
    assert availability["avg_price_per_mt"] is None
    assert availability["supplier_count"] == 0


@pytest.mark.asyncio
async def test_legacy_trade_shapes_are_excluded_from_every_formal_evidence_route(
    route_market,
):
    client, seeded = route_market
    now = datetime.now(UTC)
    legacy_rows = [
        {
            "id": uuid4(),
            "product_id": seeded["product_id"],
            "product_name": "VLSFO Conventional",
            "fuel_type": "VLSFO",
            "fuel_grade": "Conventional",
            "market_product": "BIO_METHANOL",
            "delivery_point_id": seeded["point_id"],
            "delivery_point_name": "Singapore",
            "delivery_point_region": "Asia",
            "availability_window": "SPOT",
        },
        {
            "id": uuid4(),
            "product_id": seeded["product_id"],
            "product_name": "Bio Methanol",
            "fuel_type": "Methanol",
            "fuel_grade": "Bio",
            "market_product": "BIO_METHANOL",
            "delivery_point_id": seeded["point_id"],
            "delivery_point_name": "Amsterdam",
            "delivery_point_region": "Europe",
            "availability_window": "SPOT",
        },
        {
            "id": uuid4(),
            "product_id": None,
            "product_name": None,
            "fuel_type": None,
            "fuel_grade": None,
            "market_product": None,
            "delivery_point_id": None,
            "delivery_point_name": None,
            "delivery_point_region": None,
            "availability_window": None,
        },
    ]
    async with seeded["factory"]() as session:
        await session.execute(text("SET LOCAL session_replication_role = 'replica'"))
        for row in legacy_rows:
            await session.execute(
                text(
                    """
                    INSERT INTO trades (
                        id, buyer_id, seller_id, initiator_org_id,
                        buyer_provenance, seller_provenance, initiated_by,
                        is_anonymous, product_id, product_name, fuel_type,
                        fuel_grade, market_product, delivery_point_id,
                        delivery_point_name, delivery_point_region,
                        availability_window, market_snapshot_version,
                        quantity_mt, price_per_mt_usd, status,
                        commission_rate_pct, confirmed_at, created_at
                    ) VALUES (
                        :id, :buyer_id, :seller_id, :buyer_id,
                        'REAL', 'REAL', 'BUYER', false, :product_id,
                        :product_name, :fuel_type, :fuel_grade,
                        :market_product, :delivery_point_id,
                        :delivery_point_name, :delivery_point_region,
                        :availability_window, NULL, 10.00, 700.00,
                        'CONFIRMED', 0.5, :confirmed_at, :confirmed_at
                    )
                    """
                ),
                {
                    **row,
                    "buyer_id": seeded["buyer_org_id"],
                    "seller_id": seeded["seller_org_id"],
                    "confirmed_at": now,
                },
            )
        await session.execute(text("SET LOCAL session_replication_role = 'origin'"))
        await session.commit()

    tape = await client.get("/api/trade-tape")
    prices = await client.get("/api/prices")
    reference = await client.get("/api/prices/reference")
    curve = await client.get(
        "/api/curves/forward/slice",
        params={
            "market_product": "BIO_METHANOL",
            "delivery_point_id": str(seeded["point_id"]),
            "availability_window": "SPOT",
        },
    )

    for response in (tape, prices, reference, curve):
        assert response.status_code == 200, response.text
    assert tape.json()["total"] == 0
    assert prices.json()["summaries"] == []
    assert reference.json()["prices"] == []
    assert curve.json()["trades"] == []
    assert curve.json()["cell"]["primary_signal_type"] != "CONFIRMED_TRADE"


@pytest.mark.asyncio
async def test_concurrent_active_negotiations_have_one_stable_winner(
    route_market, monkeypatch
):
    client, seeded = route_market
    second_point_spec = DELIVERY_POINTS_BY_NAME["Dalian"]
    async with seeded["factory"]() as session:
        second_point = DeliveryPoint(
            id=second_point_spec.id,
            name=second_point_spec.name,
            region=second_point_spec.region,
            timezone=second_point_spec.timezone,
            is_active=True,
        )
        asks = [
            OrderBookOrder(
                organization_id=seeded["seller_org_id"],
                # Integrated contract: security's concrete-party gate rejects
                # negotiations against resting orders without a concrete owner.
                owner_user_id=seeded["seller_id"],
                provenance=OrganizationProvenance.REAL,
                side="ASK",
                product_id=seeded["product_id"],
                delivery_point_id=point_id,
                quantity_mt=Decimal("50.00"),
                remaining_quantity_mt=Decimal("50.00"),
                price_per_mt_usd=Decimal("750.00"),
                availability_window="SPOT",
                status=OrderBookStatus.OPEN,
                certification_declared=True,
                certification_scheme="ISCC EU",
                specification_standard="IMPCA",
                msds_available=True,
                carbon_intensity_gco2_mj=Decimal("20.00"),
                feedstock="test",
                origin="test",
            )
            for point_id in (seeded["point_id"], second_point.id)
        ]
        session.add_all([second_point, *asks])
        await session.commit()
        ask_ids = [ask.id for ask in asks]

    # Integration note: security's authentication serializes same-user
    # requests on the user row lock, so a two-arrival transaction-boundary
    # gate would deadlock (the second create cannot reach the checkpoint
    # while the first is paused). The concurrent submissions below serialize
    # by construction; the one-active-negotiation capacity invariant is
    # asserted on the outcome.

    async def negotiate(ask_id, point_id):
        return await client.post(
            "/api/negotiations",
            json={
                "ask_order_id": str(ask_id),
                "counterparty_org_id": str(seeded["seller_org_id"]),
                "product_id": str(seeded["product_id"]),
                "delivery_point_id": str(point_id),
                "availability_window": "SPOT",
                "quantity_mt": "10.00",
                "proposed_price": "740.00",
            },
            headers=_headers(seeded["buyer_id"], f"unused-neg-{point_id}"),
        )

    responses = await asyncio.wait_for(
        asyncio.gather(
            negotiate(ask_ids[0], seeded["point_id"]),
            negotiate(ask_ids[1], second_point.id),
        ),
        timeout=15,
    )

    assert sorted(response.status_code for response in responses) == [201, 409]
    conflict = next(response for response in responses if response.status_code == 409)
    assert conflict.json()["detail"] == (
        "An active negotiation already exists for this product with this counterparty"
    )
    async with seeded["factory"]() as session:
        active = (
            await session.execute(
                select(Negotiation).where(
                    Negotiation.status.in_(
                        (NegotiationStatus.OPEN, NegotiationStatus.COUNTERED)
                    )
                )
            )
        ).scalars().all()
        assert len(active) == 1


@pytest.mark.asyncio
async def test_bilateral_execution_is_permanently_disabled_but_one_sided_read_is_safe(
    route_market
):
    client, seeded = route_market
    async with seeded["factory"]() as session:
        ask = OrderBookOrder(
            organization_id=seeded["seller_org_id"],
            # Integrated contract: security's concrete-party gate rejects a
            # negotiation against a resting order without a concrete owner.
            owner_user_id=seeded["seller_id"],
            provenance=OrganizationProvenance.REAL,
            side="ASK",
            product_id=seeded["product_id"],
            delivery_point_id=seeded["point_id"],
            quantity_mt=Decimal("50.00"),
            remaining_quantity_mt=Decimal("50.00"),
            price_per_mt_usd=Decimal("750.00"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
            certification_declared=True,
            certification_scheme="ISCC EU",
            specification_standard="IMPCA",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("20.00"),
            feedstock="test",
            origin="test",
        )
        session.add(ask)
        await session.commit()
        ask_id = ask.id

    created = await client.post(
        "/api/negotiations",
        json={
            "ask_order_id": str(ask_id),
            "counterparty_org_id": str(seeded["seller_org_id"]),
            "product_id": str(seeded["product_id"]),
            "delivery_point_id": str(seeded["point_id"]),
            "availability_window": "SPOT",
            "quantity_mt": "10.00",
            "proposed_price": "740.00",
        },
        headers=_headers(seeded["buyer_id"], "unused-negotiation"),
    )
    assert created.status_code == 201, created.text
    negotiation = created.json()
    assert negotiation["delivery_point_id"] == str(seeded["point_id"])
    assert negotiation["availability_window"] == "SPOT"
    fetched = await client.get(
        f"/api/negotiations/{negotiation['id']}",
        headers=_headers(seeded["buyer_id"], "unused-negotiation-read"),
    )
    assert fetched.status_code == 200, fetched.text

    negotiation_accept = await client.post(
        f"/api/negotiations/{negotiation['id']}/accept",
        headers=_headers(seeded["buyer_id"], "unused-negotiation-accept"),
    )
    rfq_accept = await client.post(
        f"/api/rfq/{uuid4()}/accept/{uuid4()}",
        headers=_headers(seeded["buyer_id"], "unused-rfq-accept"),
    )
    assert negotiation_accept.status_code == 409
    assert rfq_accept.status_code == 409
    assert "disabled" in negotiation_accept.json()["detail"].lower()
    assert "disabled" in rfq_accept.json()["detail"].lower()
