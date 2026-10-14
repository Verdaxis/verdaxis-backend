"""Execution, grace membership, assisted organization and profile contracts."""
from datetime import UTC, datetime, timedelta
from uuid import uuid4
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest
from fastapi import HTTPException
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.database import Base
from app.models.kyc_execution_grace import KYCExecutionGrace
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.routers.auth_simple import read_users_me
from app.services.execution_policy import customer_organization_is_execution_eligible
from app.services.kyc_execution import kyc_execution_eligibility


@pytest.fixture
async def kyc_db():
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=[Organization.__table__, User.__table__, KYCExecutionGrace.__table__])
    async with async_sessionmaker(engine, expire_on_commit=False)() as db:
        yield db
    await engine.dispose()


async def member(db):
    org = Organization(id=uuid4(), name="KYC customer", type=OrgType.FUEL_BUYER, verification_status="APPROVED")
    user = User(id=uuid4(), email="member@example.com", password_hash="unused", role=UserRole.BUYER,
                status=UserStatus.APPROVED, email_verified=True, must_change_password=False,
                organization_id=org.id, kyc_status="PENDING")
    db.add_all([org, user])
    await db.flush()
    return org, user


@pytest.mark.asyncio
async def test_upload_does_not_renew_grace_and_membership_change_voids_it(kyc_db):
    org, user = await member(kyc_db)
    now = datetime.now(UTC)
    grace = KYCExecutionGrace(user_id=user.id, organization_id=org.id, kyc_status_at_enrollment="PENDING",
                              enrolled_at=now, expires_at=now + timedelta(days=14), created_by="kycg_20261014_staging")
    kyc_db.add(grace)
    await kyc_db.flush()
    assert (await kyc_execution_eligibility(kyc_db, user)).reason == "KYC_GRACE"
    user.kyc_status = "SUBMITTED"
    user.kyc_organization_id = org.id
    await kyc_db.flush()
    assert (await kyc_execution_eligibility(kyc_db, user)).grace_expires_at == grace.expires_at
    user.organization_id = uuid4()
    await kyc_db.flush()
    assert (await kyc_execution_eligibility(kyc_db, user)).reason == "KYC_REQUIRED"


@pytest.mark.asyncio
async def test_assisted_customer_requires_one_eligible_member(kyc_db):
    org, user = await member(kyc_db)
    assert not await customer_organization_is_execution_eligible(kyc_db, org)
    user.kyc_status = "APPROVED"
    user.kyc_organization_id = org.id
    await kyc_db.flush()
    assert await customer_organization_is_execution_eligible(kyc_db, org)
    user.email_verified = False
    await kyc_db.flush()
    assert not await customer_organization_is_execution_eligible(kyc_db, org)


@pytest.mark.asyncio
async def test_profile_reports_current_kyc_and_grace(kyc_db):
    org, user = await member(kyc_db)
    user.kyc_status = "APPROVED"
    user.kyc_organization_id = None
    profile = (await read_users_me(user, kyc_db)).model_dump(mode="json")
    assert profile["kyc_status"] == "APPROVED"
    assert profile["execution_eligibility"]["reason"] == "KYC_REQUIRED"
    now = datetime.now(UTC)
    kyc_db.add(KYCExecutionGrace(user_id=user.id, organization_id=org.id, kyc_status_at_enrollment="PENDING",
                                enrolled_at=now, expires_at=now + timedelta(days=14), created_by="kycg_20261014_staging"))
    await kyc_db.flush()
    profile = (await read_users_me(user, kyc_db)).model_dump(mode="json")
    assert profile["execution_eligibility"]["reason"] == "KYC_GRACE"
    assert profile["execution_eligibility"]["grace_expires_at"]
    user.role = UserRole.ADMIN
    profile = (await read_users_me(user, kyc_db)).model_dump(mode="json")
    assert profile["execution_eligibility"]["reason"] == "NOT_APPLICABLE"


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["create", "amend", "publish", "take", "confirm"])
async def test_new_execution_endpoints_return_kyc_required(operation, kyc_db):
    from app.routers import orderbook, inventory, trades
    from app.schemas.orderbook import OrderCreate, OrderUpdate, TradeCreate
    from app.market_catalog import PRODUCT_IDS, DELIVERY_POINT_IDS

    _, user = await member(kyc_db)
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))
    with pytest.raises(HTTPException) as error:
        if operation == "create":
            await orderbook.create_order(request=request, current_user=user, db=kyc_db,
                order_data=OrderCreate(side="BID", product_id=PRODUCT_IDS["BIO_METHANOL"],
                    delivery_point_id=DELIVERY_POINT_IDS["Singapore"], quantity_mt=100,
                    price_per_mt_usd=500, availability_window="SPOT"))
        elif operation == "amend":
            await orderbook.update_order(uuid4(), request, OrderUpdate(price_per_mt_usd=500), user, kyc_db)
        elif operation == "publish":
            await inventory.publish_inventory_item(uuid4(), request, kyc_db, user)
        elif operation == "take":
            await trades.create_trade(TradeCreate(order_id=uuid4(), quantity_mt=10), request, kyc_db, user)
        else:
            await trades.confirm_trade(uuid4(), request, kyc_db, user)
    assert error.value.status_code == 403
    assert error.value.detail["code"] == "KYC_REQUIRED"


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["amend", "confirm"])
async def test_committed_command_replay_precedes_kyc(operation, monkeypatch, kyc_db):
    from app.routers import orderbook, trades
    from app.schemas.orderbook import OrderUpdate

    _, user = await member(kyc_db)
    request = SimpleNamespace(headers={"Idempotency-Key": "committed"})
    router = orderbook if operation == "amend" else trades
    receipt = SimpleNamespace(response_status=200, response_body={"committed": True})
    monkeypatch.setattr(router, "prepare_command_attempt", AsyncMock(return_value=SimpleNamespace(replay=receipt)))
    guard = AsyncMock(side_effect=AssertionError("replays do not execute"))
    monkeypatch.setattr(router, "require_execution_eligible_user", guard)
    if operation == "amend":
        response = await router.update_order(uuid4(), request, OrderUpdate(price_per_mt_usd=500), user, kyc_db)
    else:
        response = await router.confirm_trade(uuid4(), request, kyc_db, user)
    assert response.status_code == 200
    assert response.body == b'{"committed":true}'
    guard.assert_not_awaited()


@pytest.mark.asyncio
async def test_assisted_revalidation_refuses_member_rejected_before_lock():
    from sqlalchemy.dialects import postgresql
    from app.services.execution_policy import require_customer_organization_execution

    org = SimpleNamespace(id=uuid4(), verification_status="APPROVED")
    admitted = Mock(scalar_one_or_none=lambda: uuid4())
    rejected = Mock(scalar_one_or_none=lambda: None)
    db = SimpleNamespace(execute=AsyncMock(side_effect=[admitted, rejected]))
    await require_customer_organization_execution(db, org)
    with pytest.raises(HTTPException) as error:
        await require_customer_organization_execution(db, org)
    assert error.value.detail["code"] == "KYC_REQUIRED"
    sql = str(db.execute.call_args.args[0].compile(dialect=postgresql.dialect()))
    assert "ORDER BY users.id" in sql
    assert "FOR SHARE SKIP LOCKED" in sql
    assert "kyc_execution_grace" in sql
