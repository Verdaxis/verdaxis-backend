"""Focused proofs for the seller-paid transaction-fee contract."""
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError

from app.config import Settings
from app.models.subscription import Subscription, SubscriptionTier
from app.routers.subscriptions import get_fee_schedule, update_subscription
from app.schemas.subscription import SubscriptionUpdate
from app.schemas.subscription import FeeScheduleResponse
from app.services.trade_fees import public_fee_schedule, resolve_seller_trade_fee


def _settings(**overrides) -> Settings:
    values = {
        "ENVIRONMENT": "test",
        "RELEASE_SHA": "test",
        "DATABASE_URL": "sqlite+aiosqlite:///:memory:",
        "JWT_SECRET": "test-secret-key-for-fee-tests-is-long-enough",
    }
    values.update(overrides)
    return Settings(**values)


def _request():
    return MagicMock(headers={}, client=MagicMock(host="127.0.0.1"))


def _db_returning(subscription):
    result = MagicMock()
    result.scalar_one_or_none.return_value = subscription
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.return_value = result
    return db


@pytest.mark.asyncio
async def test_public_schedule_is_buyer_free_and_server_configurable():
    response = await get_fee_schedule()
    assert response["currency"] == "USD"
    assert response["buyer_fee_per_mt_usd"] == Decimal("0")
    assert response["seller_fee_per_mt_usd"] == {
        SubscriptionTier.FREE: Decimal("2.00"),
        SubscriptionTier.STANDARD: Decimal("1.50"),
        SubscriptionTier.ENTERPRISE: None,
    }
    assert FeeScheduleResponse.model_validate(response).model_dump(mode="json") == {
        "currency": "USD",
        "buyer_fee_per_mt_usd": "0",
        "seller_fee_per_mt_usd": {
            "free": "2.00",
            "standard": "1.50",
            "enterprise": None,
        },
    }

    configured = _settings(
        SELLER_FEE_PILOT_PER_MT_USD="2.25",
        SELLER_FEE_PROFESSIONAL_PER_MT_USD="1.25",
    )
    assert configured.SELLER_FEE_PILOT_PER_MT_USD == Decimal("2.25")
    assert configured.SELLER_FEE_PROFESSIONAL_PER_MT_USD == Decimal("1.25")
    assert public_fee_schedule(configured)["seller_fee_per_mt_usd"] == {
        SubscriptionTier.FREE: Decimal("2.25"),
        SubscriptionTier.STANDARD: Decimal("1.25"),
        SubscriptionTier.ENTERPRISE: None,
    }
    configured_standard = Subscription(
        org_id=uuid4(),
        tier=SubscriptionTier.STANDARD,
    )
    assert await resolve_seller_trade_fee(
        _db_returning(configured_standard),
        configured_standard.org_id,
        configuration=configured,
    ) == (SubscriptionTier.STANDARD, Decimal("1.25"))
    with pytest.raises(ValidationError):
        _settings(SELLER_FEE_PILOT_PER_MT_USD="2.001")


@pytest.mark.asyncio
async def test_resolver_uses_active_seller_plan_and_pilot_for_expired_plan():
    seller_id = uuid4()
    standard = Subscription(
        org_id=seller_id,
        tier=SubscriptionTier.STANDARD,
        expires_at=datetime.now(UTC) + timedelta(days=1),
    )
    assert await resolve_seller_trade_fee(_db_returning(standard), seller_id) == (
        SubscriptionTier.STANDARD,
        Decimal("1.50"),
    )

    standard.expires_at = datetime.now(UTC) - timedelta(seconds=1)
    assert await resolve_seller_trade_fee(_db_returning(standard), seller_id) == (
        SubscriptionTier.FREE,
        Decimal("2.00"),
    )


@pytest.mark.asyncio
async def test_enterprise_requires_audited_negotiated_rate():
    organization_id = uuid4()
    enterprise = Subscription(
        org_id=organization_id,
        tier=SubscriptionTier.ENTERPRISE,
    )
    with pytest.raises(HTTPException) as error:
        await resolve_seller_trade_fee(_db_returning(enterprise), organization_id)
    assert error.value.status_code == 409
    assert "enterprise" not in error.value.detail.lower()

    invalid_update_db = _db_returning(enterprise)
    with pytest.raises(HTTPException) as update_error:
        await update_subscription(
            org_id=organization_id,
            request=_request(),
            body=SubscriptionUpdate(tier=SubscriptionTier.ENTERPRISE),
            current_user=MagicMock(id=uuid4()),
            db=invalid_update_db,
        )
    assert update_error.value.status_code == 422
    invalid_update_db.commit.assert_not_awaited()

    db = _db_returning(enterprise)
    admin = MagicMock(id=uuid4())
    updated = await update_subscription(
        org_id=organization_id,
        request=_request(),
        body=SubscriptionUpdate(
            tier=SubscriptionTier.ENTERPRISE,
            seller_fee_per_mt_usd="0.80",
        ),
        current_user=admin,
        db=db,
    )
    assert updated.seller_fee_per_mt_usd == Decimal("0.80")
    assert await resolve_seller_trade_fee(
        _db_returning(updated),
        organization_id,
    ) == (SubscriptionTier.ENTERPRISE, Decimal("0.80"))
    audit_entry = db.add.call_args_list[-1].args[0]
    assert audit_entry.changes["seller_fee_per_mt_usd"] == {
        "from": None,
        "to": "0.80",
    }


def test_subscription_update_rejects_invalid_negotiated_rate():
    with pytest.raises(ValidationError):
        SubscriptionUpdate(
            tier=SubscriptionTier.ENTERPRISE,
            seller_fee_per_mt_usd="-0.01",
        )
    with pytest.raises(ValidationError):
        SubscriptionUpdate(
            tier=SubscriptionTier.ENTERPRISE,
            seller_fee_per_mt_usd="1.001",
        )


@pytest.mark.asyncio
async def test_admin_fee_override_is_preserved_when_omitted_and_cleared_explicitly():
    organization_id = uuid4()
    subscription = Subscription(
        org_id=organization_id,
        tier=SubscriptionTier.ENTERPRISE,
        seller_fee_per_mt_usd=Decimal("0.80"),
    )
    admin = MagicMock(id=uuid4())

    preserve_db = _db_returning(subscription)
    await update_subscription(
        org_id=organization_id,
        request=_request(),
        body=SubscriptionUpdate(tier=SubscriptionTier.ENTERPRISE),
        current_user=admin,
        db=preserve_db,
    )
    assert subscription.seller_fee_per_mt_usd == Decimal("0.80")
    preserve_audit = preserve_db.add.call_args_list[-1].args[0]
    assert "seller_fee_per_mt_usd" not in preserve_audit.changes

    clear_db = _db_returning(subscription)
    await update_subscription(
        org_id=organization_id,
        request=_request(),
        body=SubscriptionUpdate(
            tier=SubscriptionTier.STANDARD,
            seller_fee_per_mt_usd=None,
        ),
        current_user=admin,
        db=clear_db,
    )
    assert subscription.seller_fee_per_mt_usd is None
    clear_audit = clear_db.add.call_args_list[-1].args[0]
    assert clear_audit.changes["seller_fee_per_mt_usd"] == {
        "from": "0.80",
        "to": None,
    }
