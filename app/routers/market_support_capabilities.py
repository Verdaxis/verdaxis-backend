"""Capability grants and organization discovery for market support."""
from app.routers.market_support_common import *  # noqa: F403,F405


@admin_router.get("/capabilities", response_model=list[AdminCapability])
async def list_capabilities(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin_identity)],
):
    return await _active_capabilities(db, current_user.id)


@admin_router.post(
    "/capability-grants",
    response_model=CapabilityGrantResponse,
    status_code=status.HTTP_201_CREATED,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def grant_capability(
    request: Request,
    body: CapabilityGrantCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin_identity)],
):
    await _require_grant_administrator(db, current_user)
    target = (
        await db.execute(select(User).where(User.id == body.user_id).with_for_update())
    ).scalar_one_or_none()
    if target is None or target.role != UserRole.ADMIN or target.status != UserStatus.APPROVED:
        raise HTTPException(status_code=409, detail="Capability target must be an approved administrator")
    if target.id == current_user.id and current_user.id not in bootstrap_authorization_admin_ids():
        raise HTTPException(status_code=403, detail="Administrators cannot grant capabilities to themselves")

    grant = (
        await db.execute(
            select(AdminCapabilityGrant)
            .where(
                AdminCapabilityGrant.user_id == target.id,
                AdminCapabilityGrant.capability == body.capability,
            )
            .with_for_update()
        )
    ).scalar_one_or_none()
    now = datetime.now(UTC)
    if grant is None:
        grant = AdminCapabilityGrant(
            user_id=target.id,
            capability=body.capability,
            granted_by_user_id=current_user.id,
            reason=body.reason,
            granted_at=now,
            expires_at=body.expires_at,
        )
        db.add(grant)
    else:
        grant.granted_by_user_id = current_user.id
        grant.reason = body.reason
        grant.granted_at = now
        grant.expires_at = body.expires_at
        grant.revoked_at = None
        grant.revoked_by_user_id = None
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=MARKET_SUPPORT_CAPABILITY_GRANTED,
        resource_type="admin_capability_grant",
        resource_id=grant.id,
        changes={
            "target_user_id": str(target.id),
            "capability": body.capability.value,
            "expires_at": body.expires_at.isoformat() if body.expires_at else None,
            "reason": body.reason,
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(grant)
    return grant


@admin_router.post(
    "/capability-grants/{grant_id}/revoke",
    response_model=CapabilityGrantResponse,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def revoke_capability(
    grant_id: UUID,
    request: Request,
    body: CapabilityRevokeRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin_identity)],
):
    await _require_grant_administrator(db, current_user)
    grant = (
        await db.execute(
            select(AdminCapabilityGrant)
            .where(AdminCapabilityGrant.id == grant_id)
            .with_for_update()
        )
    ).scalar_one_or_none()
    if grant is None:
        raise HTTPException(status_code=404, detail="Capability grant not found")
    if grant.user_id == current_user.id and current_user.id not in bootstrap_authorization_admin_ids():
        raise HTTPException(status_code=403, detail="Administrators cannot revoke their own capability")
    if grant.revoked_at is None:
        grant.revoked_at = datetime.now(UTC)
        grant.revoked_by_user_id = current_user.id
        await record_audit(
            db,
            user_id=current_user.id,
            action=MARKET_SUPPORT_CAPABILITY_REVOKED,
            resource_type="admin_capability_grant",
            resource_id=grant.id,
            changes={"target_user_id": str(grant.user_id), "reason": body.reason},
            **request_audit_context(request),
        )
        await db.commit()
        await db.refresh(grant)
    return grant


@admin_router.get("/organizations", response_model=list[MarketSupportOrganizationResponse])
async def search_organizations(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin_identity)],
    query: str = Query(min_length=2, max_length=100),
    limit: int = Query(default=20, ge=1, le=50),
):
    capabilities = await _active_capabilities(db, current_user.id)
    if not capabilities:
        raise HTTPException(status_code=403, detail="Forbidden")
    pattern = f"%{query.strip()}%"
    organizations = (
        await db.execute(
            select(Organization)
            .where(
                or_(Organization.name.ilike(pattern), Organization.domain.ilike(pattern)),
                Organization.verification_status == "APPROVED",
                Organization.provenance == OrganizationProvenance.REAL,
            )
            .order_by(Organization.name.asc())
            .limit(limit)
        )
    ).scalars().all()
    return [
        MarketSupportOrganizationResponse(
            id=organization.id,
            name=organization.name,
            domain=organization.domain,
            type=_enum_value(organization.type),
            verification_status=str(organization.verification_status),
            provenance=_enum_value(organization.provenance),
        )
        for organization in organizations
    ]
