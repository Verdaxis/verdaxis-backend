"""Behavioral contracts for independent tenant-membership review."""

from datetime import UTC, datetime, timedelta
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from fastapi import Request
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.audit import AuditLog
from app.models.product_analytics import UserStatusTransition
from app.models.registration import JoinRequestStatus, OrganizationJoinRequest
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.models.catalog import Product, DeliveryPoint
from app.models.marketplace import InventoryItem
from app.models.orderbook import OrderBookOrder
from app.models.rfq import RFQ, RFQQuote
from app.models.negotiation import Negotiation
from app.routers.auth_simple import (
    AdminDecisionBody,
    JoinReviewBody,
    approve_organization,
    approve_organization_join,
    approve_user,
    admin_review_detail,
    admin_review_queue,
    reject_organization_join,
    reject_user,
)
from app.services.audit_actions import ORGANIZATION_JOIN_APPROVED, ORGANIZATION_JOIN_REJECTED


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


@pytest.fixture
async def join_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = [
        Organization.__table__,
        User.__table__,
        OrganizationJoinRequest.__table__,
        Product.__table__,
        DeliveryPoint.__table__,
        InventoryItem.__table__,
        OrderBookOrder.__table__,
        RFQ.__table__,
        RFQQuote.__table__,
        Negotiation.__table__,
        AuditLog.__table__,
        UserStatusTransition.__table__,
    ]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


async def _seed(join_db: AsyncSession):
    organization = Organization(
        id=uuid4(),
        name="Reviewed Tenant",
        domain="reviewed.example",
        type=OrgType.FUEL_BUYER,
        verification_status="PENDING",
    )
    admin = User(
        id=uuid4(),
        email=f"admin-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.ADMIN,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    candidate = User(
        id=uuid4(),
        email=f"candidate-{uuid4()}@reviewed.example",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.PENDING,
        email_verified=True,
        organization_id=None,
    )
    join_request = OrganizationJoinRequest(
        id=uuid4(),
        user_id=candidate.id,
        organization_id=organization.id,
        status=JoinRequestStatus.PENDING,
    )
    join_db.add_all([organization, admin, candidate, join_request])
    await join_db.commit()
    return organization, admin, candidate, join_request


@pytest.mark.asyncio
async def test_user_and_org_admission_do_not_grant_membership(join_db: AsyncSession):
    organization, admin, candidate, join_request = await _seed(join_db)

    await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )
    assert candidate.status == UserStatus.APPROVED
    assert candidate.organization_id is None
    assert join_request.status == JoinRequestStatus.PENDING

    await approve_organization(
        request=_request(f"/api/auth/organization/{organization.id}/approve"),
        organization_id=organization.id,
        current_user=admin,
        db=join_db,
    )
    assert organization.verification_status == "APPROVED"
    assert candidate.organization_id is None
    assert join_request.status == JoinRequestStatus.PENDING

    await approve_organization_join(
        request=_request(f"/api/auth/organization-joins/{join_request.id}/approve"),
        join_request_id=join_request.id,
        body=JoinReviewBody(review_note="Corporate domain and tenant request reviewed."),
        current_user=admin,
        db=join_db,
    )
    assert candidate.organization_id == organization.id
    assert join_request.status == JoinRequestStatus.APPROVED
    assert join_request.reviewed_by == admin.id
    audit = await join_db.scalar(
        select(AuditLog).where(AuditLog.action == ORGANIZATION_JOIN_APPROVED)
    )
    assert audit is not None


@pytest.mark.asyncio
async def test_account_approval_sends_email_once_after_commit(
    join_db: AsyncSession,
    monkeypatch,
):
    _organization, admin, candidate, _join_request = await _seed(join_db)
    committed = False
    original_commit = join_db.commit

    async def tracked_commit():
        nonlocal committed
        await original_commit()
        committed = True

    async def deliver_approval_email(_db, *, user_id, transition_id) -> str:
        assert committed is True
        assert user_id == candidate.id
        assert transition_id == candidate.pending_approval_email_transition_id
        return "sent"

    monkeypatch.setattr(join_db, "commit", tracked_commit)
    deliver = AsyncMock(side_effect=deliver_approval_email)
    monkeypatch.setattr(
        "app.routers.auth_simple.deliver_account_approval_email",
        deliver,
    )

    await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )
    await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )

    assert candidate.status == UserStatus.APPROVED
    assert candidate.pending_approval_email_transition_id is not None
    assert candidate.pending_approval_email_payload["to"] == [candidate.email]
    assert candidate.pending_approval_email_retry_at is not None
    deliver.assert_awaited_once()


@pytest.mark.asyncio
async def test_account_approval_survives_email_provider_failure(
    join_db: AsyncSession,
    monkeypatch,
):
    _organization, admin, candidate, _join_request = await _seed(join_db)
    deliver = AsyncMock(return_value=False)
    monkeypatch.setattr(
        "app.routers.auth_simple.deliver_account_approval_email",
        deliver,
    )

    approved = await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )

    assert approved.status == UserStatus.APPROVED
    assert candidate.pending_approval_email_transition_id is not None
    assert candidate.pending_approval_email_payload is not None
    assert candidate.pending_approval_email_retry_at is not None
    deliver.assert_awaited_once()


@pytest.mark.asyncio
async def test_account_approval_survives_unexpected_delivery_failure(
    join_db: AsyncSession,
    monkeypatch,
):
    _organization, admin, candidate, _join_request = await _seed(join_db)
    deliver = AsyncMock(side_effect=RuntimeError("unexpected delivery failure"))
    monkeypatch.setattr(
        "app.routers.auth_simple.deliver_account_approval_email",
        deliver,
    )

    approved = await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )

    assert approved.status == UserStatus.APPROVED
    assert candidate.pending_approval_email_transition_id is not None
    assert candidate.pending_approval_email_payload is not None
    deliver.assert_awaited_once()


@pytest.mark.asyncio
async def test_rejected_account_reapproval_creates_a_new_delivery_marker(
    join_db: AsyncSession,
    monkeypatch,
):
    _organization, admin, candidate, _join_request = await _seed(join_db)
    candidate.status = UserStatus.REJECTED
    await join_db.commit()
    deliver = AsyncMock(return_value=False)
    monkeypatch.setattr(
        "app.routers.auth_simple.deliver_account_approval_email",
        deliver,
    )

    await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )

    assert candidate.status == UserStatus.APPROVED
    assert candidate.pending_approval_email_transition_id is not None
    assert candidate.pending_approval_email_payload is not None
    deliver.assert_awaited_once()


@pytest.mark.asyncio
async def test_rejection_invalidates_unsent_account_approval_email(
    join_db: AsyncSession,
    monkeypatch,
):
    _organization, admin, candidate, _join_request = await _seed(join_db)
    monkeypatch.setattr(
        "app.routers.auth_simple.deliver_account_approval_email",
        AsyncMock(return_value="deferred"),
    )
    await approve_user(
        request=_request(f"/api/auth/approve/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )

    await reject_user(
        request=_request(f"/api/auth/reject/{candidate.id}"),
        user_id=candidate.id,
        body=AdminDecisionBody(reason="Approval was entered in error."),
        current_user=admin,
        db=join_db,
    )

    assert candidate.status == UserStatus.REJECTED
    assert candidate.pending_approval_email_transition_id is None
    assert candidate.pending_approval_email_payload is None
    assert candidate.pending_approval_email_retry_at is None


@pytest.mark.asyncio
async def test_membership_approval_rejects_opposite_side(join_db: AsyncSession):
    organization, admin, candidate, join_request = await _seed(join_db)
    organization.verification_status = "APPROVED"
    candidate.status = UserStatus.APPROVED
    candidate.email_verified = True
    candidate.role = UserRole.SUPPLIER
    await join_db.commit()

    with pytest.raises(Exception) as exc_info:
        await approve_organization_join(
            request=_request(f"/api/auth/organization-joins/{join_request.id}/approve"),
            join_request_id=join_request.id,
            body=JoinReviewBody(review_note="Role and organization checked."),
            current_user=admin,
            db=join_db,
        )

    assert exc_info.value.status_code == 409
    assert exc_info.value.detail["code"] == "ORGANIZATION_JOIN_ROLE_MISMATCH"
    assert candidate.organization_id is None
    assert join_request.status == JoinRequestStatus.PENDING


@pytest.mark.asyncio
async def test_join_rejection_changes_only_the_join_transition(join_db: AsyncSession):
    organization, admin, candidate, join_request = await _seed(join_db)
    original_user_status = candidate.status
    original_org_status = organization.verification_status

    await reject_organization_join(
        request=_request(f"/api/auth/organization-joins/{join_request.id}/reject"),
        join_request_id=join_request.id,
        body=JoinReviewBody(review_note="Membership evidence was not sufficient."),
        current_user=admin,
        db=join_db,
    )

    assert join_request.status == JoinRequestStatus.REJECTED
    assert candidate.organization_id is None
    assert candidate.status == original_user_status
    assert organization.verification_status == original_org_status
    audit = await join_db.scalar(
        select(AuditLog).where(AuditLog.action == ORGANIZATION_JOIN_REJECTED)
    )
    assert audit is not None


@pytest.mark.asyncio
async def test_admin_review_queue_and_detail_expose_bounded_case_metadata(join_db: AsyncSession):
    organization, admin, candidate, join_request = await _seed(join_db)

    queue = await admin_review_queue(
        request=_request("/api/auth/admin/review-queue"),
        current_user=admin,
        db=join_db,
        limit=50,
    )
    case = next(item for item in queue["items"] if item["user_id"] == str(candidate.id))
    assert case["email"] == candidate.email
    assert case["email_verified"] is True
    assert case["account_status"] == "PENDING"
    assert case["current_organization"] is None
    assert case["requested_organizations"][0]["request_id"] == str(join_request.id)
    assert case["requested_organizations"][0]["organization"]["id"] == str(organization.id)

    detail = await admin_review_detail(
        request=_request(f"/api/auth/admin/review-queue/{candidate.id}"),
        user_id=candidate.id,
        current_user=admin,
        db=join_db,
    )
    assert detail == case

    with pytest.raises(Exception) as exc_info:
        await admin_review_queue(
            request=_request("/api/auth/admin/review-queue"),
            current_user=candidate,
            db=join_db,
            limit=50,
        )
    assert exc_info.value.status_code == 403


@pytest.mark.asyncio
async def test_admin_review_projection_caps_join_history_per_candidate_without_starvation(
    join_db: AsyncSession,
):
    _organization, admin, heavy_candidate, _join_request = await _seed(join_db)
    single_candidate = User(
        id=uuid4(),
        email=f"single-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.PENDING,
        email_verified=True,
    )
    now = datetime.now(UTC)
    heavy_organizations = [
        Organization(
            id=uuid4(),
            name=f"Heavy request {index}",
            domain=f"heavy-{index}-{uuid4()}.example",
            type=OrgType.FUEL_BUYER,
            verification_status="PENDING",
        )
        for index in range(40)
    ]
    single_organization = Organization(
        id=uuid4(),
        name="Single request",
        domain=f"single-{uuid4()}.example",
        type=OrgType.FUEL_BUYER,
        verification_status="PENDING",
    )
    join_db.add_all([single_candidate, single_organization, *heavy_organizations])
    await join_db.flush()
    join_db.add_all(
        [
            OrganizationJoinRequest(
                id=uuid4(),
                user_id=heavy_candidate.id,
                organization_id=organization.id,
                status=JoinRequestStatus.PENDING,
                created_at=now + timedelta(minutes=index + 1),
            )
            for index, organization in enumerate(heavy_organizations)
        ]
        + [
            OrganizationJoinRequest(
                id=uuid4(),
                user_id=single_candidate.id,
                organization_id=single_organization.id,
                status=JoinRequestStatus.PENDING,
                created_at=now - timedelta(days=1),
            )
        ]
    )
    await join_db.commit()

    queue = await admin_review_queue(
        request=_request("/api/auth/admin/review-queue"),
        current_user=admin,
        db=join_db,
        limit=50,
    )
    by_user = {item["user_id"]: item for item in queue["items"]}

    assert len(by_user[str(heavy_candidate.id)]["requested_organizations"]) == 20
    assert len(by_user[str(single_candidate.id)]["requested_organizations"]) == 1
    assert (
        by_user[str(single_candidate.id)]["requested_organizations"][0]["organization"]["id"]
        == str(single_organization.id)
    )
