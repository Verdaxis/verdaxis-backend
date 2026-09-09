"""Admin-created, recipient-claimed account flow."""

from datetime import UTC, datetime, timedelta
from urllib.parse import parse_qs, urlsplit
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import get_password_hash, hash_token_identifier, verify_password
from app.database import Base, get_db
from app.models.audit import AuditLog
from app.models.product_analytics import UserLoginDay, UserStatusTransition
from app.models.referral import Referral, ReferralStatus
from app.models.refresh_session import RefreshSession
from app.models.registration import OrganizationJoinRequest, PendingRegistration
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from app.routers.auth_simple import get_current_user, router as auth_router
from app.services.audit_actions import ADMIN_USER_INVITED, USER_INVITATION_ACCEPTED


@pytest.fixture
async def invitation_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = [
        Organization.__table__,
        User.__table__,
        Referral.__table__,
        RefreshSession.__table__,
        AuditLog.__table__,
        UserLoginDay.__table__,
        UserStatusTransition.__table__,
        PendingRegistration.__table__,
        OrganizationJoinRequest.__table__,
    ]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


@pytest.fixture
async def invitation_context(invitation_db):
    organization = Organization(
        id=uuid4(),
        name="Goldwind Green Methanol",
        domain="goldwind.example",
        type=OrgType.FUEL_SUPPLIER,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    admin = User(
        id=uuid4(),
        email="belinda@verdaxis.exchange",
        password_hash=get_password_hash("Admin-password-9"),
        first_name="Belinda",
        last_name="Tan",
        role=UserRole.ADMIN,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    invitation_db.add_all([organization, admin])
    await invitation_db.commit()

    app = FastAPI()
    app.include_router(auth_router, prefix="/api")

    async def override_db():
        yield invitation_db

    async def override_admin():
        return admin

    app.dependency_overrides[get_db] = override_db
    app.dependency_overrides[get_current_user] = override_admin
    return app, admin, organization


def _payload(organization_id):
    return {
        "email": "abdullah@customer.example",
        "first_name": "Abdullah",
        "last_name": "Rahman",
        "role": "SUPPLIER",
        "organization_id": str(organization_id),
    }


def _new_organization_payload(**overrides):
    payload = {
        "email": "new.user@northstar.example",
        "first_name": "New",
        "last_name": "User",
        "role": "BUYER",
        "new_organization": {
            "name": "Northstar Shipping",
            "type": "SHIPPING_LINE",
            "country_code": "SG",
            "tax_id": "SG-2026-001",
        },
    }
    payload.update(overrides)
    return payload


def _token(response) -> str:
    fragment = parse_qs(urlsplit(response.json()["acceptance_url"]).fragment)
    return fragment["token"][0]


@pytest.mark.asyncio
async def test_admin_invite_is_preapproved_reissuable_and_attributed(invitation_context, invitation_db):
    app, admin, organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        first = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
        second = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))

    assert first.status_code == 201, first.text
    assert second.status_code == 200, second.text
    assert first.json()["user_id"] == second.json()["user_id"]
    assert first.json()["reissued"] is False
    assert second.json()["reissued"] is True
    assert _token(first) != _token(second)

    user = (
        await invitation_db.execute(select(User).where(User.email == _payload(organization.id)["email"]))
    ).scalar_one()
    assert user.status == UserStatus.APPROVED
    assert user.organization_id == organization.id
    assert user.email_verified is False
    assert user.must_change_password is True
    assert user.referred_by_id == admin.id

    referral = (
        await invitation_db.execute(select(Referral).where(Referral.referred_user_id == user.id))
    ).scalar_one()
    assert referral.referrer_id == admin.id
    assert referral.status == ReferralStatus.SIGNED_UP

    actions = (await invitation_db.execute(select(AuditLog.action))).scalars().all()
    assert actions.count(ADMIN_USER_INVITED) == 2


@pytest.mark.asyncio
async def test_admin_invite_atomically_creates_preapproved_organization(
    invitation_context,
    invitation_db,
):
    app, admin, _existing_organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post(
            "/api/auth/admin/invitations",
            json=_new_organization_payload(),
        )

    assert response.status_code == 201, response.text
    assert response.json()["organization_name"] == "Northstar Shipping"
    assert response.json()["organization_created"] is True

    organization = (
        await invitation_db.execute(
            select(Organization).where(Organization.name == "Northstar Shipping")
        )
    ).scalar_one()
    assert organization.domain == "northstar.example"
    assert organization.type == OrgType.SHIPPING_LINE
    assert organization.country_code == "SG"
    assert organization.tax_id == "SG-2026-001"
    assert organization.verification_status == "APPROVED"
    assert organization.provenance == OrganizationProvenance.REAL

    user = (
        await invitation_db.execute(
            select(User).where(User.email == "new.user@northstar.example")
        )
    ).scalar_one()
    assert user.organization_id == organization.id
    assert user.status == UserStatus.APPROVED
    assert user.referred_by_id == admin.id

    audit = (
        await invitation_db.execute(
            select(AuditLog).where(AuditLog.action == ADMIN_USER_INVITED)
        )
    ).scalar_one()
    assert audit.changes["organization_created"] is True
    assert audit.changes["organization_type"] == "SHIPPING_LINE"
    assert audit.changes["organization_country_code"] == "SG"
    assert "tax" not in str(audit.changes).lower()
    assert audit.changes["organization_provenance"] == "REAL"


@pytest.mark.asyncio
async def test_admin_can_invite_second_user_to_new_organization_before_acceptance(
    invitation_context,
    invitation_db,
):
    app, _admin, _existing_organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        first = await client.post("/api/auth/admin/invitations", json=_new_organization_payload())
        organization = (
            await invitation_db.execute(
                select(Organization).where(Organization.name == "Northstar Shipping")
            )
        ).scalar_one()
        second = await client.post(
            "/api/auth/admin/invitations",
            json={
                "email": "second.user@northstar.example",
                "first_name": "Second",
                "last_name": "User",
                "role": "BUYER",
                "organization_id": str(organization.id),
            },
        )

    assert first.status_code == 201, first.text
    assert second.status_code == 201, second.text
    assert second.json()["organization_name"] == "Northstar Shipping"


@pytest.mark.asyncio
async def test_admin_invitation_organization_list_includes_approved_live_and_pending_market_status(
    invitation_context,
    invitation_db,
):
    app, _admin, real_organization = invitation_context
    pending_market = Organization(
        id=uuid4(),
        name="Pending Market Review",
        domain="pending.example",
        type=OrgType.FUEL_SUPPLIER,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.UNKNOWN,
    )
    unapproved = Organization(
        id=uuid4(),
        name="Unapproved Organization",
        domain="unapproved.example",
        type=OrgType.FUEL_SUPPLIER,
        verification_status="PENDING",
        provenance=OrganizationProvenance.REAL,
    )
    demo = Organization(
        id=uuid4(),
        name="Demo Organization",
        domain="demo.example",
        type=OrgType.FUEL_SUPPLIER,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.DEMO,
    )
    invitation_db.add_all([pending_market, unapproved, demo])
    await invitation_db.commit()

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get("/api/auth/admin/invitations/organizations")

    assert response.status_code == 200, response.text
    items = {item["name"]: item for item in response.json()["items"]}
    assert set(items) == {real_organization.name, pending_market.name}
    assert items[real_organization.name]["provenance"] == "REAL"
    assert items[pending_market.name]["provenance"] == "UNKNOWN"


@pytest.mark.asyncio
async def test_recipient_accepts_invitation_for_new_onboarding_approved_organization(
    invitation_context,
    invitation_db,
):
    app, _admin, _existing_organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        created = await client.post(
            "/api/auth/admin/invitations",
            json=_new_organization_payload(),
        )
        token = _token(created)

        resolved = await client.post("/api/auth/invitations/resolve", json={"token": token})
        accepted = await client.post(
            "/api/auth/invitations/accept",
            json={"token": token, "new_password": "Accepted-password-9", "accept_terms": True},
        )

    assert created.status_code == 201, created.text
    assert resolved.status_code == 200, resolved.text
    assert resolved.json()["organization_name"] == "Northstar Shipping"
    assert accepted.status_code == 200, accepted.text

    organization = (
        await invitation_db.execute(
            select(Organization).where(Organization.name == "Northstar Shipping")
        )
    ).scalar_one()
    assert organization.verification_status == "APPROVED"
    assert organization.provenance == OrganizationProvenance.REAL


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "payload",
    [
        {
            "email": "missing@organization.example",
            "first_name": "Missing",
            "role": "BUYER",
        },
        _new_organization_payload(organization_id=str(uuid4())),
    ],
)
async def test_admin_invite_requires_exactly_one_organization_source(
    invitation_context,
    payload,
):
    app, _admin, _organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post("/api/auth/admin/invitations", json=payload)

    assert response.status_code == 422


@pytest.mark.asyncio
async def test_admin_invite_new_organization_rejects_role_mismatch_without_writes(
    invitation_context,
    invitation_db,
):
    app, _admin, _organization = invitation_context
    payload = _new_organization_payload(
        new_organization={
            "name": "Wrong Side Fuels",
            "type": "FUEL_SUPPLIER",
            "country_code": "SG",
        }
    )
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post("/api/auth/admin/invitations", json=payload)

    assert response.status_code == 422
    assert response.json()["detail"] == "Organization type does not match the selected account role"
    assert (
        await invitation_db.execute(
            select(Organization).where(Organization.name == "Wrong Side Fuels")
        )
    ).scalar_one_or_none() is None


@pytest.mark.asyncio
async def test_admin_invite_new_organization_rejects_owned_domain_without_writes(
    invitation_context,
    invitation_db,
):
    app, _admin, existing_organization = invitation_context
    payload = _new_organization_payload(
        email=f"new.supplier@{existing_organization.domain}",
        role="SUPPLIER",
        new_organization={
            "name": "Duplicate Domain Supplier",
            "type": "FUEL_SUPPLIER",
            "country_code": "SG",
        },
    )
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post("/api/auth/admin/invitations", json=payload)

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "ORGANIZATION_DOMAIN_CONFLICT"
    assert (
        await invitation_db.execute(
            select(Organization).where(Organization.name == "Duplicate Domain Supplier")
        )
    ).scalar_one_or_none() is None
    assert (
        await invitation_db.execute(
            select(User).where(User.email == payload["email"])
        )
    ).scalar_one_or_none() is None


@pytest.mark.asyncio
async def test_admin_invite_existing_email_does_not_leave_orphan_organization(
    invitation_context,
    invitation_db,
):
    app, _admin, existing_organization = invitation_context
    existing = User(
        id=uuid4(),
        email="registered@other.example",
        password_hash=get_password_hash("Registered-password-9"),
        first_name="Registered",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=existing_organization.id,
    )
    invitation_db.add(existing)
    await invitation_db.commit()

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post(
            "/api/auth/admin/invitations",
            json=_new_organization_payload(
                email="registered@other.example",
                new_organization={
                    "name": "Must Roll Back",
                    "type": "SHIPPING_LINE",
                    "country_code": "SG",
                },
            ),
        )

    assert response.status_code == 409
    assert (
        await invitation_db.execute(
            select(Organization).where(Organization.name == "Must Roll Back")
        )
    ).scalar_one_or_none() is None


@pytest.mark.asyncio
async def test_recipient_accepts_once_and_enters_with_a_normal_session(invitation_context, invitation_db):
    app, _admin, organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        created = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
        token = _token(created)

        resolved = await client.post("/api/auth/invitations/resolve", json={"token": token})
        assert resolved.status_code == 200, resolved.text
        assert resolved.json()["organization_name"] == organization.name
        assert resolved.json()["invited_by_name"] == "Belinda Tan"

        refused = await client.post(
            "/api/auth/invitations/accept",
            json={"token": token, "new_password": "Accepted-password-9", "accept_terms": False},
        )
        assert refused.status_code == 422

        accepted = await client.post(
            "/api/auth/invitations/accept",
            json={"token": token, "new_password": "Accepted-password-9", "accept_terms": True},
        )
        replay = await client.post(
            "/api/auth/invitations/accept",
            json={"token": token, "new_password": "Another-password-9", "accept_terms": True},
        )

    assert accepted.status_code == 200, accepted.text
    assert accepted.json()["access_token"]
    assert accepted.cookies.get("refresh_token")
    assert replay.status_code == 400

    user = (
        await invitation_db.execute(select(User).where(User.email == _payload(organization.id)["email"]))
    ).scalar_one()
    assert user.email_verified is True
    assert user.must_change_password is False
    assert user.last_login is not None
    assert verify_password("Accepted-password-9", user.password_hash)
    assert user.password_reset_token_hash is None

    referral = (
        await invitation_db.execute(select(Referral).where(Referral.referred_user_id == user.id))
    ).scalar_one()
    assert referral.status == ReferralStatus.VERIFIED
    assert referral.verified_at is not None
    assert (await invitation_db.execute(select(UserLoginDay))).scalars().one()

    accepted_audit = (
        await invitation_db.execute(select(AuditLog).where(AuditLog.action == USER_INVITATION_ACCEPTED))
    ).scalar_one()
    assert accepted_audit.user_id == user.id
    assert accepted_audit.changes["terms_accepted"] is True
    assert accepted_audit.changes["terms_url"].endswith("/terms")


@pytest.mark.asyncio
async def test_invite_requires_admin_and_eligible_real_organization(invitation_context, invitation_db):
    app, admin, organization = invitation_context
    admin.role = UserRole.BUYER
    await invitation_db.commit()
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        forbidden = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
    assert forbidden.status_code == 403

    admin.role = UserRole.ADMIN
    organization.provenance = OrganizationProvenance.DEMO
    await invitation_db.commit()
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        ineligible = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
    assert ineligible.status_code == 404


@pytest.mark.asyncio
async def test_invite_role_must_match_organization_side(invitation_context):
    app, _admin, organization = invitation_context
    payload = _payload(organization.id)
    payload["role"] = "BUYER"
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post("/api/auth/admin/invitations", json=payload)

    assert response.status_code == 422
    assert response.json()["detail"] == "Organization type does not match the selected account role"


@pytest.mark.asyncio
async def test_previously_issued_mismatched_invitation_is_not_resolvable(invitation_context, invitation_db):
    app, _admin, organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        created = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
        token = _token(created)
        invited_user = (
            await invitation_db.execute(select(User).where(User.email == _payload(organization.id)["email"]))
        ).scalar_one()
        invited_user.role = UserRole.BUYER
        await invitation_db.commit()
        resolved = await client.post("/api/auth/invitations/resolve", json={"token": token})

    assert resolved.status_code == 400


@pytest.mark.asyncio
async def test_invitation_password_policy_is_enforced(invitation_context):
    app, _admin, organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        created = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
        token = _token(created)
        weak = await client.post(
            "/api/auth/invitations/accept",
            json={"token": token, "new_password": "alllowercase", "accept_terms": True},
        )
        still_valid = await client.post("/api/auth/invitations/resolve", json={"token": token})

    assert weak.status_code == 422
    assert still_valid.status_code == 200


@pytest.mark.asyncio
async def test_registration_rejects_opposite_side_before_creating_organization(invitation_db):
    token = "registration-token-with-enough-entropy"
    pending = PendingRegistration(
        token_hash=hash_token_identifier(token),
        email="buyer@new-company.example",
        password_hash=get_password_hash("Buyer-password-9"),
        first_name="Buyer",
        role=UserRole.BUYER,
        expires_at=datetime.now(UTC) + timedelta(hours=1),
    )
    invitation_db.add(pending)
    await invitation_db.commit()

    app = FastAPI()
    app.include_router(auth_router, prefix="/api")

    async def override_db():
        yield invitation_db

    app.dependency_overrides[get_db] = override_db
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post(
            "/api/auth/register-with-org",
            json={
                "registration_token": token,
                "organization": {
                    "name": "Wrong Side Supplier",
                    "type": "FUEL_SUPPLIER",
                    "country_code": "SG",
                },
            },
        )

    created = (
        await invitation_db.execute(select(Organization).where(Organization.name == "Wrong Side Supplier"))
    ).scalar_one_or_none()
    await invitation_db.refresh(pending)
    assert response.status_code == 422
    assert created is None
    assert pending.used_at is None


@pytest.mark.asyncio
async def test_domain_join_rejects_opposite_side_before_creating_user(invitation_context, invitation_db):
    app, _admin, organization = invitation_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post(
            "/api/auth/register",
            json={
                "email": f"buyer@{organization.domain}",
                "password": "Buyer-password-9",
                "first_name": "Buyer",
                "role": "BUYER",
            },
        )

    created = (
        await invitation_db.execute(select(User).where(User.email == f"buyer@{organization.domain}"))
    ).scalar_one_or_none()
    assert response.status_code == 422
    assert created is None


@pytest.mark.asyncio
async def test_unclaimed_invitation_cannot_fall_into_ordinary_password_reset(invitation_context, invitation_db, monkeypatch):
    app, _admin, organization = invitation_context
    send_reset = AsyncMock()
    monkeypatch.setattr("app.routers.auth_simple.send_password_reset_email", send_reset)

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        created = await client.post("/api/auth/admin/invitations", json=_payload(organization.id))
        invited_token = _token(created)
        before_hash = (
            await invitation_db.execute(select(User.password_reset_token_hash).where(User.email == _payload(organization.id)["email"]))
        ).scalar_one()
        forgot = await client.post(
            "/api/auth/forgot-password",
            json={"email": _payload(organization.id)["email"]},
        )

    assert forgot.status_code == 200
    after_hash = (
        await invitation_db.execute(select(User.password_reset_token_hash).where(User.email == _payload(organization.id)["email"]))
    ).scalar_one()
    assert after_hash == before_hash
    assert invited_token
    send_reset.assert_not_awaited()


@pytest.mark.asyncio
@pytest.mark.parametrize("existing_org", [True, False])
@pytest.mark.parametrize("canary", [True, False])
async def test_signup_alert_after_committed_application(
    invitation_context, invitation_db, monkeypatch, existing_org, canary,
):
    from app.config import settings

    app, _admin, organization = invitation_context
    provider = AsyncMock(return_value=False)
    monkeypatch.setattr("app.services.email._send_email", provider)
    monkeypatch.setattr(settings, "ENVIRONMENT", "staging")
    monkeypatch.setattr(settings, "FRONTEND_URL", "https://staging.verdaxis.exchange")
    monkeypatch.setattr(settings, "MONITOR_TOKEN", "monitor-test-secret")
    domain = "signup.canary.verdaxis.exchange" if canary else "signup.example.com"
    organization.domain = domain if existing_org else "other.example.com"
    organization.name = "Fuel & Shipping"
    await invitation_db.commit()
    applicant_email = f"canary+signup@{domain}" if canary else f"applicant@{domain}"
    headers = {"X-Monitor-Token": "monitor-test-secret"} if canary else {}
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.post(
            "/api/auth/register",
            headers=headers,
            json={
                "email": applicant_email,
                "password": "Supplier-password-9",
                "first_name": "<New>",
                "last_name": "Applicant",
                "role": "SUPPLIER",
            },
        )
        assert response.status_code == 200, response.text
        if not existing_org:
            assert response.json()["status"] == "requires_org"
            provider.assert_not_awaited()
            response = await client.post(
                "/api/auth/register-with-org",
                headers=headers,
                json={
                    "registration_token": response.json()["registration_token"],
                    "organization": {
                        "name": "Fuel & Shipping",
                        "type": "FUEL_SUPPLIER",
                        "country_code": "SG",
                    },
                },
            )
            assert response.status_code == 200, response.text

    user = (await invitation_db.execute(
        select(User).where(User.email == applicant_email)
    )).scalar_one()
    assert user.status == UserStatus.PENDING
    if canary:
        provider.assert_not_awaited()
    else:
        # Provider failure must not remove the submitted application or prevent verification.
        assert provider.await_count == 2
        alert = provider.await_args_list[0].kwargs
        assert alert["to_email"] == "admin@verdaxis.exchange"
        assert alert["subject"] == "[staging] New Verdaxis account application"
        assert "&lt;New&gt; Applicant" in alert["html"]
        assert "Fuel &amp; Shipping" in alert["html"]
        assert applicant_email in alert["html"]
        assert "SUPPLIER" in alert["html"]
        assert "https://staging.verdaxis.exchange/app/admin/users" in alert["html"]
        assert alert["idempotency_key"] == f"signup-alert/staging/{user.id}"
        assert provider.await_args_list[1].args[0] == applicant_email
