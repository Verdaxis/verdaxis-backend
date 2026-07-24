"""ASGI proofs for the narrow assisted-listing workflow on PostgreSQL."""
from __future__ import annotations

from datetime import UTC, datetime, timedelta
from uuid import uuid4

import httpx
import pytest
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.config import settings
from app.core.security import create_access_token
from app.database import get_db
from app.main import app
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.market_support import MarketSupportCapability, StaffCapabilityAssignment
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from tests.postgres.market_test_support import assign_fixture_real_provenance


async def _seed_market_support(engine):
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        supplier_org = Organization(
            name=f"Assisted Supplier {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
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
        session.add_all([supplier, admin, product, point])
        await session.flush()
        await assign_fixture_real_provenance(session, (supplier_org,))
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
