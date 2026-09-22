"""Targeted RFQs require an explicit source version and private visibility."""
from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from uuid import uuid4

from fastapi import HTTPException
from pydantic import ValidationError
import pytest

from app.models.rfq import RFQStatus
from app.models.user import UserRole
from app.routers import rfq as router
from app.schemas.rfq import RFQCreateRequest


@pytest.mark.parametrize("source_fields", [
    {"source_offer_id": uuid4()},
    {"expected_source_offer_revision": 1},
    {"source_offer_id": uuid4(), "expected_source_offer_revision": 0},
])
def test_source_offer_and_positive_revision_are_required_together(source_fields):
    with pytest.raises(ValidationError):
        RFQCreateRequest(
            product_id=uuid4(), delivery_point_id=uuid4(), quantity_mt="100.00",
            **source_fields,
        )


@pytest.mark.parametrize("derived_field", ["target_supplier_org_id", "source_offer_snapshot"])
def test_client_cannot_supply_derived_target_or_snapshot(derived_field):
    supplied_value = {} if derived_field == "source_offer_snapshot" else str(uuid4())
    with pytest.raises(ValidationError) as error:
        RFQCreateRequest(
            product_id=uuid4(), delivery_point_id=uuid4(), quantity_mt="100.00",
            source_offer_id=uuid4(), expected_source_offer_revision=1,
            **{derived_field: supplied_value},
        )
    assert any(
        detail["loc"] == (derived_field,) and detail["type"] == "extra_forbidden"
        for detail in error.value.errors()
    )


def test_open_targeted_rfq_is_private_to_buyer_and_target_supplier():
    target_org_id = uuid4()
    unrelated_supplier = SimpleNamespace(role=UserRole.SUPPLIER, organization_id=uuid4())
    rfq = SimpleNamespace(
        buyer_org_id=uuid4(), target_supplier_org_id=target_org_id,
        source_offer_id=uuid4(), status=RFQStatus.OPEN,
        expires_at=datetime.now(UTC) + timedelta(hours=1),
        quotes=[],
    )
    with pytest.raises(HTTPException) as error:
        router._ensure_rfq_detail_visible(rfq, unrelated_supplier)
    assert error.value.status_code == 404

    buyer = SimpleNamespace(role=UserRole.BUYER, organization_id=rfq.buyer_org_id)
    target_supplier = SimpleNamespace(role=UserRole.SUPPLIER, organization_id=target_org_id)
    router._ensure_rfq_detail_visible(rfq, buyer)
    router._ensure_rfq_detail_visible(rfq, target_supplier)
