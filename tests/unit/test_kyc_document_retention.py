"""Account cleanup must preserve retained evidence."""

import pytest
from fastapi import HTTPException
from sqlalchemy import select

from app.models.kyc_document import KYCDocument
from app.models.user import Organization, User
from app.routers.monitor import CanaryCleanupRequest, cleanup_signup_canary
from tests.unit.test_kyc_document_routes import state as state
from tests.unit.test_kyc_document_routes import submit


@pytest.mark.parametrize("organization_only", [False, True])
async def test_monitor_refuses_retained_user_or_organization(state, monkeypatch, organization_only):
    db, user, _, _ = state
    await submit(state)
    user_id, org_id = user.id, user.organization_id
    user.email = "probe@canary.example.com"
    org = await db.get(Organization, org_id)
    org.domain = "canary.example.com"
    if organization_only:
        rows = (await db.scalars(select(KYCDocument))).all()
        for row in rows:
            row.user_id = state[2].id
    await db.commit()
    monkeypatch.setattr("app.routers.monitor.get_monitor_canary_domain", lambda email: "canary.example.com")
    with pytest.raises(HTTPException) as error:
        await cleanup_signup_canary(CanaryCleanupRequest(email=user.email), None, db)
    assert error.value.status_code == 409
    assert error.value.detail["code"] == "KYC_DOCUMENTS_RETAINED"
    assert await db.get(User, user_id) is not None
    assert await db.get(Organization, org_id) is not None
    assert len((await db.scalars(select(KYCDocument))).all()) == 2


@pytest.mark.parametrize("model", [User, Organization])
async def test_sqladmin_delete_refuses_retained_evidence(state, model):
    from app.admin import RetentionAwareDelete
    db, user, _, request = state
    await submit(state)
    instance = object.__new__(RetentionAwareDelete)
    instance.model = model
    # Exercise the exact override with the fixture's session factory.
    from sqlalchemy.ext.asyncio import async_sessionmaker
    instance.session_maker = async_sessionmaker(db.bind, expire_on_commit=False)
    pk = user.id if model is User else user.organization_id
    with pytest.raises(HTTPException) as error:
        await instance.delete_model(request, str(pk))
    assert error.value.detail["code"] == "KYC_DOCUMENTS_RETAINED"
    assert await db.get(model, pk) is not None
