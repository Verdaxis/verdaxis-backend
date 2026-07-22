from datetime import UTC, datetime, timedelta
from decimal import Decimal
from pathlib import Path
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError

from app.models.market_support import MarketSupportAuthorizationStatus
from app.schemas.market_support import AuthorizationCreate
from app.schemas.orderbook import OrderCreate, OrderResponse
from app.services.market_support import (
    authorization_terms_digest,
    order_etag,
    require_matching_etag,
)


def _order(**changes) -> OrderCreate:
    values = {
        "side": "ASK",
        "product_id": uuid4(),
        "delivery_point_id": uuid4(),
        "quantity_mt": Decimal("100.00"),
        "price_per_mt_usd": Decimal("650.00"),
        "availability_window": "SPOT",
        "expires_at": datetime.now(UTC) + timedelta(days=2),
        "certification_declared": True,
        "certification_scheme": "ISCC EU",
        "specification_standard": "IMPCA",
        "msds_available": True,
        "carbon_intensity_gco2_mj": Decimal("12.40"),
        "feedstock": "Waste biomass",
        "origin": "Singapore",
    }
    values.update(changes)
    return OrderCreate(**values)


def _authorization(**changes) -> AuthorizationCreate:
    order = changes.pop("order", _order())
    values = {
        "accountable_user_id": uuid4(),
        "order": order,
        "authorization_expires_at": datetime.now(UTC) + timedelta(days=1),
        "evidence_reference": "support-case/123/customer-email",
        "evidence_sha256": "a" * 64,
        "commercial_consent_version": "v1",
        "commercial_consent_reference": "consent/v1/standing-order-and-fills",
    }
    values.update(changes)
    return AuthorizationCreate(**values)


def test_phase_one_authorization_is_exact_ask_with_fixed_expiry():
    authorization = _authorization()
    assert authorization.order.side.value == "ASK"
    assert authorization.order.expires_at is not None
    assert authorization.authorization_expires_at <= authorization.order.expires_at

    with pytest.raises(ValidationError):
        _authorization(order=_order(side="BID"))
    with pytest.raises(ValidationError):
        _authorization(order=_order(expires_at=None))


def test_terms_digest_is_canonical_and_changes_with_terms():
    first = _order(certifications=["ISCC EU", " RSB "])
    equivalent = _order(
        product_id=first.product_id,
        delivery_point_id=first.delivery_point_id,
        expires_at=first.expires_at,
        certifications=["ISCC EU", " RSB "],
    )
    changed = equivalent.model_copy(update={"price_per_mt_usd": Decimal("651.00")})
    assert authorization_terms_digest(first) == authorization_terms_digest(equivalent)
    assert authorization_terms_digest(first) != authorization_terms_digest(changed)


def test_support_etag_requires_exact_version():
    order_id = uuid4()
    etag = order_etag(order_id, 3)
    require_matching_etag(etag, order_id=order_id, version=3)

    for value, expected_status in ((None, 428), ("bad", 400), (order_etag(order_id, 2), 412)):
        with pytest.raises(HTTPException) as error:
            require_matching_etag(value, order_id=order_id, version=3)
        assert error.value.status_code == expected_status


def test_authorization_lifecycle_has_no_reusable_state():
    assert {value.value for value in MarketSupportAuthorizationStatus} == {
        "ACTIVE",
        "CONSUMED",
        "REVOKED",
    }


def test_router_exposes_no_edit_or_impersonation_surface():
    source = (Path(__file__).resolve().parents[2] / "app/routers/market_support.py").read_text()
    assert "@router.patch" not in source
    assert "X-Acting-Organization" not in source
    assert '"/organizations/{organization_id}/listings/{order_id}/cancel"' in source


def test_public_order_contract_excludes_support_evidence_and_attribution():
    assert {
        "support_authorization_id",
        "created_by_actor_user_id",
        "creation_method",
        "evidence_reference",
        "evidence_sha256",
    }.isdisjoint(OrderResponse.model_fields)


def test_migration_contains_one_use_attribution_and_no_generic_terms_blob():
    source = (
        Path(__file__).resolve().parents[2]
        / "alembic/versions/ms_20260723_assisted_listings.py"
    ).read_text()
    assert "support_authorization_id" in source
    assert "uq_orderbook_support_authorization" in source
    assert "created_by_actor_user_id" in source
    assert "normalized_terms" not in source
    assert "max_uses" not in source
