from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from pydantic import ValidationError

from app.models.market_support import SupportAuthorizationScope
from app.schemas.market_support import (
    AssistedOrderCreateRequest,
    SupportAuthorizationCreate,
)


def _authorization_payload():
    now = datetime.now(UTC)
    return {
        "authorized_contact_name": "Commercial Director",
        "authorized_contact_email": "commercial@example.com",
        "evidence_reference": "crm://cases/VDX-100",
        "scopes": [
            SupportAuthorizationScope.PUBLISH_POST_ONLY_EXECUTABLE_ORDER,
            SupportAuthorizationScope.EDIT_SUPPORT_MANAGED_ORDER,
            SupportAuthorizationScope.CANCEL_SUPPORT_MANAGED_ORDER,
        ],
        "valid_from": now - timedelta(minutes=1),
        "valid_until": now + timedelta(days=7),
        "max_quantity_mt_per_order": Decimal("1000"),
        "max_total_open_quantity_mt": Decimal("2500"),
        "max_order_ttl_hours": 168,
    }


def _order_payload():
    return {
        "side": "ASK",
        "product_id": uuid4(),
        "delivery_point_id": uuid4(),
        "quantity_mt": Decimal("500"),
        "price_per_mt_usd": Decimal("620"),
        "availability_window": "SPOT",
        "expires_at": datetime.now(UTC) + timedelta(hours=24),
        "certification_declared": True,
        "certification_scheme": "ISCC EU",
        "specification_standard": "IMPCA",
        "msds_available": True,
        "carbon_intensity_gco2_mj": Decimal("18.5"),
        "feedstock": "biogenic residue",
        "origin": "Republic of Korea",
    }


def test_authorization_requires_aggregate_limit_at_least_per_order_limit():
    payload = _authorization_payload()
    payload["max_total_open_quantity_mt"] = Decimal("999")
    with pytest.raises(ValidationError):
        SupportAuthorizationCreate(**payload)


def test_authorization_deduplicates_scopes():
    payload = _authorization_payload()
    payload["scopes"].append(
        SupportAuthorizationScope.PUBLISH_POST_ONLY_EXECUTABLE_ORDER
    )
    authorization = SupportAuthorizationCreate(**payload)
    assert authorization.scopes.count(
        SupportAuthorizationScope.PUBLISH_POST_ONLY_EXECUTABLE_ORDER
    ) == 1


def test_assisted_publish_requires_explicit_executable_order_acknowledgement():
    with pytest.raises(ValidationError):
        AssistedOrderCreateRequest(
            accountable_user_id=uuid4(),
            support_authorization_id=uuid4(),
            reason_code="CUSTOMER_ONBOARDING",
            order=_order_payload(),
            acknowledge_executable_resting_order=False,
        )


def test_assisted_reason_code_is_normalized():
    request = AssistedOrderCreateRequest(
        accountable_user_id=uuid4(),
        support_authorization_id=uuid4(),
        reason_code="customer_onboarding",
        order=_order_payload(),
        acknowledge_executable_resting_order=True,
    )
    assert request.reason_code == "CUSTOMER_ONBOARDING"
