"""Suggestions serialize public order contracts before a trade exists."""

from decimal import Decimal

import httpx
import pytest
from fastapi import FastAPI
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import OrderSide
from app.models.user import OrgType, UserRole
from app.routers.auth_simple import get_current_user
from app.routers.matchmaking import router
from app.schemas.fame_order import FameAskTerms, FameBidTerms
from tests.unit.test_matchmaking_router import (
    _make_delivery_point,
    _make_order,
    _make_org,
    _make_product,
    _make_user,
)
from tests.unit.test_matchmaking_router import (
    async_engine as async_engine,  # noqa: PLC0414
)
from tests.unit.test_matchmaking_router import db as db  # noqa: PLC0414
from tests.unit.test_matchmaking_router import (
    setup_tables as setup_tables,  # noqa: PLC0414
)


def _bid_terms():
    return FameBidTerms(
        side="BID",
        neat_fame=True,
        standard="EN_14214",
        standard_edition="2012+A2:2019",
        sustainability_scheme="ISCC_EU",
        max_cfpp_c=-5,
        require_quality_evidence=True,
        require_sustainability_evidence=True,
        evidence_due="BEFORE_LOADING",
    ).model_dump(mode="json")


def _ask_terms():
    return FameAskTerms(
        side="ASK",
        neat_fame=True,
        nomination_status="IDENTIFIED",
        batch_reference="PRIVATE-BATCH",
        producing_site="PRIVATE-PRODUCING-SITE",
        production_origin="Malaysia",
        feedstock_origin="Indonesia",
        shipping_location="Singapore",
        uco_mass_pct=100,
        standard="EN_14214",
        standard_edition="2012+A2:2019",
        cfpp_c=-10,
        sustainability_scheme="ISCC_EU",
        certificate_reference="PRIVATE-CERTIFICATE-REFERENCE",
        certificate_holder="PRIVATE-CERTIFICATE-HOLDER",
        certificate_scope="PRIVATE-CERTIFICATE-SCOPE",
        certificate_valid_until="2099-01-01",
        evidence_status="AVAILABLE",
        document_references=["PRIVATE-DOCUMENT-LINK"],
        evidence_due="BEFORE_LOADING",
        quality_evidence={
            "status": "AVAILABLE",
            "reference": "PRIVATE-COA-REFERENCE",
            "batch_reference": "PRIVATE-BATCH",
            "laboratory": "PRIVATE-LABORATORY",
            "sampled_on": "2026-01-01",
            "tested_on": "2026-01-02",
            "results": [{"property": "KINEMATIC_VISCOSITY_40C_MM2_S", "value": "4.5"}],
        },
        sustainability_evidence={
            "status": "AVAILABLE",
            "document_type": "POS",
            "reference": "PRIVATE-POS-REFERENCE",
            "issuer": "PRIVATE-POS-ISSUER",
            "quantity_mt": "1000.00",
            "supply_date": "2026-01-02",
            "due": "BEFORE_LOADING",
        },
    ).model_dump(mode="json")


@pytest.mark.asyncio
@pytest.mark.parametrize("viewer_role", [UserRole.BUYER, UserRole.SUPPLIER])
async def test_serialized_suggestions_redact_order_identity_and_private_fuel_evidence(
    db, viewer_role
):
    buyer_org = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, "Supplier", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(
        db, name="UCOME B100", fuel_type="FAME", fuel_grade="UCOME"
    )
    singapore = await _make_delivery_point(db, "Singapore")
    bid = await _make_order(
        db,
        organization_id=buyer_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.BID,
        price="1100",
    )
    ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.ASK,
        price="1080",
    )
    bid.fame_terms = _bid_terms()
    ask.fame_terms = _ask_terms()
    bid.owner_user_id = buyer.id
    ask.owner_user_id = supplier.id
    await db.commit()

    viewer = buyer if viewer_role == UserRole.BUYER else supplier
    other_organization = supplier_org if viewer_role == UserRole.BUYER else buyer_org
    app = FastAPI()
    app.include_router(router, prefix="/api")
    app.dependency_overrides[get_current_user] = lambda: viewer

    async def request_db():
        # A real request has no fixture organizations in its identity map.
        # This also proves that the projection does not trigger async lazy loads.
        async with AsyncSession(bind=db.bind, expire_on_commit=False) as session:
            yield session

    app.dependency_overrides[get_db] = request_db
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app), base_url="http://test"
    ) as client:
        response = await client.get("/api/matchmaking/suggestions")

    assert response.status_code == 200
    assert "PRIVATE-" not in response.text
    assert str(other_organization.id) not in response.text
    suggestion = response.json()[0]
    assert suggestion["bid_order_id"] == str(bid.id)
    assert suggestion["ask_order_id"] == str(ask.id)
    assert suggestion["recipient_org_id"] == str(viewer.organization_id)
    assert suggestion["score"] > 0
    assert "market_product_match" in suggestion["match_reasons"]
    assert "availability_match" in suggestion["match_reasons"]
    assert suggestion["status"] == "SUGGESTED"

    for key in ("bid_order", "ask_order"):
        public_order = suggestion[key]
        assert public_order["product_name"] == "UCOME B100"
        assert public_order["market_product"] == "UCOME_B100"
        assert public_order["delivery_point_name"] == "Singapore"
        assert public_order["availability_window"] == "SPOT"
        assert public_order["source_kind"] == "LIVE_ORDER"
        assert {
            "organization_id",
            "owner_user_id",
            "created_by_actor_user_id",
            "support_authorization_id",
            "idempotency_key",
            "product",
            "organization",
        }.isdisjoint(public_order)
    public_ask = suggestion["ask_order"]
    assert Decimal(public_ask["price_per_mt_usd"]) == Decimal(1080)
    assert Decimal(public_ask["remaining_quantity_mt"]) == Decimal(1000)
    public_terms = public_ask["fame_terms"]
    assert Decimal(public_terms["cfpp_c"]) == Decimal(-10)
    assert {
        "batch_reference",
        "producing_site",
        "certificate_reference",
        "certificate_holder",
        "certificate_scope",
        "document_references",
    }.isdisjoint(public_terms)
    assert public_terms["quality_evidence"] == {
        "status": "AVAILABLE",
        "results": [
            {
                "property": "KINEMATIC_VISCOSITY_40C_MM2_S",
                "value": "4.5",
                "method": None,
            }
        ],
    }
    assert public_terms["sustainability_evidence"] == {
        "status": "AVAILABLE",
        "document_type": "POS",
        "due": "BEFORE_LOADING",
    }
    assert Decimal(suggestion["bid_order"]["fame_terms"]["max_cfpp_c"]) == Decimal(-5)
