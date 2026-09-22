"""Supplier-offer routes with real authentication and exact runtime role grants."""
from __future__ import annotations

from copy import deepcopy
from datetime import UTC, datetime, timedelta
from uuid import UUID, uuid4

import pytest
from sqlalchemy import select, text

from app.models.catalog import DeliveryPoint, Product
from app.models.supplier_offer import SupplierOffer
from app.models.user import Organization, User, UserRole, UserStatus
from tests.postgres.test_fame_rfq import _headers, fame_market  # noqa: F401


_ECONOMIC_TABLES = (
    "orderbook_orders", "trades", "inventory_items", "negotiations",
    "rfqs", "rfq_quotes", "benchmarks", "live_slice_benchmarks",
)
_PRIVATE_DECLARATIONS = {
    "batch_reference", "producing_site", "certificate_reference",
    "certificate_holder", "certificate_scope", "document_references",
}


@pytest.fixture
async def supplier_market(fame_market):
    client, seeded = fame_market
    async with seeded["owner_factory"]() as session:
        peer = User(
            email=f"supplier-peer-{uuid4()}@route.test",
            password_hash="unused", role=UserRole.SUPPLIER,
            status=UserStatus.APPROVED,
            organization_id=seeded["seller_org_id"],
            email_verified=True, kyc_status="APPROVED",
        )
        session.add(peer)
        await session.commit()
        yield client, {**seeded, "peer_id": peer.id}


def _payload(seeded: dict) -> dict:
    today = datetime.now(UTC).date()
    delivery_start = today + timedelta(days=7)
    delivery_end = delivery_start + timedelta(days=7)
    return {
        "product_id": str(seeded["product_id"]),
        "delivery_point_id": str(seeded["point_id"]),
        "quantity_mt": "100.00",
        "min_fill_mt": "25.00",
        "price_per_mt_usd": "1095.00",
        "availability_window": "SPOT",
        "expires_at": (datetime.now(UTC) + timedelta(hours=24)).isoformat(),
        "delivery_basis": "EX_TANK",
        "named_location": "Singapore nominated terminal",
        "delivery_start": delivery_start.isoformat(),
        "delivery_end": delivery_end.isoformat(),
        "quantity_tolerance_pct": "5",
        "payment_terms": "Payment within 10 days of delivery",
        "inspection_terms": "Independent inspection before loading",
        "title_risk_terms": "Title and risk pass at loading flange",
        "claims_terms": "Quality claims within 30 days of delivery",
        "evidence_due": "BEFORE_LOADING",
        "notes": "Supplier declaration, subject to bilateral contract",
        "fuel_terms": {
            "schema_version": 1,
            "neat_fame": True,
            "nomination_status": "IDENTIFIED",
            "batch_reference": "PRIVATE-BATCH-001",
            "producing_site": "PRIVATE-PRODUCTION-SITE",
            "production_origin": "Malaysia",
            "feedstock_origin": "Malaysia",
            "shipping_location": "Singapore",
            "uco_mass_pct": 100,
            "standard": "EN_14214",
            "standard_edition": "2012+A2:2019",
            "en_climate_class": "A",
            "cfpp_c": "-2",
            "cloud_point_c": "1",
            "ci_gco2e_mj": "20",
            "ci_methodology": "ISCC EU declared method",
            "ci_boundary": "Well-to-wheel",
            "ci_basis": "ACTUAL",
            "sustainability_scheme": "ISCC_EU",
            "certificate_reference": "PRIVATE-CERTIFICATE-ID",
            "certificate_holder": "PRIVATE-CERTIFICATE-HOLDER",
            "certificate_valid_until": delivery_end.isoformat(),
            "certificate_scope": "PRIVATE-CERTIFICATE-SCOPE",
            "evidence_status": "AVAILABLE",
            "document_references": ["PRIVATE-DOCUMENT-REFERENCE"],
            "lhv_mj_kg": "37.2",
            "quality_evidence": {
                "status": "AVAILABLE",
                "reference": "PRIVATE-QUALITY-REFERENCE",
                "batch_reference": "PRIVATE-BATCH-001",
                "laboratory": "PRIVATE-LABORATORY",
                "sampled_on": today.isoformat(),
                "tested_on": today.isoformat(),
                "results": [{
                    "property": "ESTER_CONTENT_MASS_PCT",
                    "value": "97.5",
                    "method": "EN 14103",
                }],
            },
            "sustainability_evidence": {
                "status": "AVAILABLE",
                "document_type": "POS",
                "reference": "PRIVATE-SUSTAINABILITY-REFERENCE",
                "issuer": "PRIVATE-ISSUER",
                "quantity_mt": "100.00",
                "supply_date": today.isoformat(),
                "due": "BEFORE_LOADING",
            },
        },
    }


async def _create_offer(client, seeded: dict, *, payload: dict | None = None, key: str | None = None):
    response = await client.post(
        "/api/supplier-offers", json=payload or _payload(seeded),
        headers={**_headers(seeded["seller_id"]), "Idempotency-Key": key or str(uuid4())},
    )
    assert response.status_code == 201, response.text
    return response.json()


def _assert_public_summary(offer: dict) -> None:
    assert offer["listing_kind"] == "SUPPLIER_OFFER"
    assert offer["execution_enabled"] is False
    assert offer["can_edit"] is False
    assert offer["can_withdraw"] is False
    assert "supplier_org_id" not in offer
    assert "supplier_user_id" not in offer
    fuel = offer["fuel_terms"]
    assert _PRIVATE_DECLARATIONS.isdisjoint(fuel)
    assert "PRIVATE-" not in str(offer)
    assert fuel["standard"] == "EN_14214"
    assert fuel["uco_mass_pct"] == 100
    assert fuel["quality_evidence"] == {
        "status": "AVAILABLE",
        "results": [{"property": "ESTER_CONTENT_MASS_PCT", "value": "97.5", "method": "EN 14103"}],
    }
    assert fuel["sustainability_evidence"] == {
        "status": "AVAILABLE", "document_type": "POS", "due": "BEFORE_LOADING",
    }


async def _economic_counts(seeded: dict) -> dict[str, int]:
    async with seeded["owner_factory"]() as session:
        return {
            table: (await session.execute(text(f"SELECT count(*) FROM {table}"))).scalar_one()
            for table in _ECONOMIC_TABLES
        }


@pytest.mark.asyncio
async def test_supplier_offer_discovery_redacts_private_evidence(supplier_market):
    client, seeded = supplier_market
    before = await _economic_counts(seeded)
    payload = _payload(seeded)
    offer = await _create_offer(client, seeded, payload=payload)
    assert offer["revision"] == 1
    assert offer["status"] == "OPEN"
    assert offer["can_edit"] is True
    assert offer["can_withdraw"] is True
    assert offer["can_request_quote"] is False
    assert offer["fuel_terms"]["batch_reference"] == "PRIVATE-BATCH-001"
    assert offer["fuel_terms"]["quality_evidence"] == payload["fuel_terms"]["quality_evidence"]

    for path in (f"/api/supplier-offers/{offer['id']}", "/api/supplier-offers?limit=1"):
        response = await client.get(path, headers=_headers(seeded["buyer_id"]))
        assert response.status_code == 200, response.text
        public_offer = response.json()["items"][0] if "items" in response.json() else response.json()
        _assert_public_summary(public_offer)
        assert public_offer["can_request_quote"] is True
    mine = await client.get("/api/supplier-offers/my?limit=1", headers=_headers(seeded["seller_id"]))
    assert mine.status_code == 200, mine.text
    assert mine.json()["total"] == 1
    assert mine.json()["items"][0]["fuel_terms"]["certificate_reference"] == "PRIVATE-CERTIFICATE-ID"
    beyond_page = await client.get(
        "/api/supplier-offers?skip=1&limit=1", headers=_headers(seeded["buyer_id"])
    )
    assert beyond_page.status_code == 200, beyond_page.text
    assert beyond_page.json() == {"items": [], "total": 1, "skip": 1, "limit": 1}
    async with seeded["owner_factory"]() as session:
        stored = await session.get(SupplierOffer, UUID(offer["id"]))
        assert stored.supplier_org_id == seeded["seller_org_id"]
        assert stored.supplier_user_id == seeded["seller_id"]
        assert stored.listing_terms["fuel_terms"]["document_references"] == ["PRIVATE-DOCUMENT-REFERENCE"]
    assert await _economic_counts(seeded) == before


@pytest.mark.asyncio
async def test_supplier_offer_auth_and_creator_boundaries(supplier_market):
    client, seeded = supplier_market
    payload = _payload(seeded)
    offer = await _create_offer(client, seeded, payload=payload)
    for path in ("/api/supplier-offers", "/api/supplier-offers/my", f"/api/supplier-offers/{offer['id']}"):
        anonymous = await client.get(path)
        assert anonymous.status_code == 401, anonymous.text
    buyer_create = await client.post(
        "/api/supplier-offers", json=payload,
        headers={**_headers(seeded["buyer_id"]), "Idempotency-Key": str(uuid4())},
    )
    assert buyer_create.status_code == 403, buyer_create.text
    peer_read = await client.get(
        f"/api/supplier-offers/{offer['id']}", headers=_headers(seeded["peer_id"])
    )
    assert peer_read.status_code == 200, peer_read.text
    assert peer_read.json()["fuel_terms"]["batch_reference"] == "PRIVATE-BATCH-001"
    assert peer_read.json()["can_edit"] is False
    assert peer_read.json()["can_withdraw"] is False

    for user_id in (seeded["buyer_id"], seeded["other_seller_id"], seeded["peer_id"]):
        revised = await client.put(
            f"/api/supplier-offers/{offer['id']}",
            json={**payload, "expected_revision": 1}, headers=_headers(user_id),
        )
        withdrawn = await client.post(
            f"/api/supplier-offers/{offer['id']}/withdraw",
            json={"expected_revision": 1}, headers=_headers(user_id),
        )
        assert revised.status_code in {403, 404}, revised.text
        assert withdrawn.status_code in {403, 404}, withdrawn.text
    async with seeded["owner_factory"]() as session:
        stored = await session.get(SupplierOffer, UUID(offer["id"]))
        assert stored.status == "OPEN"
        assert stored.revision == 1


@pytest.mark.asyncio
async def test_supplier_offer_post_replay_is_scoped_to_creator_and_payload(supplier_market):
    client, seeded = supplier_market
    payload = _payload(seeded)
    missing_key = await client.post(
        "/api/supplier-offers", json=payload, headers=_headers(seeded["seller_id"])
    )
    assert missing_key.status_code in {400, 422}, missing_key.text
    key = str(uuid4())
    offer = await _create_offer(client, seeded, payload=payload, key=key)
    headers = {**_headers(seeded["seller_id"]), "Idempotency-Key": key}
    replay = await client.post("/api/supplier-offers", json=payload, headers=headers)
    assert replay.status_code in {200, 201}, replay.text
    assert replay.json()["id"] == offer["id"]
    changed_payload = deepcopy(payload)
    changed_payload["price_per_mt_usd"] = "1200.00"
    conflict = await client.post("/api/supplier-offers", json=changed_payload, headers=headers)
    assert conflict.status_code == 409, conflict.text
    peer_replay = await client.post(
        "/api/supplier-offers", json=payload,
        headers={**_headers(seeded["peer_id"]), "Idempotency-Key": key},
    )
    assert peer_replay.status_code == 409, peer_replay.text
    async with seeded["owner_factory"]() as session:
        stored = (await session.execute(select(SupplierOffer))).scalars().all()
        assert [row.id for row in stored] == [UUID(offer["id"])]
        assert stored[0].revision == 1


@pytest.mark.asyncio
async def test_supplier_offer_revision_and_withdrawal_reject_stale_forms(supplier_market):
    client, seeded = supplier_market
    before = await _economic_counts(seeded)
    payload = _payload(seeded)
    offer = await _create_offer(client, seeded, payload=payload)
    url = f"/api/supplier-offers/{offer['id']}"
    headers = _headers(seeded["seller_id"])
    update = {**payload, "price_per_mt_usd": "1080.00", "expected_revision": 1}
    revised = await client.put(url, json=update, headers=headers)
    assert revised.status_code == 200, revised.text
    assert revised.json()["revision"] == 2
    stale_update = await client.put(url, json=update, headers=headers)
    assert stale_update.status_code == 409, stale_update.text
    stale_withdraw = await client.post(
        f"{url}/withdraw", json={"expected_revision": 1}, headers=headers
    )
    assert stale_withdraw.status_code == 409, stale_withdraw.text
    withdrawn = await client.post(
        f"{url}/withdraw", json={"expected_revision": 2}, headers=headers
    )
    assert withdrawn.status_code == 200, withdrawn.text
    assert withdrawn.json()["status"] == "WITHDRAWN"
    assert withdrawn.json()["revision"] == 3
    repeated = await client.post(
        f"{url}/withdraw", json={"expected_revision": 2}, headers=headers
    )
    assert repeated.status_code == 200, repeated.text
    assert repeated.json()["revision"] == 3
    stale_reopen = await client.put(url, json={**update, "expected_revision": 2}, headers=headers)
    assert stale_reopen.status_code == 409, stale_reopen.text
    buyer_view = await client.get(url, headers=_headers(seeded["buyer_id"]))
    assert buyer_view.status_code == 404, buyer_view.text
    reopened = await client.put(url, json={**update, "expected_revision": 3}, headers=headers)
    assert reopened.status_code == 200, reopened.text
    assert reopened.json()["status"] == "OPEN"
    assert reopened.json()["revision"] == 4
    assert await _economic_counts(seeded) == before


@pytest.mark.asyncio
@pytest.mark.parametrize("approval_loss", ["user", "organization"])
async def test_supplier_offer_approval_loss_hides_discovery_but_allows_withdrawal(supplier_market, approval_loss):
    client, seeded = supplier_market
    payload = _payload(seeded)
    offer = await _create_offer(client, seeded, payload=payload)
    async with seeded["owner_factory"]() as session:
        if approval_loss == "user":
            seller = await session.get(User, seeded["seller_id"])
            seller.kyc_status = "REJECTED"
        else:
            organization = await session.get(Organization, seeded["seller_org_id"])
            organization.verification_status = "REJECTED"
        await session.commit()
    discovery = await client.get("/api/supplier-offers", headers=_headers(seeded["buyer_id"]))
    assert discovery.status_code == 200, discovery.text
    assert discovery.json()["items"] == []
    assert discovery.json()["total"] == 0
    detail = await client.get(
        f"/api/supplier-offers/{offer['id']}", headers=_headers(seeded["buyer_id"])
    )
    assert detail.status_code == 404, detail.text
    seller_headers = _headers(seeded["seller_id"])
    revised = await client.put(
        f"/api/supplier-offers/{offer['id']}",
        json={**payload, "expected_revision": 1}, headers=seller_headers,
    )
    assert revised.status_code == 403, revised.text
    withdrawn = await client.post(
        f"/api/supplier-offers/{offer['id']}/withdraw",
        json={"expected_revision": 1}, headers=seller_headers,
    )
    assert withdrawn.status_code == 200, withdrawn.text
    assert withdrawn.json()["status"] == "WITHDRAWN"
    assert withdrawn.json()["revision"] == 2


@pytest.mark.asyncio
async def test_expired_supplier_offer_is_hidden_from_other_organizations(supplier_market):
    client, seeded = supplier_market
    offer = await _create_offer(client, seeded)
    async with seeded["owner_factory"]() as session:
        stored = await session.get(SupplierOffer, UUID(offer["id"]))
        stored.expires_at = datetime.now(UTC) - timedelta(seconds=1)
        await session.commit()
    discovery = await client.get("/api/supplier-offers", headers=_headers(seeded["buyer_id"]))
    assert discovery.status_code == 200, discovery.text
    assert discovery.json()["items"] == []
    stranger_detail = await client.get(
        f"/api/supplier-offers/{offer['id']}", headers=_headers(seeded["other_seller_id"])
    )
    assert stranger_detail.status_code == 404, stranger_detail.text
    owner_history = await client.get("/api/supplier-offers/my", headers=_headers(seeded["seller_id"]))
    assert owner_history.status_code == 200, owner_history.text
    assert owner_history.json()["items"][0]["status"] == "EXPIRED"
    assert owner_history.json()["items"][0]["fuel_terms"]["batch_reference"] == "PRIVATE-BATCH-001"


@pytest.mark.asyncio
@pytest.mark.parametrize("catalog_entry", ["product", "delivery_point"])
async def test_disabled_catalog_hides_public_offer_but_preserves_creator_history(
    supplier_market, catalog_entry
):
    client, seeded = supplier_market
    offer = await _create_offer(client, seeded)
    detail_url = f"/api/supplier-offers/{offer['id']}"
    buyer_headers = _headers(seeded["buyer_id"])
    active_offer = await client.get(detail_url, headers=buyer_headers)
    assert active_offer.status_code == 200, active_offer.text
    assert active_offer.json()["can_request_quote"] is True

    async with seeded["owner_factory"]() as session:
        if catalog_entry == "product":
            catalog_row = await session.get(Product, seeded["product_id"])
        else:
            catalog_row = await session.get(DeliveryPoint, seeded["point_id"])
        catalog_row.is_active = False
        await session.commit()

    discovery = await client.get("/api/supplier-offers", headers=buyer_headers)
    assert discovery.status_code == 200, discovery.text
    assert discovery.json()["items"] == []
    assert discovery.json()["total"] == 0
    public_detail = await client.get(detail_url, headers=buyer_headers)
    assert public_detail.status_code == 404, public_detail.text

    creator_headers = _headers(seeded["seller_id"])
    history = await client.get("/api/supplier-offers/my", headers=creator_headers)
    assert history.status_code == 200, history.text
    assert history.json()["total"] == 1
    assert history.json()["items"][0]["id"] == offer["id"]
    assert history.json()["items"][0]["can_request_quote"] is False
    creator_detail = await client.get(detail_url, headers=creator_headers)
    assert creator_detail.status_code == 200, creator_detail.text
    assert creator_detail.json()["fuel_terms"]["batch_reference"] == "PRIVATE-BATCH-001"
    assert creator_detail.json()["can_request_quote"] is False
