"""UCOME RFQ contracts exercised through ASGI and the real PostgreSQL app role."""
from __future__ import annotations

from datetime import UTC, datetime, timedelta
from decimal import Decimal
import os
from uuid import UUID, uuid4

import httpx
import pytest
from sqlalchemy import insert, select, text
from sqlalchemy.engine import make_url
from sqlalchemy.exc import DBAPIError, IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import create_access_token
from app.database import get_db
from app.main import app
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.audit import AuditLog
from app.models.catalog import DeliveryPoint, Product
from app.models.negotiation import Negotiation
from app.models.orderbook import OrderBookOrder, Trade
from app.models.rfq import RFQ, RFQQuote
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.schemas.fame import FameContractTerms, FameOfferTerms
from app.services.audit_actions import (
    RFQ_CREATED,
    RFQ_QUOTE_REVISED,
    RFQ_QUOTE_SUBMITTED,
    RFQ_QUOTE_WITHDRAWN,
)
from tests.postgres.market_test_support import assign_fixture_real_provenance


def _headers(user_id: UUID) -> dict[str, str]:
    return {"Authorization": f"Bearer {create_access_token(str(user_id))}"}


def _contract_terms() -> dict:
    delivery_start = datetime.now(UTC).date() + timedelta(days=7)
    return {
        "schema_version": 1,
        "neat_fame": True,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "max_cfpp_c": "0",
        "max_ci_gco2e_mj": "30",
        "delivery_basis": "EX_TANK",
        "named_location": "Singapore terminal, tank nominated before loading",
        "delivery_start": delivery_start.isoformat(),
        "delivery_end": (delivery_start + timedelta(days=7)).isoformat(),
        "quantity_tolerance_pct": "5",
        "min_fill_mt": "100.00",
        "payment_terms": "Payment within 10 days of delivery",
        "inspection_terms": "Independent inspection before loading",
        "title_risk_terms": "Title and risk pass at loading flange",
        "claims_terms": "Quality claims within 30 days of delivery",
        "sustainability_scheme": "ISCC_EU",
        "evidence_due": "BEFORE_LOADING",
    }


def _create_payload(seeded: dict) -> dict:
    return {
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "100.00",
        "target_price_per_mt": "1100.00",
        "availability_window": "SPOT",
        "expires_in_hours": 24,
        "contract_terms": _contract_terms(),
    }


def _quote_payload(contract_terms: dict) -> dict:
    return {
        "price_per_mt_usd": "1095.00",
        "expires_at": (datetime.now(UTC) + timedelta(hours=2)).isoformat(),
        "notes": "Declared terms pending document review",
        "offer_terms": {
            "schema_version": 1,
            "matches_contract_terms": True,
            "batch_reference": "SG-UCOME-2026-001",
            "producing_site": "Supplier declared production site",
            "production_origin": "Malaysia",
            "feedstock_origin": "Malaysia",
            "shipping_location": "Singapore",
            "uco_mass_pct": 100,
            "standard": contract_terms["standard"],
            "standard_edition": contract_terms["standard_edition"],
            "cfpp_c": "-2",
            "ci_gco2e_mj": "20",
            "ci_methodology": "Supplier declared ISCC EU method",
            "sustainability_scheme": "ISCC_EU",
            "certificate_reference": "TEST-DECLARED-CERTIFICATE",
            "certificate_holder": "Test supplier",
            "certificate_valid_until": contract_terms["delivery_end"],
            "evidence_status": "DECLARED",
            "document_references": [],
            "available_quantity_mt": "100.00",
            "lhv_mj_kg": "37.2",
        },
    }


def _revision_payload(contract_terms: dict, quote: dict) -> dict:
    return {**_quote_payload(contract_terms), "expected_revision": quote["revision"]}


@pytest.fixture
async def fame_market(market_pg):
    """Seed as migrator; authenticate and mutate as the restricted app role."""
    app_url = os.environ.get("DATABASE_URL", "")
    app_role = os.environ.get("RUNTIME_TEST_APP_ROLE", "")
    if not app_url or not app_role:
        pytest.skip("the PostgreSQL runtime app role is not configured")
    runtime_url = make_url(app_url).set(database=market_pg.url.database)
    assert runtime_url.username == app_role
    runtime_engine = create_async_engine(runtime_url, hide_parameters=True)
    owner_factory = async_sessionmaker(market_pg, expire_on_commit=False)
    runtime_factory = async_sessionmaker(
        runtime_engine, class_=AsyncSession, expire_on_commit=False
    )
    try:
        async with runtime_engine.connect() as connection:
            role = (await connection.execute(text(
                "SELECT current_user, rolsuper FROM pg_roles WHERE rolname = current_user"
            ))).one()
            assert role == (app_role, False)

        async with owner_factory() as session:
            buyer_org = Organization(
                name=f"FAME buyer {uuid4()}", type=OrgType.FUEL_BUYER,
                verification_status="APPROVED",
            )
            seller_org = Organization(
                name=f"FAME supplier {uuid4()}", type=OrgType.FUEL_SUPPLIER,
                verification_status="APPROVED",
            )
            other_org = Organization(
                name=f"Other FAME supplier {uuid4()}", type=OrgType.FUEL_SUPPLIER,
                verification_status="APPROVED",
            )
            users = [
                User(
                    email=f"fame-{uuid4()}@route.test", password_hash="unused",
                    role=role, status=UserStatus.APPROVED, organization=organization,
                    email_verified=True, kyc_status="APPROVED",
                )
                for organization, role in (
                    (buyer_org, UserRole.BUYER),
                    (seller_org, UserRole.SUPPLIER),
                    (other_org, UserRole.SUPPLIER),
                )
            ]
            product_spec = PRODUCTS_BY_CODE["UCOME_B100"]
            point_spec = DELIVERY_POINTS_BY_NAME["Singapore"]
            product = Product(
                id=product_spec.id, name=product_spec.name,
                fuel_type=product_spec.fuel_type, fuel_grade=product_spec.fuel_grade,
                unit=product_spec.unit, is_active=True,
            )
            point = DeliveryPoint(
                id=point_spec.id, name=point_spec.name, region=point_spec.region,
                timezone=point_spec.timezone, is_active=True,
            )
            session.add_all([*users, product, point])
            await assign_fixture_real_provenance(session, (buyer_org, seller_org, other_org))
            await session.commit()
            seeded = {
                "owner_factory": owner_factory,
                "runtime_engine": runtime_engine,
                "buyer_id": users[0].id,
                "seller_id": users[1].id,
                "other_seller_id": users[2].id,
                "buyer_org_id": buyer_org.id,
                "seller_org_id": seller_org.id,
                "product_id": product.id,
                "point_id": point.id,
            }

        async def override_db():
            async with runtime_factory() as session:
                yield session

        previous_override = app.dependency_overrides.get(get_db)
        app.dependency_overrides[get_db] = override_db
        try:
            transport = httpx.ASGITransport(app=app)
            async with httpx.AsyncClient(
                transport=transport, base_url="http://fame-rfq.test"
            ) as client:
                yield client, seeded
        finally:
            if previous_override is None:
                app.dependency_overrides.pop(get_db, None)
            else:
                app.dependency_overrides[get_db] = previous_override
    finally:
        await runtime_engine.dispose()


async def _create_rfq(client: httpx.AsyncClient, seeded: dict) -> dict:
    response = await client.post(
        "/api/rfq", json=_create_payload(seeded), headers=_headers(seeded["buyer_id"])
    )
    assert response.status_code == 201, response.text
    return response.json()


async def _submit_quote(client: httpx.AsyncClient, seeded: dict, rfq: dict) -> dict:
    response = await client.post(
        f"/api/rfq/{rfq['id']}/quote", json=_quote_payload(rfq["contract_terms"]),
        headers=_headers(seeded["seller_id"]),
    )
    assert response.status_code == 201, response.text
    return response.json()


@pytest.mark.asyncio
async def test_fame_lifecycle_preserves_declared_terms_and_quote_versions(fame_market):
    client, seeded = fame_market
    rfq = await _create_rfq(client, seeded)
    assert rfq["execution_enabled"] is False
    assert rfq["can_cancel"] is True
    expected_contract_terms = FameContractTerms.model_validate(
        _create_payload(seeded)["contract_terms"]
    ).model_dump(mode="json")
    assert rfq["contract_terms"] == expected_contract_terms
    quote = await _submit_quote(client, seeded, rfq)
    assert quote["revision"] == 1
    assert quote["status"] == "PENDING"
    assert quote["execution_enabled"] is False
    assert quote["is_expired"] is False

    updated_payload = _revision_payload(rfq["contract_terms"], quote)
    updated_payload["price_per_mt_usd"] = "1080.00"
    updated_payload["offer_terms"]["batch_reference"] = "SG-UCOME-2026-002"
    expected_offer_terms = FameOfferTerms.model_validate(
        updated_payload["offer_terms"]
    ).model_dump(mode="json")
    revised = await client.put(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}", json=updated_payload,
        headers=_headers(seeded["seller_id"]),
    )
    assert revised.status_code == 200, revised.text
    assert revised.json()["revision"] == 2
    assert revised.json()["offer_terms"] == expected_offer_terms
    stale_revision = await client.put(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}", json=updated_payload,
        headers=_headers(seeded["seller_id"]),
    )
    assert stale_revision.status_code == 409, stale_revision.text
    assert "reload" in stale_revision.json()["detail"]

    withdrawn = await client.post(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}/withdraw",
        headers=_headers(seeded["seller_id"]),
    )
    assert withdrawn.status_code == 200, withdrawn.text
    assert withdrawn.json()["status"] == "WITHDRAWN"
    assert withdrawn.json()["revision"] == 3
    accepted = await client.post(
        f"/api/rfq/{rfq['id']}/accept/{quote['id']}",
        headers=_headers(seeded["buyer_id"]),
    )
    assert accepted.status_code == 409, accepted.text
    assert "non-executable" in accepted.json()["detail"]

    async with seeded["owner_factory"]() as session:
        stored_rfq = await session.get(RFQ, UUID(rfq["id"]))
        stored_quote = await session.get(RFQQuote, UUID(quote["id"]))
        assert stored_rfq.contract_terms == rfq["contract_terms"]
        assert stored_rfq.trade_id is None
        assert stored_rfq.accepted_quote_id is None
        assert stored_quote.offer_terms == expected_offer_terms
        assert stored_quote.revision == 3
        assert stored_quote.price_per_mt_usd == Decimal("1080.00")
        audits = (await session.execute(select(AuditLog).where(
            AuditLog.resource_id.in_([rfq["id"], quote["id"]])
        ))).scalars().all()
        by_action = {audit.action: audit.changes for audit in audits}
        assert by_action[RFQ_CREATED]["contract_terms"] == rfq["contract_terms"]
        assert by_action[RFQ_QUOTE_SUBMITTED]["quote"]["offer_terms"] == quote["offer_terms"]
        assert by_action[RFQ_QUOTE_REVISED]["from"]["revision"] == 1
        assert by_action[RFQ_QUOTE_REVISED]["from"]["offer_terms"] == quote["offer_terms"]
        assert by_action[RFQ_QUOTE_REVISED]["to"]["offer_terms"] == expected_offer_terms
        assert by_action[RFQ_QUOTE_WITHDRAWN]["to"]["status"] == "WITHDRAWN"


@pytest.mark.asyncio
async def test_withdrawal_rejects_stale_revision_but_allows_fresh_resubmission(fame_market):
    client, seeded = fame_market
    rfq = await _create_rfq(client, seeded)
    quote = await _submit_quote(client, seeded, rfq)
    assert quote["revision"] == 1
    quote_url = f"/api/rfq/{rfq['id']}/quotes/{quote['id']}"
    headers = _headers(seeded["seller_id"])

    withdrawn = await client.post(f"{quote_url}/withdraw", headers=headers)
    assert withdrawn.status_code == 200, withdrawn.text
    assert withdrawn.json()["status"] == "WITHDRAWN"
    assert withdrawn.json()["revision"] == 2
    repeated = await client.post(f"{quote_url}/withdraw", headers=headers)
    assert repeated.status_code == 200, repeated.text
    assert repeated.json()["status"] == "WITHDRAWN"
    assert repeated.json()["revision"] == 2

    stale_revision = await client.put(
        quote_url, json=_revision_payload(rfq["contract_terms"], quote), headers=headers
    )
    assert stale_revision.status_code == 409, stale_revision.text
    async with seeded["owner_factory"]() as session:
        stored_quote = await session.get(RFQQuote, UUID(quote["id"]))
        assert stored_quote.status.value == "WITHDRAWN"
        assert stored_quote.revision == 2

    fetched = await client.get(f"/api/rfq/{rfq['id']}", headers=headers)
    assert fetched.status_code == 200, fetched.text
    current_quote = fetched.json()["quotes"][0]
    assert current_quote["id"] == quote["id"]
    assert current_quote["revision"] == 2
    resubmitted = await client.put(
        quote_url, json=_revision_payload(rfq["contract_terms"], current_quote),
        headers=headers,
    )
    assert resubmitted.status_code == 200, resubmitted.text
    assert resubmitted.json()["status"] == "PENDING"
    assert resubmitted.json()["revision"] == 3
    async with seeded["owner_factory"]() as session:
        stored_quote = await session.get(RFQQuote, UUID(quote["id"]))
        assert stored_quote.status.value == "PENDING"
        assert stored_quote.revision == 3


@pytest.mark.asyncio
async def test_ucome_requires_contract_terms_and_rejects_unverified_extra_claims(fame_market):
    client, seeded = fame_market
    missing_terms = _create_payload(seeded)
    missing_terms.pop("contract_terms")
    response = await client.post(
        "/api/rfq", json=missing_terms, headers=_headers(seeded["buyer_id"])
    )
    assert response.status_code == 422, response.text
    assert "contract_terms" in response.json()["detail"]
    false_verification = _create_payload(seeded)
    false_verification["contract_terms"]["platform_verified"] = True
    response = await client.post(
        "/api/rfq", json=false_verification, headers=_headers(seeded["buyer_id"])
    )
    assert response.status_code == 422, response.text
    async with seeded["owner_factory"]() as session:
        assert (await session.execute(select(RFQ))).scalars().all() == []


@pytest.mark.asyncio
@pytest.mark.parametrize("invalid_field", ["offer_terms", "expires_at", "standard", "quantity"])
async def test_quote_rejects_missing_or_incompatible_declarations(fame_market, invalid_field):
    client, seeded = fame_market
    rfq = await _create_rfq(client, seeded)
    payload = _quote_payload(rfq["contract_terms"])
    if invalid_field in {"offer_terms", "expires_at"}:
        payload.pop(invalid_field)
    elif invalid_field == "standard":
        payload["offer_terms"]["standard"] = "ASTM_D6751"
    else:
        payload["offer_terms"]["available_quantity_mt"] = "99.00"
    response = await client.post(
        f"/api/rfq/{rfq['id']}/quote", json=payload,
        headers=_headers(seeded["seller_id"]),
    )
    assert response.status_code == 422, response.text
    async with seeded["owner_factory"]() as session:
        assert (await session.execute(select(RFQQuote))).scalars().all() == []
        assert (await session.get(RFQ, UUID(rfq["id"]))).status.value == "OPEN"


@pytest.mark.asyncio
async def test_other_supplier_cannot_read_or_mutate_declared_quote(fame_market):
    client, seeded = fame_market
    rfq = await _create_rfq(client, seeded)
    quote = await _submit_quote(client, seeded, rfq)
    headers = _headers(seeded["other_seller_id"])
    fetched = await client.get(f"/api/rfq/{rfq['id']}", headers=headers)
    assert fetched.status_code == 200, fetched.text
    assert fetched.json()["quotes"] == []
    revised = await client.put(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}",
        json=_revision_payload(rfq["contract_terms"], quote), headers=headers,
    )
    withdrawn = await client.post(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}/withdraw", headers=headers
    )
    assert revised.status_code == 404, revised.text
    assert withdrawn.status_code == 404, withdrawn.text
    async with seeded["owner_factory"]() as session:
        stored = await session.get(RFQQuote, UUID(quote["id"]))
        assert stored.status.value == "PENDING"
        assert stored.revision == 1


@pytest.mark.asyncio
async def test_quote_and_rfq_expiry_preserve_history_and_allow_withdrawal(fame_market):
    client, seeded = fame_market
    rfq = await _create_rfq(client, seeded)
    quote = await _submit_quote(client, seeded, rfq)
    expired_at = datetime.now(UTC) - timedelta(seconds=1)
    async with seeded["owner_factory"]() as session:
        stored = await session.get(RFQQuote, UUID(quote["id"]))
        stored.expires_at = expired_at
        await session.commit()
    fetched = await client.get(
        f"/api/rfq/{rfq['id']}", headers=_headers(seeded["buyer_id"])
    )
    assert fetched.status_code == 200, fetched.text
    assert fetched.json()["quotes"][0]["is_expired"] is True
    assert fetched.json()["quotes"][0]["offer_terms"] == quote["offer_terms"]

    async with seeded["owner_factory"]() as session:
        stored = await session.get(RFQ, UUID(rfq["id"]))
        stored.expires_at = expired_at
        await session.commit()
    headers = _headers(seeded["seller_id"])
    visible = await client.get("/api/rfq", headers=headers)
    assert visible.status_code == 200, visible.text
    assert [item["id"] for item in visible.json()["items"]] == [rfq["id"]]
    assert visible.json()["items"][0]["quotes"][0]["id"] == quote["id"]
    other_supplier = await client.get("/api/rfq", headers=_headers(seeded["other_seller_id"]))
    assert other_supplier.status_code == 200, other_supplier.text
    assert other_supplier.json()["items"] == []
    revised = await client.put(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}",
        json=_revision_payload(rfq["contract_terms"], quote), headers=headers,
    )
    assert revised.status_code == 400, revised.text
    assert "expired" in revised.json()["detail"]
    withdrawn = await client.post(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}/withdraw", headers=headers
    )
    assert withdrawn.status_code == 200, withdrawn.text
    assert withdrawn.json()["status"] == "WITHDRAWN"
    assert withdrawn.json()["is_expired"] is True


@pytest.mark.asyncio
async def test_supplier_can_withdraw_after_kyc_approval_is_revoked(fame_market):
    client, seeded = fame_market
    rfq = await _create_rfq(client, seeded)
    quote = await _submit_quote(client, seeded, rfq)
    async with seeded["owner_factory"]() as session:
        seller = await session.get(User, seeded["seller_id"])
        seller.kyc_status = "REJECTED"
        await session.commit()
    headers = _headers(seeded["seller_id"])
    revised = await client.put(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}",
        json=_revision_payload(rfq["contract_terms"], quote), headers=headers,
    )
    assert revised.status_code == 403, revised.text
    withdrawn = await client.post(
        f"/api/rfq/{rfq['id']}/quotes/{quote['id']}/withdraw", headers=headers
    )
    assert withdrawn.status_code == 200, withdrawn.text
    assert withdrawn.json()["status"] == "WITHDRAWN"


@pytest.mark.asyncio
@pytest.mark.parametrize("table", ["orderbook_orders", "negotiations"])
async def test_direct_sql_rejects_ucome_executable_market_rows(fame_market, table):
    _, seeded = fame_market
    common = {
        "product_id": seeded["product_id"],
        "delivery_point_id": seeded["point_id"],
        "quantity_mt": Decimal("100.00"),
        "availability_window": "SPOT",
    }
    if table == "orderbook_orders":
        statement = insert(OrderBookOrder.__table__).values(
            **common, organization_id=seeded["buyer_org_id"],
            owner_user_id=seeded["buyer_id"], provenance="REAL", side="BID",
            remaining_quantity_mt=Decimal("100.00"), price_per_mt_usd=Decimal("1100.00"),
            status="OPEN",
        )
    else:
        statement = insert(Negotiation.__table__).values(
            **common, initiator_org_id=seeded["buyer_org_id"],
            counterparty_org_id=seeded["seller_org_id"],
            initiator_user_id=seeded["buyer_id"], counterparty_user_id=seeded["seller_id"],
            last_actor_org_id=seeded["buyer_org_id"], initiator_side="BUYER",
            current_price=Decimal("1100.00"), status="OPEN",
            expires_at=datetime.now(UTC) + timedelta(hours=2),
        )
    with pytest.raises(IntegrityError) as error:
        async with seeded["runtime_engine"].begin() as connection:
            await connection.execute(statement)
    assert f"ck_{table}_execution_product" in str(error.value.orig)


@pytest.mark.asyncio
async def test_direct_sql_rejects_ucome_trade_snapshot(fame_market):
    _, seeded = fame_market
    product = PRODUCTS_BY_CODE["UCOME_B100"]
    point = DELIVERY_POINTS_BY_NAME["Singapore"]
    statement = insert(Trade.__table__).values(
        buyer_id=seeded["buyer_org_id"],
        seller_id=seeded["seller_org_id"],
        buyer_user_id=seeded["buyer_id"],
        seller_user_id=seeded["seller_id"],
        initiator_org_id=seeded["buyer_org_id"],
        buyer_provenance="REAL",
        seller_provenance="REAL",
        initiated_by="BUYER",
        product_id=product.id,
        product_name=product.name,
        fuel_type=product.fuel_type,
        fuel_grade=product.fuel_grade,
        market_product="UCOME_B100",
        delivery_point_id=point.id,
        delivery_point_name=point.name,
        delivery_point_region=point.region,
        availability_window="SPOT",
        market_snapshot_version=1,
        quantity_mt=Decimal("100.00"),
        price_per_mt_usd=Decimal("1100.00"),
        status="PENDING_CONFIRMATION",
    )
    with pytest.raises(DBAPIError) as error:
        async with seeded["runtime_engine"].begin() as connection:
            await connection.execute(statement)
    assert error.value.orig.sqlstate == "P0001"
    assert "trade product snapshot must reference an active canonical product" in str(
        error.value.orig
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("delivery_point_name", [None, "Dalian"])
async def test_direct_sql_rejects_ucome_rfq_outside_singapore(fame_market, delivery_point_name):
    _, seeded = fame_market
    delivery_point_id = None
    if delivery_point_name is not None:
        point = DELIVERY_POINTS_BY_NAME[delivery_point_name]
        async with seeded["owner_factory"]() as session:
            session.add(DeliveryPoint(
                id=point.id, name=point.name, region=point.region,
                timezone=point.timezone, is_active=True,
            ))
            await session.commit()
        delivery_point_id = point.id
    statement = insert(RFQ.__table__).values(
        buyer_org_id=seeded["buyer_org_id"],
        buyer_user_id=seeded["buyer_id"],
        product_id=seeded["product_id"],
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal("100.00"),
        availability_window="SPOT",
        contract_terms=_contract_terms(),
        expires_at=datetime.now(UTC) + timedelta(hours=24),
        status="OPEN",
    )
    with pytest.raises(IntegrityError) as error:
        async with seeded["runtime_engine"].begin() as connection:
            await connection.execute(statement)
    assert "ck_rfqs_fame_delivery_lane" in str(error.value.orig)


@pytest.mark.asyncio
@pytest.mark.parametrize("table", ["rfqs", "rfq_quotes"])
async def test_runtime_rfq_acl_rejects_unknown_columns(fame_market, market_pg, table):
    _, seeded = fame_market
    # A future migration must explicitly admit writable columns. Table-wide
    # INSERT/UPDATE grants would silently make this unreviewed field writable.
    column = "unreviewed_contract_probe"
    async with market_pg.begin() as connection:
        await connection.execute(text(f"ALTER TABLE {table} ADD COLUMN {column} text"))
    try:
        async with seeded["runtime_engine"].connect() as connection:
            privileges = (await connection.execute(text(
                "SELECT has_table_privilege(current_user, :table, 'INSERT'), "
                "has_table_privilege(current_user, :table, 'UPDATE'), "
                "has_column_privilege(current_user, :table, :column, 'INSERT'), "
                "has_column_privilege(current_user, :table, :column, 'UPDATE')"
            ), {"table": table, "column": column})).one()
            assert privileges == (False, False, False, False)
        for statement in (
            f"INSERT INTO {table} ({column}) VALUES ('unreviewed')",
            f"UPDATE {table} SET {column} = 'unreviewed' WHERE false",
        ):
            with pytest.raises(DBAPIError) as error:
                async with seeded["runtime_engine"].begin() as connection:
                    await connection.execute(text(statement))
            assert error.value.orig.sqlstate == "42501"
    finally:
        async with market_pg.begin() as connection:
            await connection.execute(text(f"ALTER TABLE {table} DROP COLUMN {column}"))
