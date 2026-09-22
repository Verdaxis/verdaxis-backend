"""Supplier-offer RFQs exercised through ASGI and the restricted PostgreSQL role."""
from __future__ import annotations

import asyncio
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from uuid import UUID, uuid4

import pytest
from sqlalchemy import select, text

from app.market_catalog import PRODUCTS_BY_CODE
from app.models.audit import AuditLog
from app.models.catalog import Product
from app.models.market_event import MarketEventOutbox
from app.models.notification import Notification
from app.models.rfq import RFQ, RFQQuote
from app.models.supplier_offer import SupplierOffer
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.schemas.fame import FameOfferTerms
from app.schemas.supplier_offer import SupplierOfferTerms
from app.services.audit_actions import RFQ_CREATED
from tests.postgres.market_test_support import assign_fixture_real_provenance
from tests.postgres.test_fame_rfq import (
    _contract_terms,
    _create_payload,
    _headers,
    _quote_payload,
    fame_market,
)


def _listing_terms() -> dict:
    contract = _contract_terms()
    declaration = _quote_payload(contract)["offer_terms"]
    declaration.pop("matches_contract_terms")
    declaration.pop("available_quantity_mt")
    declaration.update(
        neat_fame=True, nomination_status="IDENTIFIED",
        ci_boundary="Cultivation through final distribution", ci_basis="ACTUAL",
    )
    commercial_fields = (
        "delivery_basis", "named_location", "delivery_start", "delivery_end",
        "quantity_tolerance_pct", "payment_terms", "inspection_terms",
        "title_risk_terms", "claims_terms", "evidence_due",
    )
    return SupplierOfferTerms.model_validate({
        **{field: contract[field] for field in commercial_fields},
        "fuel_terms": declaration,
    }).model_dump(mode="json")


async def _seed_offer(seeded: dict, **overrides) -> SupplierOffer:
    async with seeded["owner_factory"]() as session:
        offer = SupplierOffer(**{
            "supplier_org_id": seeded["seller_org_id"],
            "supplier_user_id": seeded["seller_id"],
            "product_id": seeded["product_id"],
            "delivery_point_id": seeded["point_id"],
            "quantity_mt": Decimal("250.00"),
            "min_fill_mt": Decimal("10.00"),
            "price_per_mt_usd": Decimal("1095.00"),
            "availability_window": "SPOT",
            "listing_terms": _listing_terms(),
            "expires_at": datetime.now(UTC) + timedelta(days=1),
            **overrides,
        })
        session.add(offer)
        await session.commit()
        return offer


@pytest.fixture
async def targeted_market(fame_market):
    client, seeded = fame_market
    source = await _seed_offer(seeded)
    return client, seeded, source


def _targeted_payload(seeded: dict, source: SupplierOffer) -> dict:
    return {
        **_create_payload(seeded),
        "source_offer_id": str(source.id),
        "expected_source_offer_revision": source.revision,
    }


async def _create_targeted(client, seeded: dict, source: SupplierOffer) -> dict:
    response = await client.post(
        "/api/rfq", json=_targeted_payload(seeded, source),
        headers=_headers(seeded["buyer_id"]),
    )
    assert response.status_code == 201, response.text
    return response.json()


async def _assert_no_rfq(seeded: dict) -> None:
    async with seeded["owner_factory"]() as session:
        assert (await session.execute(select(RFQ.id))).scalars().all() == []


def _assert_public_snapshot(snapshot: dict) -> None:
    assert "supplier_org_id" not in snapshot
    fuel = snapshot["listing_terms"]["fuel_terms"]
    assert not {
        "batch_reference", "producing_site", "certificate_reference",
        "certificate_holder", "document_references", "certificate_scope",
    }.intersection(fuel)
    if fuel.get("quality_evidence"):
        assert not {"reference", "batch_reference", "laboratory"}.intersection(fuel["quality_evidence"])
    if fuel.get("sustainability_evidence"):
        assert not {"reference", "issuer"}.intersection(fuel["sustainability_evidence"])


@pytest.mark.asyncio
async def test_targeted_rfq_captures_server_source_and_preserves_snapshot(targeted_market):
    client, seeded, source = targeted_market
    terms = _listing_terms()
    terms["fuel_terms"].update(
        evidence_status="AVAILABLE", document_references=["PRIVATE-DOCUMENT-REFERENCE"],
        quality_evidence={
            "status": "AVAILABLE", "reference": "PRIVATE-COA-REFERENCE",
            "batch_reference": terms["fuel_terms"]["batch_reference"],
            "laboratory": "Private laboratory", "tested_on": datetime.now(UTC).date().isoformat(),
            "results": [],
        },
        sustainability_evidence={
            "status": "AVAILABLE", "reference": "PRIVATE-POS-REFERENCE",
            "issuer": "Private issuer", "document_type": "POS", "due": "BEFORE_LOADING",
        },
    )
    source.listing_terms = SupplierOfferTerms.model_validate(terms).model_dump(mode="json")
    async with seeded["owner_factory"]() as session:
        current_offer = await session.get(SupplierOffer, source.id)
        current_offer.listing_terms = source.listing_terms
        await session.commit()
    rfq = await _create_targeted(client, seeded, source)
    assert rfq["source_offer_id"] == str(source.id)
    assert rfq["target_supplier_org_id"] is None
    assert rfq["execution_enabled"] is False
    snapshot = rfq["source_offer_snapshot"]
    assert snapshot["offer_id"] == str(source.id)
    assert snapshot["revision"] == source.revision
    assert Decimal(snapshot["quantity_mt"]) == source.quantity_mt
    assert Decimal(snapshot["price_per_mt_usd"]) == source.price_per_mt_usd
    _assert_public_snapshot(snapshot)

    async with seeded["owner_factory"]() as session:
        stored = await session.get(RFQ, UUID(rfq["id"]))
        assert stored.source_offer_id == source.id
        assert stored.target_supplier_org_id == seeded["seller_org_id"]
        full_snapshot = stored.source_offer_snapshot
        assert full_snapshot["supplier_org_id"] == str(seeded["seller_org_id"])
        assert full_snapshot["listing_terms"] == source.listing_terms
        audit = (await session.execute(select(AuditLog).where(
            AuditLog.resource_id == rfq["id"], AuditLog.action == RFQ_CREATED,
        ))).scalar_one()
        assert audit.changes["source_offer_snapshot"] == full_snapshot
        event = (await session.execute(select(MarketEventOutbox).where(
            MarketEventOutbox.aggregate_id == rfq["id"],
            MarketEventOutbox.event_type == "rfq_created",
        ))).scalar_one()
        assert set(event.participant_org_ids) == {
            str(seeded["buyer_org_id"]), str(seeded["seller_org_id"]),
        }
        notifications = (await session.execute(select(Notification))).scalars().all()
        assert [notification.recipient_id for notification in notifications] == [seeded["seller_id"]]
        assert notifications[0].data == {"rfq_id": rfq["id"], "source_offer_id": str(source.id)}
        current_offer = await session.get(SupplierOffer, source.id)
        current_offer.price_per_mt_usd = Decimal("1250.00")
        current_offer.status = "WITHDRAWN"
        current_offer.revision += 1
        await session.commit()

    detail = await client.get(f"/api/rfq/{rfq['id']}", headers=_headers(seeded["buyer_id"]))
    assert detail.status_code == 200, detail.text
    assert detail.json()["source_offer_snapshot"] == snapshot
    supplier_detail = await client.get(f"/api/rfq/{rfq['id']}", headers=_headers(seeded["seller_id"]))
    assert supplier_detail.status_code == 200, supplier_detail.text
    assert supplier_detail.json()["target_supplier_org_id"] == str(seeded["seller_org_id"])
    assert supplier_detail.json()["source_offer_snapshot"] == full_snapshot


@pytest.mark.asyncio
async def test_only_buyer_and_target_supplier_can_list_read_or_quote(targeted_market):
    client, seeded, source = targeted_market
    rfq = await _create_targeted(client, seeded, source)
    async with seeded["owner_factory"]() as session:
        other_buyer_org = Organization(
            name=f"Unrelated buyer {uuid4()}", type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        other_buyer = User(
            email=f"targeted-{uuid4()}@route.test", password_hash="unused",
            role=UserRole.BUYER, status=UserStatus.APPROVED,
            organization=other_buyer_org, email_verified=True,
        )
        session.add(other_buyer)
        await assign_fixture_real_provenance(session, [other_buyer_org])
        await session.commit()

    for user_id in (seeded["buyer_id"], seeded["seller_id"]):
        listing = await client.get("/api/rfq", headers=_headers(user_id))
        assert listing.status_code == 200, listing.text
        assert listing.json()["total"] == 1
        assert [item["id"] for item in listing.json()["items"]] == [rfq["id"]]
        detail = await client.get(f"/api/rfq/{rfq['id']}", headers=_headers(user_id))
        assert detail.status_code == 200, detail.text

    for user_id in (seeded["other_seller_id"], other_buyer.id):
        listing = await client.get("/api/rfq", headers=_headers(user_id))
        assert listing.status_code == 200, listing.text
        assert listing.json() == {"items": [], "total": 0}
        detail = await client.get(f"/api/rfq/{rfq['id']}", headers=_headers(user_id))
        assert detail.status_code == 404, detail.text

    rejected_quote = await client.post(
        f"/api/rfq/{rfq['id']}/quote", json=_quote_payload(rfq["contract_terms"]),
        headers=_headers(seeded["other_seller_id"]),
    )
    assert rejected_quote.status_code == 404, rejected_quote.text
    accepted_quote = await client.post(
        f"/api/rfq/{rfq['id']}/quote", json=_quote_payload(rfq["contract_terms"]),
        headers=_headers(seeded["seller_id"]),
    )
    assert accepted_quote.status_code == 201, accepted_quote.text
    buyer_detail = await client.get(f"/api/rfq/{rfq['id']}", headers=_headers(seeded["buyer_id"]))
    assert buyer_detail.status_code == 200, buyer_detail.text
    assert buyer_detail.json()["source_offer_snapshot"] == rfq["source_offer_snapshot"]
    _assert_public_snapshot(buyer_detail.json()["source_offer_snapshot"])
    assert buyer_detail.json()["quotes"][0]["seller_org_id"] == str(seeded["seller_org_id"])
    expected_quote_terms = FameOfferTerms.model_validate(
        _quote_payload(rfq["contract_terms"])["offer_terms"],
    ).model_dump(mode="json")
    assert buyer_detail.json()["quotes"][0]["offer_terms"] == expected_quote_terms
    async with seeded["owner_factory"]() as session:
        quotes = (await session.execute(select(RFQQuote))).scalars().all()
        assert [quote.seller_org_id for quote in quotes] == [seeded["seller_org_id"]]


@pytest.mark.asyncio
@pytest.mark.parametrize("state", ["stale", "withdrawn", "expired"])
async def test_unavailable_source_cannot_create_targeted_rfq(targeted_market, state):
    client, seeded, source = targeted_market
    async with seeded["owner_factory"]() as session:
        current = await session.get(SupplierOffer, source.id)
        if state == "stale":
            current.revision += 1
        elif state == "withdrawn":
            current.status = "WITHDRAWN"
        else:
            current.expires_at = datetime.now(UTC) - timedelta(seconds=1)
        await session.commit()
    response = await client.post(
        "/api/rfq", json=_targeted_payload(seeded, source),
        headers=_headers(seeded["buyer_id"]),
    )
    assert response.status_code == 409, response.text
    await _assert_no_rfq(seeded)


@pytest.mark.asyncio
@pytest.mark.parametrize("change", [
    "quantity_below_minimum", "quantity_above_offer", "product", "standard", "edition",
    "sustainability_scheme", "availability_window", "astm_grade", "en_climate_class",
])
async def test_targeted_rfq_rejects_incompatible_source_terms(targeted_market, change):
    client, seeded, source = targeted_market
    payload = _targeted_payload(seeded, source)
    if change.startswith("quantity_"):
        payload["quantity_mt"] = "9.99" if change == "quantity_below_minimum" else "250.01"
        payload["contract_terms"]["min_fill_mt"] = "1.00"
    elif change == "product":
        specification = PRODUCTS_BY_CODE["E_METHANOL"]
        async with seeded["owner_factory"]() as session:
            session.add(Product(
                id=specification.id, name=specification.name,
                fuel_type=specification.fuel_type, fuel_grade=specification.fuel_grade,
                unit=specification.unit, is_active=True,
            ))
            await session.commit()
        payload["product_id"] = str(specification.id)
        payload.pop("contract_terms")
    elif change == "standard":
        payload["contract_terms"].update(standard="ASTM_D6751", astm_grade="1-B S15")
    elif change == "edition":
        payload["contract_terms"]["standard_edition"] = "different edition"
    elif change == "sustainability_scheme":
        payload["contract_terms"]["sustainability_scheme"] = "REDCERT_EU"
    elif change == "availability_window":
        payload["availability_window"] = date.today().strftime("%Y-%m")
    else:
        terms = _listing_terms()
        if change == "astm_grade":
            terms["fuel_terms"].update(standard="ASTM_D6751", astm_grade="1-B S15")
            payload["contract_terms"].update(standard="ASTM_D6751", astm_grade="2-B S15")
        else:
            terms["fuel_terms"]["en_climate_class"] = "Class A"
            payload["contract_terms"]["en_climate_class"] = "Class B"
        async with seeded["owner_factory"]() as session:
            current = await session.get(SupplierOffer, source.id)
            current.listing_terms = terms
            await session.commit()
    response = await client.post(
        "/api/rfq", json=payload, headers=_headers(seeded["buyer_id"]),
    )
    assert response.status_code == 422, response.text
    await _assert_no_rfq(seeded)


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["role", "user_status", "organization_status", "membership"])
async def test_source_supplier_current_admission_is_revalidated(targeted_market, change):
    client, seeded, source = targeted_market
    async with seeded["owner_factory"]() as session:
        seller = await session.get(User, seeded["seller_id"])
        if change == "role":
            seller.role = UserRole.BUYER
        elif change == "user_status":
            seller.status = UserStatus.REJECTED
        elif change == "organization_status":
            organization = await session.get(Organization, seeded["seller_org_id"])
            organization.verification_status = "REJECTED"
        else:
            other_seller = await session.get(User, seeded["other_seller_id"])
            seller.organization_id = other_seller.organization_id
        await session.commit()
    response = await client.post(
        "/api/rfq", json=_targeted_payload(seeded, source),
        headers=_headers(seeded["buyer_id"]),
    )
    assert response.status_code == 409, response.text
    await _assert_no_rfq(seeded)


@pytest.mark.asyncio
async def test_source_supplier_without_real_provenance_is_not_admitted(fame_market):
    client, seeded = fame_market
    async with seeded["owner_factory"]() as session:
        organization = Organization(
            name=f"Unknown supplier {uuid4()}", type=OrgType.FUEL_SUPPLIER,
            verification_status="APPROVED",
        )
        seller = User(
            email=f"unknown-{uuid4()}@route.test", password_hash="unused",
            role=UserRole.SUPPLIER, status=UserStatus.APPROVED,
            organization=organization, email_verified=True,
        )
        session.add(seller)
        await session.commit()
    source = await _seed_offer(
        seeded, supplier_org_id=organization.id, supplier_user_id=seller.id,
    )
    response = await client.post(
        "/api/rfq", json=_targeted_payload(seeded, source),
        headers=_headers(seeded["buyer_id"]),
    )
    assert response.status_code == 409, response.text
    await _assert_no_rfq(seeded)


@pytest.mark.asyncio
async def test_buyer_cannot_request_quote_from_own_organization(targeted_market):
    client, seeded, source = targeted_market
    async with seeded["owner_factory"]() as session:
        buyer = await session.get(User, seeded["buyer_id"])
        buyer.organization_id = seeded["seller_org_id"]
        await session.commit()
    response = await client.post(
        "/api/rfq", json=_targeted_payload(seeded, source),
        headers=_headers(seeded["buyer_id"]),
    )
    assert response.status_code == 409, response.text
    await _assert_no_rfq(seeded)


@pytest.mark.asyncio
async def test_source_revision_is_rechecked_after_waiting_for_offer_lock(targeted_market):
    client, seeded, source = targeted_market
    response_task = None
    try:
        async with seeded["owner_factory"]() as session:
            locked = (await session.execute(select(SupplierOffer).where(
                SupplierOffer.id == source.id,
            ).with_for_update())).scalar_one()
            blocker_pid = (await session.execute(text("SELECT pg_backend_pid()"))).scalar_one()
            locked.revision += 1
            locked.price_per_mt_usd = Decimal("1200.00")
            await session.flush()
            response_task = asyncio.create_task(client.post(
                "/api/rfq", json=_targeted_payload(seeded, source),
                headers=_headers(seeded["buyer_id"]),
            ))
            # Observe an actual PostgreSQL lock wait before releasing the new
            # version. A pre-lock revision check would accept a stale snapshot.
            async with asyncio.timeout(3):
                while True:
                    blocked = (await session.execute(text(
                        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity "
                        "WHERE :pid = ANY(pg_blocking_pids(pid)))"
                    ), {"pid": blocker_pid})).scalar_one()
                    if blocked:
                        break
                    assert not response_task.done(), "RFQ creation did not wait for the source lock"
                    await asyncio.sleep(0.01)
            await session.commit()
        response = await asyncio.wait_for(response_task, timeout=5)
        assert response.status_code == 409, response.text
        await _assert_no_rfq(seeded)
    finally:
        if response_task is not None and not response_task.done():
            response_task.cancel()
            await asyncio.gather(response_task, return_exceptions=True)
