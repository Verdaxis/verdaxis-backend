"""Production contracts retained on the staging-derived Phase 0 backend."""

from datetime import UTC, datetime
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.config import settings
from app.database import Base
from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.models.notification import Notification, NotificationType
from app.models.user import User, UserRole, UserStatus
from app.routers import auth_simple, notifications
from app.schemas.lng_order import LngAskTerms, LngBidTerms
from app.schemas.user import UserResponse, UserUpdate
from app.services.lng_order import lng_metadata_fields
from app.services.market_catalog_validation import require_canonical_market_slice
from tests.unit.test_lng_order import ASK, BID


@pytest.mark.parametrize("schema,payload,field", [
    (LngAskTerms, ASK, "union_database_registered"),
    (LngAskTerms, ASK, "seller_is_physical_supplier"),
    (LngBidTerms, BID, "require_union_database"),
    (LngBidTerms, BID, "accept_resellers"),
])
@pytest.mark.parametrize("value", ["false", 0, 1])
def test_lng_booleans_do_not_coerce(schema, payload, field, value):
    with pytest.raises(ValidationError):
        schema.model_validate({**payload, field: value})


@pytest.mark.parametrize("scheme,label", [("ISCC_EU", "ISCC EU"), ("REDCERT_EU", "REDcert-EU")])
def test_lng_scalar_certifications_match_production(scheme, label):
    ask = lng_metadata_fields({**ASK, "sustainability_scheme": scheme})
    bid = lng_metadata_fields({**BID, "accepted_schemes": [scheme]})
    assert ask["certifications"] == bid["certifications"] == [label]
    assert ask["certification_scheme"] == label
    assert bid["certification_scheme"] is None
    for fields in (ask, bid):
        assert fields["carbon_intensity_gco2_mj"] is None
        assert fields["carbon_intensity_method"] is None


@pytest.mark.asyncio
@pytest.mark.parametrize("gate", [True, False])
@pytest.mark.parametrize("kyc,role,has_org,moved", [
    ("PENDING", UserRole.BUYER, True, False),
    ("APPROVED", UserRole.SUPPLIER, True, False),
    ("REJECTED", UserRole.BUYER, True, False),
    ("APPROVED", UserRole.BUYER, True, True),
    ("PENDING", UserRole.ADMIN, True, False),
    ("PENDING", UserRole.BUYER, False, False),
])
async def test_put_profile_matches_get(gate, kyc, role, has_org, moved, monkeypatch):
    monkeypatch.setattr(settings, "KYC_EXECUTION_GATE_ENABLED", gate)
    org_id = uuid4() if has_org else None
    user = User(id=uuid4(), email="profile@example.com", role=role,
                status=UserStatus.APPROVED, organization_id=org_id,
                must_change_password=False, kyc_status=kyc,
                kyc_organization_id=uuid4() if moved else org_id)
    db = AsyncMock()
    db.scalar.return_value = None
    updated = await auth_simple.update_users_me(UserUpdate(first_name="Updated"), user, db)
    current = await auth_simple.read_users_me(user, db)
    assert UserResponse.model_validate(updated).model_dump() == current.model_dump()
    assert current.first_name == "Updated"
    if kyc == "REJECTED" or moved:
        assert not current.execution_eligibility.eligible


@pytest.mark.asyncio
async def test_copied_notification_privacy_and_counts():
    engine = create_async_engine("sqlite+aiosqlite://")
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=[Notification.__table__])
    owner, other = SimpleNamespace(id=uuid4()), SimpleNamespace(id=uuid4())
    async with async_sessionmaker(engine, expire_on_commit=False)() as db:
        original = {"event": "order_expiring", "_email_status": "pending",
                    "_email_payload": {"to": ["private@example.com"]}, "_email_retry_at": 123}
        for recipient, data in [(owner, original), (owner, {**original, "_email_only": True}),
                                (owner, None), (other, original)]:
            db.add(Notification(recipient_id=recipient.id, type=NotificationType.ORDER_UPDATE,
                                title="Reminder", message="Synthetic", data=data,
                                created_at=datetime.now(UTC)))
        await db.commit()
        rows = await notifications.get_notifications(skip=0, limit=100, current_user=owner, db=db)
        assert len(rows) == 2
        assert await notifications.get_unread_count(current_user=owner, db=db) == {"count": 2}
        for row in rows:
            public = notifications.NotificationResponse.model_validate(row).model_dump()
            assert not any(key.startswith("_email_") for key in public["data"] or {})
        assert original["_email_payload"]["to"] == ["private@example.com"]
    await engine.dispose()


@pytest.mark.asyncio
async def test_lng_restricted_lane_names_rotterdam():
    db = AsyncMock()
    result = MagicMock()
    result.scalar_one_or_none.return_value = SimpleNamespace(id=PRODUCT_IDS["BIO_LNG"])
    db.execute.return_value = result
    with pytest.raises(HTTPException) as caught:
        await require_canonical_market_slice(db, product_id=PRODUCT_IDS["BIO_LNG"],
                                             delivery_point_id=DELIVERY_POINT_IDS["SINGAPORE"])
    assert caught.value.status_code == 400
    assert "Rotterdam" in caught.value.detail
    assert "Singapore" not in caught.value.detail
