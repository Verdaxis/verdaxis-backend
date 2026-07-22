"""Customer support-mandate administration."""
from app.routers.market_support_common import *  # noqa: F403,F405


@admin_router.post(
    "/organizations/{organization_id}/authorizations",
    response_model=SupportAuthorizationResponse,
    status_code=status.HTTP_201_CREATED,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def create_authorization(
    organization_id: UUID,
    request: Request,
    body: SupportAuthorizationCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin_identity)],
):
    await _lock_active_capability(
        db,
        user_id=current_user.id,
        capability=AdminCapability.MARKET_SUPPORT_AUTHORIZATIONS,
    )
    await _load_real_approved_organization(db, organization_id, lock=True)
    if body.product_id is not None:
        product = (
            await db.execute(
                select(Product).where(
                    Product.id == body.product_id,
                    Product.is_active.is_(True),
                    canonical_market_product_expression(Product).is_not(None),
                )
            )
        ).scalar_one_or_none()
        if product is None:
            raise HTTPException(status_code=400, detail="Invalid product_id")
    if body.delivery_point_id is not None:
        delivery_point = (
            await db.execute(
                select(DeliveryPoint).where(
                    DeliveryPoint.id == body.delivery_point_id,
                    canonical_delivery_point_clause(DeliveryPoint),
                )
            )
        ).scalar_one_or_none()
        if delivery_point is None:
            raise HTTPException(status_code=400, detail="Invalid delivery_point_id")
    if body.max_order_ttl_hours > delegated_listing_max_ttl_hours():
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "AUTHORIZATION_TTL_EXCEEDS_PILOT_LIMIT",
                "Authorization order lifetime exceeds the configured pilot limit",
            ),
        )
    authorization = OrganizationSupportAuthorization(
        organization_id=organization_id,
        authorized_contact_name=body.authorized_contact_name,
        authorized_contact_email=body.authorized_contact_email,
        evidence_reference=body.evidence_reference,
        support_case_reference=body.support_case_reference,
        scopes=[scope.value for scope in body.scopes],
        valid_from=body.valid_from,
        valid_until=body.valid_until,
        status=SupportAuthorizationStatus.ACTIVE,
        allowed_side=OrderSide.ASK.value,
        product_id=body.product_id,
        delivery_point_id=body.delivery_point_id,
        min_price_per_mt_usd=body.min_price_per_mt_usd,
        max_price_per_mt_usd=body.max_price_per_mt_usd,
        max_quantity_mt_per_order=body.max_quantity_mt_per_order,
        max_total_open_quantity_mt=body.max_total_open_quantity_mt,
        max_order_ttl_hours=body.max_order_ttl_hours,
        max_uses=body.max_uses,
        created_by_admin_user_id=current_user.id,
    )
    db.add(authorization)
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=AUTHORIZATION_CREATED,
        resource_type="organization_support_authorization",
        resource_id=authorization.id,
        changes={
            "target_organization_id": str(organization_id),
            "authorized_contact_email": body.authorized_contact_email,
            "evidence_reference": body.evidence_reference,
            "scopes": [scope.value for scope in body.scopes],
            "valid_from": body.valid_from.isoformat(),
            "valid_until": body.valid_until.isoformat(),
            "product_id": str(body.product_id) if body.product_id else None,
            "delivery_point_id": str(body.delivery_point_id) if body.delivery_point_id else None,
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(authorization)
    return SupportAuthorizationResponse.model_validate(
        authorization, from_attributes=True
    ).model_copy(update={"usable_by_current_admin": False})


@admin_router.post(
    "/organizations/{organization_id}/authorizations/{authorization_id}/revoke",
    response_model=SupportAuthorizationResponse,
)
@limiter.limit("10/minute", key_func=_token_rate_key)
@retry_market_transaction()
async def revoke_authorization(
    organization_id: UUID,
    authorization_id: UUID,
    request: Request,
    body: SupportAuthorizationRevokeRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin_identity)],
):
    await _lock_active_capability(
        db,
        user_id=current_user.id,
        capability=AdminCapability.MARKET_SUPPORT_AUTHORIZATIONS,
    )
    authorization = (
        await db.execute(
            select(OrganizationSupportAuthorization)
            .where(
                OrganizationSupportAuthorization.id == authorization_id,
                OrganizationSupportAuthorization.organization_id == organization_id,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    if authorization is None:
        raise HTTPException(status_code=404, detail="Support authorization not found")
    if authorization.status == SupportAuthorizationStatus.REVOKED:
        return SupportAuthorizationResponse.model_validate(
            authorization, from_attributes=True
        ).model_copy(update={"usable_by_current_admin": False})

    preview_rows = (
        await db.execute(
            select(OrderBookOrder)
            .join(OrderSupportAttribution, OrderSupportAttribution.order_id == OrderBookOrder.id)
            .where(
                OrderSupportAttribution.support_authorization_id == authorization.id,
                OrderSupportAttribution.management_authority == SupportManagementAuthority.SUPPORT_MANDATE,
                OrderBookOrder.status.in_(_ACTIVE_ORDER_STATUSES),
            )
        )
    ).scalars().all()
    await acquire_market_slice_locks(
        db,
        [
            (order.side, order.product_id, order.delivery_point_id, order.availability_window)
            for order in preview_rows
        ],
    )
    rows = (
        await db.execute(
            select(OrderBookOrder, OrderSupportAttribution)
            .join(OrderSupportAttribution, OrderSupportAttribution.order_id == OrderBookOrder.id)
            .options(
                selectinload(OrderBookOrder.organization),
                selectinload(OrderBookOrder.product),
                selectinload(OrderBookOrder.delivery_point),
            )
            .where(
                OrderSupportAttribution.support_authorization_id == authorization.id,
                OrderSupportAttribution.management_authority == SupportManagementAuthority.SUPPORT_MANDATE,
                OrderBookOrder.status.in_(_ACTIVE_ORDER_STATUSES),
            )
            .order_by(OrderBookOrder.id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).all()

    authorization.status = SupportAuthorizationStatus.REVOKED
    authorization.revoked_at = datetime.now(UTC)
    authorization.revoked_by_admin_user_id = current_user.id
    authorization.revocation_reason = body.reason
    events = []
    benchmark_keys: list[LiveBenchmarkKey] = []
    for order, attribution in rows:
        before = await _watchlist_before_state(db, order)
        if order.inventory_item_id is not None and order.remaining_quantity_mt > 0:
            await release_inventory(db, order.inventory_item_id, order.remaining_quantity_mt)
        order.status = OrderBookStatus.CANCELLED
        attribution.support_version += 1
        attribution.last_action_actor_user_id = current_user.id
        attribution.last_action_reason_code = "AUTHORIZATION_REVOKED"
        attribution.updated_at = datetime.now(UTC)
        benchmark_keys.append(
            (order.side, order.market_product, order.delivery_point_id, order.availability_window)
        )
        await emit_order_updated(db, before=before, order=order)
        await record_audit(
            db,
            user_id=current_user.id,
            action=ORDER_CANCELLED,
            resource_type="order",
            resource_id=order.id,
            changes={
                "status": OrderBookStatus.CANCELLED.value,
                "market_support": {
                    "target_organization_id": str(organization_id),
                    "accountable_user_id": str(attribution.accountable_user_id),
                    "support_authorization_id": str(authorization.id),
                    "reason_code": "AUTHORIZATION_REVOKED",
                    "support_version": attribution.support_version,
                },
            },
            **request_audit_context(request),
        )
        events.append(
            participant_market_event(
                event_type="order_cancelled",
                aggregate_type="order",
                aggregate_id=order.id,
                participant_org_ids=(order.organization_id,),
                payload={
                    **order_activity_provenance(order),
                    "id": str(order.id),
                    "side": order.side.value,
                    "product_name": order.product_name,
                    "fuel_type": order.fuel_type,
                    "region": order.region,
                },
            )
        )
    await rebuild_live_slice_benchmarks_for_keys(db, benchmark_keys)
    await notify_org_users_batched(
        db,
        [(
            organization_id,
            NotificationType.ORDER_UPDATE,
            "Verdaxis support authorization revoked",
            f"The support mandate was revoked and {len(rows)} remaining assisted listing(s) were cancelled.",
            {"support_authorization_id": str(authorization.id)},
        )],
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=AUTHORIZATION_REVOKED,
        resource_type="organization_support_authorization",
        resource_id=authorization.id,
        changes={
            "target_organization_id": str(organization_id),
            "reason": body.reason,
            "cancelled_order_count": len(rows),
        },
        **request_audit_context(request),
    )
    await commit_market_events(db, events)
    await db.refresh(authorization)
    return SupportAuthorizationResponse.model_validate(
        authorization, from_attributes=True
    ).model_copy(update={"usable_by_current_admin": False})
