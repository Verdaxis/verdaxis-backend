"""Route-level PostgreSQL NOWAIT contention contracts for security invalidation."""

from __future__ import annotations

from decimal import Decimal
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.models.catalog import Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.registration import JoinRequestStatus, OrganizationJoinRequest
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.routers import admin_analytics, auth_simple, kyc


def _request(path: str) -> Request:
    return Request(
        {
            "type": "http",
            "method": "PUT",
            "path": path,
            "query_string": b"",
            "headers": [],
            "client": ("127.0.0.1", 1234),
            "scheme": "https",
            "server": ("test", 443),
        }
    )


async def _seed_contention_case(session: AsyncSession, transition: str):
    current_org = Organization(
        id=uuid4(),
        name=f"Current org {uuid4()}",
        type=OrgType.FUEL_SUPPLIER,
        verification_status="APPROVED",
    )
    requested_org = Organization(
        id=uuid4(),
        name=f"Requested org {uuid4()}",
        type=OrgType.FUEL_SUPPLIER,
        verification_status="APPROVED",
    )
    product = Product(
        id=uuid4(),
        name=f"Contention product {uuid4()}",
        fuel_type="Methanol",
        fuel_grade="Bio",
    )
    admin = User(
        id=uuid4(),
        email=f"admin-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.ADMIN,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    target = User(
        id=uuid4(),
        email=f"target-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=None if transition == "membership" else current_org.id,
        kyc_status="SUBMITTED" if transition == "kyc" else "PENDING",
        kyc_organization_id=current_org.id if transition == "kyc" else None,
    )
    session.add_all((current_org, requested_org, product, admin, target))
    await session.flush()
    order = OrderBookOrder(
        organization_id=current_org.id,
        owner_user_id=target.id,
        side=OrderSide.ASK,
        product_id=product.id,
        quantity_mt=Decimal("10"),
        remaining_quantity_mt=Decimal("10"),
        price_per_mt_usd=Decimal("100"),
        certification_declared=True,
        certification_scheme="ISCC",
        status=OrderBookStatus.OPEN,
    )
    join_request = OrganizationJoinRequest(
        id=uuid4(),
        user_id=target.id,
        organization_id=requested_org.id,
        status=JoinRequestStatus.PENDING,
    )
    session.add_all((order, join_request))
    await session.commit()
    return admin, target, current_org, order, join_request


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "transition",
    ("user", "analytics_user", "organization", "kyc", "membership"),
)
async def test_security_review_routes_translate_order_lock_contention_and_rollback(
    pg_session,
    transition,
    monkeypatch,
):
    engine, session = pg_session
    admin, target, organization, order, join_request = await _seed_contention_case(
        session, transition
    )
    target_id = target.id
    organization_id = organization.id
    order_id = order.id
    join_request_id = join_request.id
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    auth_publish = AsyncMock()
    analytics_publish = AsyncMock()
    kyc_publish = AsyncMock()
    monkeypatch.setattr(auth_simple, "publish_execution_invalidation", auth_publish)
    monkeypatch.setattr(admin_analytics, "publish_execution_invalidation", analytics_publish)
    monkeypatch.setattr(kyc, "publish_execution_invalidation", kyc_publish)

    async with factory() as competing:
        await competing.scalar(
            select(OrderBookOrder).where(OrderBookOrder.id == order_id).with_for_update()
        )

        with pytest.raises(HTTPException) as exc_info:
            if transition == "user":
                await auth_simple.reject_user(
                    request=_request(f"/api/auth/reject/{target_id}"),
                    user_id=target_id,
                    body=auth_simple.AdminDecisionBody(reason="security review"),
                    current_user=admin,
                    db=session,
                )
            elif transition == "analytics_user":
                await admin_analytics.reject_user(
                    request=_request(f"/api/admin/users/{target_id}/reject"),
                    user_id=target_id,
                    current_user=admin,
                    db=session,
                )
            elif transition == "organization":
                await auth_simple.reject_organization(
                    request=_request(f"/api/auth/organization/{organization_id}/reject"),
                    organization_id=organization_id,
                    body=auth_simple.AdminDecisionBody(reason="security review"),
                    current_user=admin,
                    db=session,
                )
            elif transition == "kyc":
                await kyc.admin_reject_kyc(
                    user_id=target_id,
                    request=_request(f"/api/kyc/admin/{target_id}/reject"),
                    body=kyc.AdminRejectBody(reason="security review"),
                    current_user=admin,
                    db=session,
                )
            else:
                await auth_simple.approve_organization_join(
                    request=_request(
                        f"/api/auth/organization-joins/{join_request_id}/approve"
                    ),
                    join_request_id=join_request_id,
                    body=auth_simple.JoinReviewBody(review_note="security review"),
                    current_user=admin,
                    db=session,
                )

        assert exc_info.value.status_code == 409
        assert exc_info.value.headers == {"Retry-After": "1"}
        assert exc_info.value.detail["code"] == "EXECUTION_INVALIDATION_BUSY"
        await competing.rollback()

    session.expire_all()
    persisted_target = await session.get(User, target_id)
    persisted_org = await session.get(Organization, organization_id)
    persisted_order = await session.get(OrderBookOrder, order_id)
    persisted_join = await session.get(OrganizationJoinRequest, join_request_id)
    assert persisted_target.status == UserStatus.APPROVED
    assert persisted_target.kyc_status == ("SUBMITTED" if transition == "kyc" else "PENDING")
    assert persisted_org.verification_status == "APPROVED"
    assert persisted_order.status == OrderBookStatus.OPEN
    assert persisted_join.status == JoinRequestStatus.PENDING
    auth_publish.assert_not_awaited()
    analytics_publish.assert_not_awaited()
    kyc_publish.assert_not_awaited()
