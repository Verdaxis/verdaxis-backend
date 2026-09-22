"""B100 inventory declarations remain attached to their executable publication."""

from copy import deepcopy
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.marketplace import InventoryItem
from app.models.user import (
    Organization,
    OrganizationProvenance,
    User,
    UserRole,
    UserStatus,
)
from app.routers.inventory import (
    _listing_payload,
    add_inventory,
    publish_inventory_item,
    update_inventory,
)
from app.schemas.marketplace import InventoryCreate, InventoryItemUpdate


@pytest.fixture
def ask_terms():
    return {
        "side": "ASK",
        "schema_version": 1,
        "neat_fame": True,
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "Supplier certificate reference",
        "certificate_holder": "Supplier declared holder",
        "certificate_valid_until": (
            datetime.now(UTC).date() + timedelta(days=60)
        ).isoformat(),
        "evidence_status": "PENDING",
        "evidence_due": "BEFORE_LOADING",
    }


def inventory_payload(terms):
    return {
        "port_id": "SGSIN",
        "fuel_type": "FAME",
        "product_name": "UCOME B100",
        "current_stock_mt": "100.00",
        "price_per_mt_usd": "950.00",
        "certification_declared": True,
        "msds_available": True,
        "fame_terms": terms,
    }


def test_inventory_requires_canonical_b100_identity_and_ask_declaration(ask_terms):
    item = InventoryCreate(**inventory_payload(ask_terms))
    assert item.fuel_type.value == "FAME"
    assert item.fame_terms.side == "ASK"
    assert item.fame_terms.ci_gco2e_mj is None

    for changed in (
        {"fame_terms": None},
        {"fuel_type": "Biofuel"},
        {"product_name": "Bio Methanol", "fuel_type": "Methanol"},
        {"product_name": "FAME blend"},
        {"fame_terms": ask_terms | {"side": "BID"}},
        {"is_certified": True},
    ):
        with pytest.raises(ValidationError):
            InventoryCreate(**(inventory_payload(ask_terms) | changed))


@pytest.mark.asyncio
async def test_inventory_create_derives_metadata_without_certification_claim(
    ask_terms, monkeypatch
):
    monkeypatch.setattr(
        "app.routers.inventory.lock_and_load_market_organizations", AsyncMock()
    )
    db = AsyncMock()
    db.add = MagicMock()
    user = SimpleNamespace(id=uuid4(), organization_id=uuid4(), role=UserRole.SUPPLIER)
    body = InventoryCreate(
        **(
            inventory_payload(ask_terms)
            | {"specification_standard": "Wrong standard", "feedstock": "Palm"}
        )
    )

    saved = await add_inventory(item=body, db=db, current_user=user)

    assert saved.fame_terms["side"] == "ASK"
    assert saved.fame_terms["certificate_holder"] == "Supplier declared holder"
    assert saved.specification_standard != "Wrong standard"
    assert saved.feedstock != "Palm"
    assert saved.carbon_intensity_gco2_mj is None
    assert saved.certification_declared is True
    assert saved.msds_available is True
    assert saved.is_certified is False
    db.commit.assert_awaited_once()


@pytest.mark.asyncio
async def test_inventory_patch_cannot_remove_required_terms(ask_terms, monkeypatch):
    monkeypatch.setattr(
        "app.routers.inventory.lock_and_load_market_organizations", AsyncMock()
    )
    monkeypatch.setattr("app.routers.inventory.assert_inventory_mutable", AsyncMock())
    user = SimpleNamespace(id=uuid4(), organization_id=uuid4(), role=UserRole.SUPPLIER)
    values = InventoryCreate(**inventory_payload(ask_terms)).model_dump()
    values["fame_terms"] = deepcopy(ask_terms)
    item = InventoryItem(**values, id=uuid4(), supplier_id=user.organization_id)
    db = AsyncMock()
    db.execute.return_value = MagicMock(scalar_one_or_none=MagicMock(return_value=item))

    with pytest.raises(HTTPException) as error:
        await update_inventory(
            item_id=item.id,
            updates=InventoryItemUpdate(fame_terms=None),
            db=db,
            current_user=user,
        )

    assert error.value.status_code == 422
    assert item.fame_terms == ask_terms
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
async def test_inventory_terms_cannot_change_while_reserved(ask_terms, monkeypatch):
    monkeypatch.setattr(
        "app.routers.inventory.lock_and_load_market_organizations", AsyncMock()
    )
    user = SimpleNamespace(id=uuid4(), organization_id=uuid4(), role=UserRole.SUPPLIER)
    values = InventoryCreate(**inventory_payload(ask_terms)).model_dump()
    values["fame_terms"] = deepcopy(ask_terms)
    item = InventoryItem(
        **values,
        id=uuid4(),
        supplier_id=user.organization_id,
        reserved_stock_mt=Decimal(10),
    )
    db = AsyncMock()
    db.execute.return_value = MagicMock(scalar_one_or_none=MagicMock(return_value=item))

    with pytest.raises(HTTPException, match="active reservation"):
        await update_inventory(
            item_id=item.id,
            updates=InventoryItemUpdate(
                fame_terms=ask_terms | {"certificate_reference": "Changed"}
            ),
            db=db,
            current_user=user,
        )

    assert item.fame_terms == ask_terms
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "delivery_name,invalid_state,error_message",
    [
        ("Singapore", None, None),
        ("Rotterdam", None, "Singapore only"),
        ("Singapore", "missing_terms", "Valid B100"),
        ("Singapore", "expired_certificate", "certificate has expired"),
        ("Singapore", "low_stock", "below the product minimum"),
    ],
)
async def test_inventory_publish_uses_canonical_lane_frozen_terms_and_shared_matcher(
    ask_terms, monkeypatch, delivery_name, invalid_state, error_message
):
    """Publication retains the declaration and passes the normal matching path."""
    from app.routers.inventory import _inventory_write_values

    terms = ask_terms | {"lhv_mj_kg": "37.5"}
    organization = Organization(
        id=uuid4(),
        name="Declared supplier",
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    user = User(
        id=uuid4(),
        organization_id=organization.id,
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
    )
    item = InventoryItem(
        **_inventory_write_values(InventoryCreate(**inventory_payload(terms))),
        id=uuid4(),
        supplier_id=organization.id,
        owner_user_id=user.id,
        reserved_stock_mt=0,
    )
    if invalid_state == "missing_terms":
        item.fame_terms = None
    elif invalid_state == "expired_certificate":
        item.fame_terms["certificate_valid_until"] = "2020-01-01"
    elif invalid_state == "low_stock":
        item.current_stock_mt = Decimal("0.50")
    product_spec = PRODUCTS_BY_CODE["UCOME_B100"]
    product = Product(
        id=product_spec.id,
        name=product_spec.name,
        fuel_type=product_spec.fuel_type,
        fuel_grade=product_spec.fuel_grade,
    )
    delivery_point = DeliveryPoint(
        id=DELIVERY_POINT_IDS[delivery_name], name=delivery_name, region="Asia"
    )
    monkeypatch.setattr("app.config.settings.AUTO_MATCHING_ENABLED", True)
    monkeypatch.setattr(
        "app.routers.inventory._resolve_catalog_product",
        AsyncMock(return_value=product),
    )
    monkeypatch.setattr(
        "app.routers.inventory._resolve_delivery_point",
        AsyncMock(return_value=delivery_point),
    )
    monkeypatch.setattr("app.routers.inventory.acquire_market_slice_lock", AsyncMock())
    monkeypatch.setattr(
        "app.routers.inventory.expire_market_slice_orders",
        AsyncMock(return_value=([], {organization.id: organization})),
    )
    reserve = AsyncMock(return_value=item)
    monkeypatch.setattr("app.routers.inventory.reserve_inventory", reserve)
    for name in (
        "_best_slice_price",
        "rebuild_live_slice_benchmarks_for_keys",
        "emit_order_created",
        "enqueue_market_events",
        "record_audit",
    ):
        monkeypatch.setattr(
            f"app.routers.inventory.{name}", AsyncMock(return_value=None)
        )
    monkeypatch.setattr(
        "app.routers.inventory.collect_auto_match_side_effects",
        AsyncMock(return_value=[]),
    )
    monkeypatch.setattr("app.routers.inventory.track_analytics_event", MagicMock())
    matcher = AsyncMock(return_value=[])
    monkeypatch.setattr("app.services.matching_engine.match_order", matcher)
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.side_effect = [
        MagicMock(scalar_one_or_none=MagicMock(return_value=value))
        for value in (user, organization, item, None, item, None, None)
    ]
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))

    if error_message is not None:
        with pytest.raises(HTTPException, match=error_message):
            await publish_inventory_item(item.id, request, db, user)
        reserve.assert_not_awaited()
        matcher.assert_not_awaited()
        db.commit.assert_not_awaited()
        return

    await publish_inventory_item(item.id, request, db, user)

    listing = db.add.call_args.args[0]
    assert listing.fame_terms == item.fame_terms
    assert listing.fame_terms is not item.fame_terms
    assert listing.energy_density_mj_kg == Decimal("37.5")
    assert listing.certifications == []
    assert listing.certification_declared is True
    reserve.assert_awaited_once_with(db, item.id, Decimal("100.00"))
    matcher.assert_awaited_once_with(db, listing, is_anonymous=True)
    public = _listing_payload(listing)
    private = _listing_payload(listing, include_private_terms=True)
    assert "certificate_holder" not in public["fame_terms"]
    assert private["fame_terms"]["certificate_holder"] == terms["certificate_holder"]
    item.fame_terms["certificate_holder"] = "Later inventory edit"
    assert listing.fame_terms["certificate_holder"] == terms["certificate_holder"]
