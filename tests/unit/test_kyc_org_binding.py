from datetime import UTC, datetime, timedelta
from io import BytesIO
from types import SimpleNamespace
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request, UploadFile
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from starlette.datastructures import Headers

from app.database import Base
from app.models.kyc_execution_grace import KycExecutionGrace
from app.models.user import Organization, OrganizationProvenance, OrgType, User, UserRole, UserStatus
from app.routers import kyc as kyc_router
from app.routers.kyc import _require_current_kyc_organization
from app.services.execution_policy import MarketPartyPolicy, market_party_is_eligible
from app.services.kyc_execution import ExecutionEligibilityReason, evaluate_kyc_execution

# The migration's one enrollment instant and its fixed 14-day grace period.
ENROLLED_AT = datetime(2026, 10, 14, 12, 0, tzinfo=UTC)
GRACE_EXPIRES_AT = ENROLLED_AT + timedelta(days=14)
DURING_GRACE = ENROLLED_AT + timedelta(days=3)
REQUIRED_TABLES = ("organizations", "users", "kyc_execution_grace", "audit_logs")


def _user(org_id, *, kyc_status="PENDING", kyc_org_id=None):
    return SimpleNamespace(
        id=uuid4(), organization_id=org_id, role=UserRole.BUYER,
        status=UserStatus.APPROVED, email_verified=True, must_change_password=False,
        kyc_status=kyc_status, kyc_organization_id=kyc_org_id,
    )


def _org(org_id):
    return SimpleNamespace(id=org_id, verification_status="APPROVED")


@pytest.fixture
async def db():
    engine = create_async_engine("sqlite+aiosqlite://")
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


async def _organization(db: AsyncSession) -> Organization:
    organization = Organization(
        name=f"Org {uuid4().hex[:8]}",
        type=OrgType.FUEL_BUYER,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    db.add(organization)
    await db.flush()
    return organization


async def _member(
    db: AsyncSession,
    organization: Organization,
    *,
    kyc_status: str = "PENDING",
    kyc_bound: bool = False,
    role: UserRole = UserRole.BUYER,
) -> User:
    user = User(
        email=f"{uuid4().hex}@kyc.test",
        password_hash="unused",
        role=role,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=organization.id,
        kyc_status=kyc_status,
        kyc_organization_id=organization.id if kyc_bound else None,
    )
    db.add(user)
    await db.flush()
    return user


async def _enroll_grace(db: AsyncSession, user: User) -> KycExecutionGrace:
    """Write the row exactly as the migration's snapshot would."""
    grace = KycExecutionGrace(
        user_id=user.id,
        organization_id=user.organization_id,
        kyc_status_at_enrollment=user.kyc_status,
        enrolled_at=ENROLLED_AT,
        expires_at=GRACE_EXPIRES_AT,
        created_by="kycg_20261014_prod",
    )
    db.add(grace)
    await db.commit()
    return grace


@pytest.mark.asyncio
async def test_indicative_policy_keeps_pending_and_legacy_kyc_advisory():
    org_id = uuid4()
    assert await market_party_is_eligible(
        SimpleNamespace(),
        user=_user(org_id),
        organization=_org(org_id),
        policy=MarketPartyPolicy.INDICATIVE,
    )


@pytest.mark.asyncio
async def test_indicative_policy_still_revokes_rejected_or_foreign_bound_kyc():
    org_id = uuid4()
    for user in (
        _user(org_id, kyc_status="APPROVED", kyc_org_id=uuid4()),
        _user(org_id, kyc_status="REJECTED", kyc_org_id=org_id),
    ):
        assert not await market_party_is_eligible(
            SimpleNamespace(),
            user=user,
            organization=_org(org_id),
            policy=MarketPartyPolicy.INDICATIVE,
        )


@pytest.mark.asyncio
async def test_kyc_approved_for_the_current_organization_is_eligible(db):
    organization = await _organization(db)
    approved = await _member(db, organization, kyc_status="APPROVED", kyc_bound=True)
    approved_elsewhere = await _member(db, organization, kyc_status="APPROVED")
    approved_elsewhere.kyc_organization_id = (await _organization(db)).id
    await db.commit()

    eligibility = await evaluate_kyc_execution(db, approved)
    assert eligibility.eligible
    assert eligibility.reason == ExecutionEligibilityReason.KYC_APPROVED
    assert eligibility.grace_expires_at is None

    foreign = await evaluate_kyc_execution(db, approved_elsewhere)
    assert not foreign.eligible
    assert foreign.reason == ExecutionEligibilityReason.KYC_REQUIRED


@pytest.mark.asyncio
async def test_pending_kyc_without_grace_is_required(db):
    user = await _member(db, await _organization(db))

    eligibility = await evaluate_kyc_execution(db, user, now=DURING_GRACE)

    assert not eligibility.eligible
    assert eligibility.reason == ExecutionEligibilityReason.KYC_REQUIRED
    assert eligibility.kyc_status == "PENDING"
    assert eligibility.grace_expires_at is None


@pytest.mark.asyncio
async def test_grace_is_valid_until_its_exact_expiry(db):
    user = await _member(db, await _organization(db), kyc_status="SUBMITTED")
    await _enroll_grace(db, user)

    during = await evaluate_kyc_execution(db, user, now=DURING_GRACE)
    assert during.eligible
    assert during.reason == ExecutionEligibilityReason.KYC_GRACE
    assert during.grace_expires_at == GRACE_EXPIRES_AT

    last_moment = await evaluate_kyc_execution(
        db, user, now=GRACE_EXPIRES_AT - timedelta(microseconds=1)
    )
    assert last_moment.reason == ExecutionEligibilityReason.KYC_GRACE

    at_expiry = await evaluate_kyc_execution(db, user, now=GRACE_EXPIRES_AT)
    assert not at_expiry.eligible
    assert at_expiry.reason == ExecutionEligibilityReason.KYC_REQUIRED
    # A blocked user still learns which deadline passed.
    assert at_expiry.grace_expires_at == GRACE_EXPIRES_AT


@pytest.mark.asyncio
async def test_kyc_resubmission_does_not_renew_grace(db, monkeypatch):
    organization = await _organization(db)
    user = await _member(db, organization)
    await _enroll_grace(db, user)
    monkeypatch.setattr(
        kyc_router,
        "verify_document_with_gemini",
        AsyncMock(return_value={"passed": True}),
    )

    response = await kyc_router.submit_kyc(
        request=Request(
            {
                "type": "http",
                "method": "POST",
                "path": "/api/kyc/submit",
                "query_string": b"",
                "headers": [],
                "client": ("127.0.0.1", 1234),
                "scheme": "https",
                "server": ("test", 443),
            }
        ),
        current_user=user,
        db=db,
        passport=UploadFile(file=BytesIO(b"%PDF-passport"), filename="passport", headers=Headers({"content-type": "application/pdf"})),
        company_doc=UploadFile(file=BytesIO(b"%PDF-company"), filename="company", headers=Headers({"content-type": "application/pdf"})),
        declared_company_name=None,
        registration_number=None,
    )

    assert response["kyc_status"] == "SUBMITTED"
    rows = (await db.execute(select(KycExecutionGrace))).scalars().all()
    assert [(row.user_id, row.enrolled_at.replace(tzinfo=UTC)) for row in rows] == [
        (user.id, ENROLLED_AT)
    ]
    refreshed = await db.get(User, user.id)
    at_expiry = await evaluate_kyc_execution(db, refreshed, now=GRACE_EXPIRES_AT)
    assert at_expiry.reason == ExecutionEligibilityReason.KYC_REQUIRED


@pytest.mark.asyncio
async def test_membership_change_voids_grace(db):
    user = await _member(db, await _organization(db))
    await _enroll_grace(db, user)

    user.organization_id = (await _organization(db)).id
    await db.commit()

    eligibility = await evaluate_kyc_execution(db, user, now=DURING_GRACE)
    assert eligibility.reason == ExecutionEligibilityReason.KYC_REQUIRED
    assert eligibility.grace_expires_at is None


@pytest.mark.asyncio
async def test_rejection_overrides_grace(db):
    user = await _member(db, await _organization(db))
    await _enroll_grace(db, user)

    user.kyc_status = "REJECTED"
    user.kyc_organization_id = user.organization_id
    await db.commit()

    eligibility = await evaluate_kyc_execution(db, user, now=DURING_GRACE)
    assert not eligibility.eligible
    assert eligibility.reason == ExecutionEligibilityReason.KYC_REJECTED


@pytest.mark.asyncio
async def test_legacy_kyc_status_is_not_approved(db):
    user = await _member(db, await _organization(db), kyc_status="REVIEW_REQUIRED", kyc_bound=True)

    eligibility = await evaluate_kyc_execution(db, user, now=DURING_GRACE)

    assert eligibility.reason == ExecutionEligibilityReason.KYC_REQUIRED
    assert eligibility.kyc_status == "REVIEW_REQUIRED"


@pytest.mark.asyncio
async def test_admins_and_users_without_organization_are_not_applicable(db):
    organization = await _organization(db)
    admin = await _member(db, organization, kyc_status="APPROVED", kyc_bound=True, role=UserRole.ADMIN)
    orgless = await _member(db, organization)
    orgless.organization_id = None

    for user in (admin, orgless):
        eligibility = await evaluate_kyc_execution(db, user)
        assert not eligibility.eligible
        assert eligibility.reason == ExecutionEligibilityReason.NOT_APPLICABLE


def test_kyc_model_has_explicit_organization_provenance():
    assert hasattr(User, "kyc_organization_id")


def test_admin_review_rejects_evidence_from_a_previous_org():
    target = _user(uuid4(), kyc_status="SUBMITTED", kyc_org_id=uuid4())
    with pytest.raises(HTTPException) as exc_info:
        _require_current_kyc_organization(target)
    assert exc_info.value.status_code == 409
    assert exc_info.value.detail["code"] == "KYC_ORGANIZATION_MISMATCH"
