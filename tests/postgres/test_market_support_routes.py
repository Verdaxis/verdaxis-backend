"""ASGI proofs for the narrow assisted-listing workflow on PostgreSQL."""
from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import httpx
import pytest
from sqlalchemy import select, update
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.config import settings
from app.core.security import create_access_token
from app.database import get_db
from app.main import app
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.market_support import MarketSupportCapability, StaffCapabilityAssignment
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderCreationMethod, OrderSide
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from tests.postgres.market_test_support import assign_fixture_real_provenance


async def _seed_market_support(engine):
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        supplier_org = Organization(
            name=f"Assisted Supplier {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
            verification_status="APPROVED",
        )
        buyer_org = Organization(
            name=f"Assisted Buyer {uuid4()}",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        support_buyer_org = Organization(
            name=f"Assisted Buyer Target {uuid4()}",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        supplier = User(
            email=f"supplier-{uuid4()}@support.test",
            password_hash="unused",
            role=UserRole.SUPPLIER,
            status=UserStatus.APPROVED,
            organization=supplier_org,
            email_verified=True,
            kyc_status="APPROVED",
        )
        buyer = User(
            email=f"buyer-{uuid4()}@support.test",
            password_hash="unused",
            role=UserRole.BUYER,
            status=UserStatus.APPROVED,
            organization=buyer_org,
            email_verified=True,
            kyc_status="APPROVED",
        )
        support_buyer = User(
            email=f"support-buyer-{uuid4()}@support.test",
            password_hash="unused",
            role=UserRole.BUYER,
            status=UserStatus.APPROVED,
            organization=support_buyer_org,
            email_verified=True,
            kyc_status="APPROVED",
        )
        admin = User(
            email=f"admin-{uuid4()}@support.test",
            password_hash="unused",
            role=UserRole.ADMIN,
            status=UserStatus.APPROVED,
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
        session.add_all([supplier, buyer, support_buyer, admin, product, point])
        await session.flush()
        await assign_fixture_real_provenance(
            session, (supplier_org, buyer_org, support_buyer_org)
        )
        session.add_all(
            [
                StaffCapabilityAssignment(
                    user_id=admin.id,
                    capability=capability,
                    reason="PostgreSQL route proof",
                    granted_by_user_id=admin.id,
                )
                for capability in MarketSupportCapability
            ]
        )
        await session.commit()
        return {
            "factory": factory,
            "admin_id": admin.id,
            "supplier_id": supplier.id,
            "organization_id": supplier_org.id,
            "buyer_id": buyer.id,
            "buyer_org_id": buyer_org.id,
            "support_buyer_id": support_buyer.id,
            "support_buyer_org_id": support_buyer_org.id,
            "product_id": product.id,
            "delivery_point_id": point.id,
        }


@pytest.fixture
async def market_support_client(market_pg, monkeypatch):
    seeded = await _seed_market_support(market_pg)
    monkeypatch.setattr(settings, "MARKET_SUPPORT_ENABLED", True)

    async def override_db():
        async with seeded["factory"]() as session:
            yield session

    app.dependency_overrides[get_db] = override_db
    transport = httpx.ASGITransport(app=app, raise_app_exceptions=False)
    async with httpx.AsyncClient(
        transport=transport,
        base_url="http://market-support.test",
    ) as client:
        yield client, seeded
    app.dependency_overrides.pop(get_db, None)


def _headers(user_id, key: str | None = None) -> dict[str, str]:
    headers = {"Authorization": f"Bearer {create_access_token(str(user_id))}"}
    if key:
        headers["Idempotency-Key"] = key
    return headers


async def _insert_order(seeded, *, side: OrderSide, organization_id, owner_id, creation_method):
    async with seeded["factory"]() as session:
        order = OrderBookOrder(
            organization_id=organization_id,
            owner_user_id=owner_id,
            created_by_actor_user_id=(seeded["admin_id"] if creation_method == OrderCreationMethod.MARKET_SUPPORT else owner_id),
            creation_method=creation_method,
            provenance=OrganizationProvenance.REAL,
            side=side,
            product_id=seeded["product_id"],
            delivery_point_id=seeded["delivery_point_id"],
            quantity_mt=Decimal("10.00"),
            remaining_quantity_mt=Decimal("10.00"),
            price_per_mt_usd=Decimal("725.00"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
            expires_at=datetime.now(UTC) + timedelta(hours=24),
            certification_declared=side == OrderSide.ASK,
            certification_scheme="ISCC EU" if side == OrderSide.ASK else None,
            specification_standard="IMPCA" if side == OrderSide.ASK else None,
            msds_available=side == OrderSide.ASK,
            carbon_intensity_gco2_mj=Decimal("18.50") if side == OrderSide.ASK else None,
            feedstock="Biogenic waste" if side == OrderSide.ASK else None,
            origin="Singapore" if side == OrderSide.ASK else None,
        )
        session.add(order)
        await session.commit()
        return order.id


async def _set_user_status(seeded, user_id, status: UserStatus):
    async with seeded["factory"]() as session:
        await session.execute(update(User).where(User.id == user_id).values(status=status))
        await session.commit()


async def _set_org_status(seeded, organization_id, status: str):
    async with seeded["factory"]() as session:
        await session.execute(
            update(Organization)
            .where(Organization.id == organization_id)
            .values(verification_status=status)
        )
        await session.commit()


def _authorization_payload(seeded) -> dict:
    now = datetime.now(UTC)
    return {
        "accountable_user_id": str(seeded["supplier_id"]),
        "order": {
            "side": "ASK",
            "product_id": str(seeded["product_id"]),
            "delivery_point_id": str(seeded["delivery_point_id"]),
            "quantity_mt": "250.00",
            "price_per_mt_usd": "725.00",
            "availability_window": "SPOT",
            "expires_at": (now + timedelta(hours=48)).isoformat(),
            "is_anonymous": True,
            "certifications": ["ISCC EU"],
            "certification_declared": True,
            "certification_scheme": "ISCC EU",
            "specification_standard": "IMPCA",
            "msds_available": True,
            "carbon_intensity_gco2_mj": "18.50",
            "carbon_intensity_method": "Supplier declaration",
            "feedstock": "Biogenic waste",
            "origin": "Singapore",
            "off_spec": False,
        },
        "authorization_expires_at": (now + timedelta(hours=24)).isoformat(),
        "evidence_reference": "CRM-TEST-001",
        "evidence_sha256": "a" * 64,
        "commercial_consent_version": "v1",
        "commercial_consent_reference": "CONSENT-TEST-001",
        "support_case_reference": "CASE-TEST-001",
    }


@pytest.mark.asyncio
async def test_legacy_workspace_mutations_are_retired_in_context_mode(
    market_support_client,
):
    client, seeded = market_support_client
    org_id = seeded["organization_id"]
    admin_id = seeded["admin_id"]

    created = await client.post(
        f"/api/admin/market-support/organizations/{org_id}/authorizations",
        headers=_headers(admin_id, "authorization-create-1"),
        json=_authorization_payload(seeded),
    )
    assert created.status_code == 410, created.text
    assert created.json()["detail"]["code"] == "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED"


@pytest.mark.asyncio
async def test_assisted_ask_confirm_requires_customer_supplier_member(market_support_client):
    client, seeded = market_support_client
    order_id = await _insert_order(
        seeded,
        side=OrderSide.ASK,
        organization_id=seeded["organization_id"],
        owner_id=seeded["admin_id"],
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )
    created = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={"order_id": str(order_id), "quantity_mt": "2.00"},
    )
    assert created.status_code == 200, created.text
    trade_id = created.json()["id"]

    denied = await client.put(
        f"/api/trades/{trade_id}/confirm", headers=_headers(seeded["support_buyer_id"])
    )
    assert denied.status_code == 403, denied.text
    denied_admin = await client.put(
        f"/api/trades/{trade_id}/confirm", headers=_headers(seeded["admin_id"])
    )
    assert denied_admin.status_code == 403, denied_admin.text
    confirmed = await client.put(
        f"/api/trades/{trade_id}/confirm", headers=_headers(seeded["supplier_id"])
    )
    assert confirmed.status_code == 200, confirmed.text
    assert confirmed.json()["status"] == "CONFIRMED"


@pytest.mark.asyncio
async def test_assisted_bid_confirm_and_decline_are_symmetric(market_support_client):
    client, seeded = market_support_client
    order_id = await _insert_order(
        seeded,
        side=OrderSide.BID,
        organization_id=seeded["support_buyer_org_id"],
        owner_id=seeded["admin_id"],
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )
    created = await client.post(
        "/api/trades/",
        headers=_headers(seeded["supplier_id"]),
        json={"order_id": str(order_id), "quantity_mt": "2.00"},
    )
    assert created.status_code == 200, created.text
    trade_id = created.json()["id"]
    confirmed = await client.put(
        f"/api/trades/{trade_id}/confirm", headers=_headers(seeded["support_buyer_id"])
    )
    assert confirmed.status_code == 200, confirmed.text

    decline_order_id = await _insert_order(
        seeded,
        side=OrderSide.BID,
        organization_id=seeded["support_buyer_org_id"],
        owner_id=seeded["admin_id"],
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )
    decline_created = await client.post(
        "/api/trades/",
        headers=_headers(seeded["supplier_id"]),
        json={"order_id": str(decline_order_id), "quantity_mt": "2.00"},
    )
    assert decline_created.status_code == 200, decline_created.text
    declined = await client.put(
        f"/api/trades/{decline_created.json()['id']}/decline",
        headers=_headers(seeded["support_buyer_id"]),
    )
    assert declined.status_code == 200, declined.text
    assert declined.json()["status"] == "DECLINED"


@pytest.mark.asyncio
async def test_assisted_decline_allows_revoked_support_owner_and_rejected_initiator(
    market_support_client,
):
    client, seeded = market_support_client
    order_id = await _insert_order(
        seeded,
        side=OrderSide.ASK,
        organization_id=seeded["organization_id"],
        owner_id=seeded["admin_id"],
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )
    created = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={"order_id": str(order_id), "quantity_mt": "2.00"},
    )
    assert created.status_code == 200, created.text
    await _set_user_status(seeded, seeded["admin_id"], UserStatus.REJECTED)
    await _set_org_status(seeded, seeded["buyer_org_id"], "REJECTED")

    declined = await client.put(
        f"/api/trades/{created.json()['id']}/decline",
        headers=_headers(seeded["supplier_id"]),
    )
    assert declined.status_code == 200, declined.text
    assert declined.json()["status"] == "DECLINED"


@pytest.mark.asyncio
async def test_trade_creation_rejects_retired_catalog_rows(market_support_client):
    client, seeded = market_support_client
    order_id = await _insert_order(
        seeded,
        side=OrderSide.ASK,
        organization_id=seeded["organization_id"],
        owner_id=seeded["supplier_id"],
        creation_method=OrderCreationMethod.SELF_SERVICE,
    )
    async with seeded["factory"]() as session:
        await session.execute(update(Product).where(Product.id == seeded["product_id"]).values(is_active=False))
        await session.commit()

    rejected = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={"order_id": str(order_id), "quantity_mt": "2.00"},
    )
    assert rejected.status_code == 400, rejected.text


@pytest.mark.asyncio
async def test_self_service_keeps_exact_principal_decline_after_owner_rejection(
    market_support_client,
):
    client, seeded = market_support_client
    confirm_order_id = await _insert_order(
        seeded,
        side=OrderSide.ASK,
        organization_id=seeded["organization_id"],
        owner_id=seeded["supplier_id"],
        creation_method=OrderCreationMethod.SELF_SERVICE,
    )
    confirmed_trade = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={"order_id": str(confirm_order_id), "quantity_mt": "2.00"},
    )
    assert confirmed_trade.status_code == 200, confirmed_trade.text
    confirmed = await client.put(
        f"/api/trades/{confirmed_trade.json()['id']}/confirm",
        headers=_headers(seeded["supplier_id"]),
    )
    assert confirmed.status_code == 200, confirmed.text

    decline_order_id = await _insert_order(
        seeded,
        side=OrderSide.ASK,
        organization_id=seeded["organization_id"],
        owner_id=seeded["supplier_id"],
        creation_method=OrderCreationMethod.SELF_SERVICE,
    )
    pending = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={"order_id": str(decline_order_id), "quantity_mt": "2.00"},
    )
    assert pending.status_code == 200, pending.text
    await _set_user_status(seeded, seeded["supplier_id"], UserStatus.REJECTED)
    declined = await client.put(
        f"/api/trades/{pending.json()['id']}/decline",
        headers=_headers(seeded["supplier_id"]),
    )
    assert declined.status_code == 200, declined.text
    assert declined.json()["status"] == "DECLINED"


@pytest.mark.asyncio
async def test_assisted_confirm_waits_for_uncommitted_customer_rejection(
    market_support_client,
):
    client, seeded = market_support_client
    order_id = await _insert_order(
        seeded,
        side=OrderSide.ASK,
        organization_id=seeded["organization_id"],
        owner_id=seeded["admin_id"],
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )
    created = await client.post(
        "/api/trades/",
        headers=_headers(seeded["buyer_id"]),
        json={"order_id": str(order_id), "quantity_mt": "2.00"},
    )
    assert created.status_code == 200, created.text

    holder = seeded["factory"]()
    try:
        locked_user = (
            await holder.execute(
                select(User).where(User.id == seeded["admin_id"]).with_for_update()
            )
        ).scalar_one()
        locked_user.status = UserStatus.REJECTED
        await holder.flush()

        confirm_task = asyncio.create_task(
            client.put(
                f"/api/trades/{created.json()['id']}/confirm",
                headers=_headers(seeded["supplier_id"]),
            )
        )
        await asyncio.sleep(0.15)
        assert not confirm_task.done(), "confirm did not wait for the user rejection lock"
        await holder.commit()
        confirmed = await asyncio.wait_for(confirm_task, timeout=5)
        assert confirmed.status_code == 409, confirmed.text
    finally:
        if holder.in_transaction():
            await holder.rollback()
        await holder.close()
