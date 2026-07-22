"""Versioned support edits, cancellation, and customer attribution."""
from app.routers.market_support_common import *  # noqa: F403,F405

@admin_router.patch(
    "/organizations/{organization_id}/orders/{order_id}",
    response_model=AssistedOrderResponse,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def update_assisted_order(
    organization_id: UUID,
    order_id: UUID,
    request: Request,
    body: AssistedOrderUpdateRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    idempotency_key_header: Annotated[
        str | None, Header(alias="Idempotency-Key")
    ] = None,
):
    idempotency_key = _require_idempotency_key(idempotency_key_header)
    request_hash = idempotency_request_hash(
        {"order_id": str(order_id), **body.model_dump(mode="json")}
    )
    await acquire_idempotency_lock(
        db,
        tenant_id=organization_id,
        operation=_UPDATE_OPERATION,
        key=idempotency_key,
    )
    replay = await _receipt_replay(
        db,
        organization_id=organization_id,
        operation=SupportActionOperation.UPDATE,
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
    authorization = await _load_authorization(
        db,
        organization_id=organization_id,
        authorization_id=body.support_authorization_id,
        required_scope=SupportAuthorizationScope.EDIT_SUPPORT_MANAGED_ORDER,
        acting_admin_id=current_user.id,
        lock=True,
    )
    preview_order, preview_attribution = await _load_attributed_order(
        db, order_id=order_id, organization_id=organization_id
    )
    _assert_expected_state(
        order=preview_order,
        attribution=preview_attribution,
        accountable_user_id=body.accountable_user_id,
        authorization_id=body.support_authorization_id,
        expected_support_version=body.expected_support_version,
        expected_order_updated_at=body.expected_order_updated_at,
    )
    if preview_order.status not in _ACTIVE_ORDER_STATUSES:
        raise HTTPException(status_code=400, detail="Only open assisted listings can be edited")

    _update_dict, effective_order_data, effective_remaining = _candidate_update_payload(
        preview_order, body.changes
    )
    _validate_order_against_authorization(
        order_data=effective_order_data,
        authorization=authorization,
        now=datetime.now(UTC),
    )
    organization = await _load_real_approved_organization(db, organization_id, lock=False)
    accountable_user = await _load_accountable_supplier(
        db,
        user_id=body.accountable_user_id,
        organization=organization,
        lock=False,
    )
    product, delivery_point = await _load_catalog(db, effective_order_data)
    candidate = _build_candidate(
        order_data=effective_order_data,
        organization=organization,
        accountable_user=accountable_user,
        product=product,
        delivery_point=delivery_point,
    )
    candidate.remaining_quantity_mt = effective_remaining
    assessment = await assess_post_only(
        db,
        candidate,
        accountable_user=accountable_user,
        organization=organization,
        lock=True,
    )
    if assessment.indeterminate or assessment.would_cross:
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "POST_ONLY_WOULD_CROSS" if not assessment.indeterminate else "POST_ONLY_FANOUT_LIMIT",
                "Verdaxis-assisted edits must leave the listing resting without immediate execution",
            ),
        )

    order, attribution = await _load_attributed_order(
        db,
        order_id=order_id,
        organization_id=organization_id,
        lock=True,
    )
    _assert_expected_state(
        order=order,
        attribution=attribution,
        accountable_user_id=body.accountable_user_id,
        authorization_id=body.support_authorization_id,
        expected_support_version=body.expected_support_version,
        expected_order_updated_at=body.expected_order_updated_at,
    )
    projected = await _open_authorized_quantity(
        db, authorization.id, exclude_order_id=order.id
    ) + effective_remaining
    if projected > authorization.max_total_open_quantity_mt:
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "ASSISTED_AGGREGATE_QUANTITY_OUT_OF_SCOPE",
                "Edit would exceed the authorization's aggregate open-quantity limit",
            ),
        )

    attribution.support_version += 1
    attribution.last_action_actor_user_id = current_user.id
    attribution.last_action_reason_code = body.reason_code
    attribution.support_case_reference = body.support_case_reference
    attribution.updated_at = datetime.now(UTC)
    db.add(
        MarketSupportActionReceipt(
            organization_id=organization_id,
            operation=SupportActionOperation.UPDATE,
            idempotency_key=idempotency_key,
            request_hash=request_hash,
            order_id=order.id,
            support_version=attribution.support_version,
            created_by_admin_user_id=current_user.id,
        )
    )
    await notify_org_users_batched(
        db,
        [(
            organization_id,
            NotificationType.ORDER_UPDATE,
            "Listing updated by Verdaxis Support",
            "Verdaxis Support updated an assisted listing for your organization.",
            {"order_id": str(order.id), "submission_method": "VERDAXIS_ASSISTED"},
        )],
    )
    context = MarketSupportActionContext(
        actor_user_id=current_user.id,
        target_organization_id=organization_id,
        accountable_user_id=accountable_user.id,
        support_authorization_id=authorization.id,
        operation=SupportActionOperation.UPDATE.value,
        reason_code=body.reason_code,
        support_case_reference=body.support_case_reference,
        idempotency_key=idempotency_key,
        request_hash=request_hash,
        support_version=attribution.support_version,
    )
    ordinary_update = getattr(update_order, "__wrapped__", update_order)
    with market_support_action_context(context):
        updated = await ordinary_update(
            order_id=order.id,
            request=request,
            update_data=body.changes,
            current_user=accountable_user,
            db=db,
        )
    refreshed_order, refreshed_attribution = await _load_attributed_order(
        db, order_id=updated.id, organization_id=organization_id
    )
    return await _assisted_response(db, refreshed_order, refreshed_attribution)


@admin_router.post(
    "/organizations/{organization_id}/orders/{order_id}/cancel",
    response_model=AssistedOrderResponse,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def cancel_assisted_order(
    organization_id: UUID,
    order_id: UUID,
    request: Request,
    body: AssistedOrderCancelRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    idempotency_key_header: Annotated[
        str | None, Header(alias="Idempotency-Key")
    ] = None,
):
    idempotency_key = _require_idempotency_key(idempotency_key_header)
    request_hash = idempotency_request_hash(
        {"order_id": str(order_id), **body.model_dump(mode="json")}
    )
    await acquire_idempotency_lock(
        db,
        tenant_id=organization_id,
        operation=_CANCEL_OPERATION,
        key=idempotency_key,
    )
    replay = await _receipt_replay(
        db,
        organization_id=organization_id,
        operation=SupportActionOperation.CANCEL,
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
    await _load_authorization(
        db,
        organization_id=organization_id,
        authorization_id=body.support_authorization_id,
        required_scope=SupportAuthorizationScope.CANCEL_SUPPORT_MANAGED_ORDER,
        acting_admin_id=current_user.id,
        lock=True,
        allow_inactive_for_cancellation=True,
    )
    preview_order, preview_attribution = await _load_attributed_order(
        db, order_id=order_id, organization_id=organization_id
    )
    _assert_expected_state(
        order=preview_order,
        attribution=preview_attribution,
        accountable_user_id=body.accountable_user_id,
        authorization_id=body.support_authorization_id,
        expected_support_version=body.expected_support_version,
        expected_order_updated_at=body.expected_order_updated_at,
    )
    if preview_order.status not in _ACTIVE_ORDER_STATUSES:
        raise HTTPException(status_code=400, detail="Only open assisted listings can be cancelled")
    await acquire_market_slice_lock(
        db,
        side=preview_order.side,
        product_id=preview_order.product_id,
        delivery_point_id=preview_order.delivery_point_id,
        availability_window=preview_order.availability_window,
    )
    order, attribution = await _load_attributed_order(
        db,
        order_id=order_id,
        organization_id=organization_id,
        lock=True,
    )
    _assert_expected_state(
        order=order,
        attribution=attribution,
        accountable_user_id=body.accountable_user_id,
        authorization_id=body.support_authorization_id,
        expected_support_version=body.expected_support_version,
        expected_order_updated_at=body.expected_order_updated_at,
    )
    if order.status not in _ACTIVE_ORDER_STATUSES:
        raise HTTPException(status_code=409, detail="Listing changed before cancellation")

    before = await _watchlist_before_state(db, order)
    benchmark_key: LiveBenchmarkKey = (
        order.side,
        order.market_product,
        order.delivery_point_id,
        order.availability_window,
    )
    if order.inventory_item_id is not None and order.remaining_quantity_mt > 0:
        await release_inventory(db, order.inventory_item_id, order.remaining_quantity_mt)
    order.status = OrderBookStatus.CANCELLED
    attribution.support_version += 1
    attribution.last_action_actor_user_id = current_user.id
    attribution.last_action_reason_code = body.reason_code
    attribution.support_case_reference = body.support_case_reference
    attribution.updated_at = datetime.now(UTC)
    db.add(
        MarketSupportActionReceipt(
            organization_id=organization_id,
            operation=SupportActionOperation.CANCEL,
            idempotency_key=idempotency_key,
            request_hash=request_hash,
            order_id=order.id,
            support_version=attribution.support_version,
            created_by_admin_user_id=current_user.id,
        )
    )
    await rebuild_live_slice_benchmarks_for_keys(db, [benchmark_key])
    await emit_order_updated(db, before=before, order=order)
    await notify_org_users_batched(
        db,
        [(
            organization_id,
            NotificationType.ORDER_UPDATE,
            "Listing cancelled by Verdaxis Support",
            "Verdaxis Support cancelled the remaining quantity of an assisted listing.",
            {"order_id": str(order.id), "submission_method": "VERDAXIS_ASSISTED"},
        )],
    )
    context = MarketSupportActionContext(
        actor_user_id=current_user.id,
        target_organization_id=organization_id,
        accountable_user_id=body.accountable_user_id,
        support_authorization_id=body.support_authorization_id,
        operation=SupportActionOperation.CANCEL.value,
        reason_code=body.reason_code,
        support_case_reference=body.support_case_reference,
        idempotency_key=idempotency_key,
        request_hash=request_hash,
        support_version=attribution.support_version,
    )
    with market_support_action_context(context):
        await record_audit(
            db,
            user_id=body.accountable_user_id,
            action=ORDER_CANCELLED,
            resource_type="order",
            resource_id=order.id,
            changes={"status": OrderBookStatus.CANCELLED.value},
            **request_audit_context(request),
        )
    await commit_market_events(
        db,
        [
            participant_market_event(
                event_type="order_cancelled",
                aggregate_type="order",
                aggregate_id=order.id,
                participant_org_ids=(organization_id,),
                payload={
                    **order_activity_provenance(order),
                    "id": str(order.id),
                    "side": order.side.value,
                    "product_name": order.product_name,
                    "fuel_type": order.fuel_type,
                    "region": order.region,
                },
            )
        ],
    )
    await db.refresh(order)
    await db.refresh(attribution)
    order, attribution = await _load_attributed_order(
        db, order_id=order.id, organization_id=organization_id
    )
    return await _assisted_response(db, order, attribution)


@customer_router.get(
    "/my-assisted-orders",
    response_model=list[CustomerAssistedOrderMetadata],
)
async def my_assisted_order_metadata(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    if current_user.organization_id is None:
        return []
    rows = (
        await db.execute(
            select(OrderSupportAttribution)
            .where(OrderSupportAttribution.organization_id == current_user.organization_id)
            .order_by(OrderSupportAttribution.created_at.desc())
            .limit(200)
        )
    ).scalars().all()
    return [
        CustomerAssistedOrderMetadata(
            order_id=row.order_id,
            submission_method=row.submission_method,
            created_at=row.created_at,
            support_version=row.support_version,
            management_authority=row.management_authority,
            customer_adopted_at=row.customer_adopted_at,
            last_action_at=row.updated_at,
        )
        for row in rows
    ]
