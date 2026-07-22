"""Market-support context, preview, and post-only publication."""
from app.routers.market_support_common import *  # noqa: F403,F405

@admin_router.get(
    "/organizations/{organization_id}/context",
    response_model=MarketSupportContextResponse,
)
async def get_support_context(
    organization_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_active_capability(
        db,
        user_id=current_user.id,
        capability=AdminCapability.MARKET_SUPPORT_LISTINGS,
    )
    organization = await _load_real_approved_organization(db, organization_id, lock=False)
    capabilities = await _active_capabilities(db, current_user.id)

    principal_rows = (
        await db.execute(
            select(User)
            .where(
                User.organization_id == organization.id,
                User.role == UserRole.SUPPLIER,
            )
            .order_by(User.email.asc())
        )
    ).scalars().all()
    principals = [
        MarketSupportPrincipalResponse(
            id=user.id,
            email=str(user.email),
            first_name=user.first_name,
            last_name=user.last_name,
            role=_enum_value(user.role),
            eligible=await execution_party_is_eligible(
                db, user=user, organization=organization
            ),
        )
        for user in principal_rows
    ]

    now = datetime.now(UTC)
    authorizations = (
        await db.execute(
            select(OrganizationSupportAuthorization)
            .where(
                OrganizationSupportAuthorization.organization_id == organization.id,
                OrganizationSupportAuthorization.status == SupportAuthorizationStatus.ACTIVE,
                OrganizationSupportAuthorization.valid_from <= now,
                OrganizationSupportAuthorization.valid_until > now,
            )
            .order_by(OrganizationSupportAuthorization.valid_until.asc())
        )
    ).scalars().all()
    authorization_responses = [
        SupportAuthorizationResponse.model_validate(
            authorization, from_attributes=True
        ).model_copy(
            update={
                "usable_by_current_admin": (
                    authorization.created_by_admin_user_id != current_user.id
                    and (
                        authorization.max_uses is None
                        or authorization.uses_count < authorization.max_uses
                    )
                )
            }
        )
        for authorization in authorizations
    ]

    orders = (
        await db.execute(
            select(OrderBookOrder)
            .options(
                selectinload(OrderBookOrder.organization),
                selectinload(OrderBookOrder.product),
                selectinload(OrderBookOrder.delivery_point),
            )
            .where(OrderBookOrder.organization_id == organization.id)
            .order_by(OrderBookOrder.created_at.desc())
            .limit(100)
        )
    ).scalars().all()
    order_ids = [order.id for order in orders]
    attributions = {
        attribution.order_id: attribution
        for attribution in (
            await db.execute(
                select(OrderSupportAttribution).where(
                    OrderSupportAttribution.order_id.in_(order_ids)
                )
            )
        ).scalars().all()
    } if order_ids else {}
    order_views: list[MarketSupportOrderView] = []
    for order in orders:
        attribution = attributions.get(order.id)
        order_views.append(
            MarketSupportOrderView(
                order=await _order_response(db, order),
                attribution=(
                    AssistedOrderAttributionResponse.model_validate(
                        attribution, from_attributes=True
                    )
                    if attribution is not None
                    else None
                ),
                order_updated_at=order.updated_at,
                support_manageable=bool(
                    attribution is not None
                    and attribution.management_authority == SupportManagementAuthority.SUPPORT_MANDATE
                    and order.status in _ACTIVE_ORDER_STATUSES
                ),
            )
        )

    return MarketSupportContextResponse(
        organization=MarketSupportOrganizationResponse(
            id=organization.id,
            name=organization.name,
            domain=organization.domain,
            type=_enum_value(organization.type),
            verification_status=str(organization.verification_status),
            provenance=_enum_value(organization.provenance),
        ),
        capabilities=capabilities,
        eligible_principals=principals,
        active_authorizations=authorization_responses,
        orders=order_views,
    )


@admin_router.post(
    "/organizations/{organization_id}/orders/preview",
    response_model=PostOnlyPreviewResponse,
)
@limiter.limit("30/minute", key_func=_token_rate_key)
async def preview_assisted_order(
    organization_id: UUID,
    request: Request,
    body: AssistedOrderPreviewRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_active_capability(
        db,
        user_id=current_user.id,
        capability=AdminCapability.MARKET_SUPPORT_LISTINGS,
    )
    (
        _organization,
        _accountable_user,
        authorization,
        _product,
        _delivery_point,
        _candidate,
        assessment,
        similar_count,
    ) = await _prepare_create_or_preview(
        db,
        organization_id=organization_id,
        body=body,
        current_user=current_user,
        lock=False,
    )
    open_quantity = await _open_authorized_quantity(db, authorization.id)
    projected = open_quantity + body.order.quantity_mt
    warnings: list[str] = []
    if assessment.would_cross:
        warnings.append(
            "This price would execute immediately and cannot be published by Verdaxis Support."
        )
    if assessment.indeterminate:
        warnings.append("The crossing set exceeds the bounded transaction limit.")
    if similar_count:
        warnings.append(f"{similar_count} similar open listing(s) already exist for this organization.")
    if projected > authorization.max_total_open_quantity_mt:
        warnings.append("Publishing would exceed the authorization's aggregate open-quantity limit.")
    return PostOnlyPreviewResponse(
        valid=(
            not assessment.would_cross
            and not assessment.indeterminate
            and projected <= authorization.max_total_open_quantity_mt
        ),
        would_cross=assessment.would_cross,
        indeterminate=assessment.indeterminate,
        best_executable_opposing_price_per_mt_usd=(
            assessment.best_executable_opposing_price_per_mt_usd
        ),
        similar_open_order_count=similar_count,
        warnings=warnings,
    )


@admin_router.post(
    "/organizations/{organization_id}/orders",
    response_model=AssistedOrderResponse,
    status_code=status.HTTP_201_CREATED,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def create_assisted_order(
    organization_id: UUID,
    request: Request,
    body: AssistedOrderCreateRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    idempotency_key_header: Annotated[
        str | None, Header(alias="Idempotency-Key")
    ] = None,
):
    idempotency_key = _require_idempotency_key(idempotency_key_header)
    request_hash = idempotency_request_hash(body.model_dump(mode="json"))
    await acquire_idempotency_lock(
        db,
        tenant_id=organization_id,
        operation=_CREATE_OPERATION,
        key=idempotency_key,
    )
    replay = await _receipt_replay(
        db,
        organization_id=organization_id,
        operation=SupportActionOperation.CREATE,
        idempotency_key=idempotency_key,
        request_hash=request_hash,
    )
    if replay is not None:
        order, attribution = await _load_attributed_order(
            db, order_id=replay.order_id, organization_id=organization_id
        )
        return await _assisted_response(db, order, attribution)

    await _lock_active_capability(
        db,
        user_id=current_user.id,
        capability=AdminCapability.MARKET_SUPPORT_LISTINGS,
    )
    (
        _organization,
        accountable_user,
        authorization,
        _product,
        _delivery_point,
        _candidate,
        assessment,
        _similar_count,
    ) = await _prepare_create_or_preview(
        db,
        organization_id=organization_id,
        body=body,
        current_user=current_user,
        lock=True,
    )
    if assessment.indeterminate:
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "POST_ONLY_FANOUT_LIMIT",
                "Market slice crossing fan-out exceeds the bounded transaction limit",
            ),
        )
    if assessment.would_cross:
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "POST_ONLY_WOULD_CROSS",
                "Verdaxis-assisted publication must rest without immediate execution",
                best_executable_opposing_price_per_mt_usd=(
                    str(assessment.best_executable_opposing_price_per_mt_usd)
                    if assessment.best_executable_opposing_price_per_mt_usd is not None
                    else None
                ),
            ),
        )
    projected = await _open_authorized_quantity(db, authorization.id) + body.order.quantity_mt
    if projected > authorization.max_total_open_quantity_mt:
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "ASSISTED_AGGREGATE_QUANTITY_OUT_OF_SCOPE",
                "Publishing would exceed the authorization's aggregate open-quantity limit",
            ),
        )
    if authorization.max_uses is not None and authorization.uses_count >= authorization.max_uses:
        raise HTTPException(
            status_code=409,
            detail=_detail("SUPPORT_AUTHORIZATION_EXHAUSTED", "Support authorization has no uses remaining"),
        )
    authorization.uses_count += 1

    context = MarketSupportActionContext(
        actor_user_id=current_user.id,
        target_organization_id=organization_id,
        accountable_user_id=accountable_user.id,
        support_authorization_id=authorization.id,
        operation=SupportActionOperation.CREATE.value,
        reason_code=body.reason_code,
        support_case_reference=body.support_case_reference,
        idempotency_key=idempotency_key,
        request_hash=request_hash,
        support_version=1,
    )
    ordinary_request = _request_without_idempotency(request)
    ordinary_create = getattr(create_order, "__wrapped__", create_order)
    with market_support_action_context(context):
        created = await ordinary_create(
            request=ordinary_request,
            order_data=body.order,
            current_user=accountable_user,
            db=db,
        )
    order, attribution = await _load_attributed_order(
        db,
        order_id=created.id,
        organization_id=organization_id,
    )
    return await _assisted_response(db, order, attribution)
