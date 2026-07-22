"""Idempotency, concurrency, response, and preview workflow helpers."""
from app.routers.market_support_authorization import *  # noqa: F403,F405


async def _open_authorized_quantity(
    db: AsyncSession,
    authorization_id: UUID,
    *,
    exclude_order_id: UUID | None = None,
) -> Decimal:
    now = datetime.now(UTC)
    filters: list[object] = [
        OrderSupportAttribution.support_authorization_id == authorization_id,
        OrderBookOrder.status.in_(_ACTIVE_ORDER_STATUSES),
        or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > now),
    ]
    if exclude_order_id is not None:
        filters.append(OrderBookOrder.id != exclude_order_id)
    value = (
        await db.execute(
            select(func.coalesce(func.sum(OrderBookOrder.remaining_quantity_mt), 0))
            .join(OrderSupportAttribution, OrderSupportAttribution.order_id == OrderBookOrder.id)
            .where(*filters)
        )
    ).scalar_one()
    return Decimal(value)


async def _similar_open_order_count(
    db: AsyncSession,
    *,
    organization_id: UUID,
    order_data: OrderCreate,
    exclude_order_id: UUID | None = None,
) -> int:
    filters: list[object] = [
        OrderBookOrder.organization_id == organization_id,
        OrderBookOrder.side == OrderSide.ASK,
        OrderBookOrder.product_id == order_data.product_id,
        OrderBookOrder.delivery_point_id == order_data.delivery_point_id,
        OrderBookOrder.availability_window == order_data.availability_window,
        OrderBookOrder.status.in_(_ACTIVE_ORDER_STATUSES),
    ]
    if exclude_order_id is not None:
        filters.append(OrderBookOrder.id != exclude_order_id)
    return int((await db.execute(select(func.count()).select_from(OrderBookOrder).where(*filters))).scalar_one())


async def _receipt_replay(
    db: AsyncSession,
    *,
    organization_id: UUID,
    operation: SupportActionOperation,
    idempotency_key: str,
    request_hash: str,
) -> MarketSupportActionReceipt | None:
    receipt = (
        await db.execute(
            select(MarketSupportActionReceipt)
            .where(
                MarketSupportActionReceipt.organization_id == organization_id,
                MarketSupportActionReceipt.operation == operation,
                MarketSupportActionReceipt.idempotency_key == idempotency_key,
            )
            .with_for_update()
        )
    ).scalar_one_or_none()
    if receipt is not None and receipt.request_hash != request_hash:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "IDEMPOTENCY_KEY_REUSED",
                "Idempotency-Key was reused with a different request",
            ),
        )
    return receipt


async def _load_attributed_order(
    db: AsyncSession,
    *,
    order_id: UUID,
    organization_id: UUID | None = None,
    lock: bool = False,
) -> tuple[OrderBookOrder, OrderSupportAttribution]:
    order_statement = (
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(OrderBookOrder.id == order_id)
    )
    if organization_id is not None:
        order_statement = order_statement.where(OrderBookOrder.organization_id == organization_id)
    if lock:
        order_statement = order_statement.with_for_update().execution_options(populate_existing=True)
    order = (await db.execute(order_statement)).scalar_one_or_none()
    attribution_statement = select(OrderSupportAttribution).where(
        OrderSupportAttribution.order_id == order_id
    )
    if lock:
        attribution_statement = attribution_statement.with_for_update().execution_options(
            populate_existing=True
        )
    attribution = (await db.execute(attribution_statement)).scalar_one_or_none()
    if order is None or attribution is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Assisted order not found")
    if attribution.organization_id != order.organization_id:
        raise HTTPException(status_code=409, detail="Assisted order attribution is inconsistent")
    return order, attribution


async def _assisted_response(
    db: AsyncSession,
    order: OrderBookOrder,
    attribution: OrderSupportAttribution,
) -> AssistedOrderResponse:
    return AssistedOrderResponse(
        order=await _order_response(db, order),
        attribution=AssistedOrderAttributionResponse.model_validate(
            attribution, from_attributes=True
        ),
        order_updated_at=order.updated_at,
    )


def _assert_expected_state(
    *,
    order: OrderBookOrder,
    attribution: OrderSupportAttribution,
    accountable_user_id: UUID,
    authorization_id: UUID,
    expected_support_version: int,
    expected_order_updated_at: datetime,
) -> None:
    if attribution.management_authority != SupportManagementAuthority.SUPPORT_MANDATE:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "CUSTOMER_MANAGES_LISTING",
                "The customer has taken direct control of this listing",
            ),
        )
    if attribution.accountable_user_id != accountable_user_id:
        raise HTTPException(status_code=409, detail=_detail("ACCOUNTABLE_USER_CHANGED", "Accountable user changed"))
    if attribution.support_authorization_id != authorization_id:
        raise HTTPException(status_code=409, detail=_detail("AUTHORIZATION_CHANGED", "Support authorization changed"))
    if attribution.support_version != expected_support_version or not _same_instant(
        order.updated_at, expected_order_updated_at
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ORDER_VERSION_CONFLICT",
                "Listing changed after it was loaded; refresh and review the current terms",
                current_support_version=attribution.support_version,
                current_order_updated_at=_as_utc(order.updated_at).isoformat(),
            ),
        )


def _candidate_update_payload(
    order: OrderBookOrder,
    changes: OrderUpdate,
) -> tuple[dict[str, object], OrderCreate, Decimal]:
    update_dict = changes.model_dump(exclude_unset=True)
    requested_window = update_dict.get("availability_window")
    if requested_window is not None and requested_window != order.availability_window:
        raise HTTPException(
            status_code=409,
            detail="Executable order market identity is immutable; cancel and create a new order",
        )
    update_dict.pop("availability_window", None)
    if "certification_scheme" in update_dict:
        update_dict["certification_scheme"] = normalize_certification_scheme(
            update_dict["certification_scheme"]
        )
    if "expires_at" in update_dict and update_dict["expires_at"] is None:
        raise HTTPException(
            status_code=422,
            detail=_detail(
                "ASSISTED_ORDER_EXPIRY_REQUIRED",
                "Delegated listings cannot clear their expiry",
            ),
        )

    quantity = Decimal(update_dict.get("quantity_mt", order.quantity_mt))
    filled = Decimal(order.quantity_mt) - Decimal(order.remaining_quantity_mt)
    remaining = quantity - filled
    if remaining < 0:
        raise HTTPException(status_code=400, detail="New quantity cannot be less than already filled amount")

    payload = OrderCreate(
        side=OrderSide.ASK.value,
        product_id=order.product_id,
        delivery_point_id=order.delivery_point_id,
        port_id=order.port_id,
        vessel_id=order.vessel_id,
        quantity_mt=quantity,
        price_per_mt_usd=update_dict.get("price_per_mt_usd", order.price_per_mt_usd),
        availability_window=order.availability_window,
        expires_at=update_dict.get("expires_at", order.expires_at),
        certifications=update_dict.get("certifications", order.certifications or []),
        certification_declared=update_dict.get(
            "certification_declared", order.certification_declared
        ),
        certification_scheme=update_dict.get(
            "certification_scheme", order.certification_scheme
        ),
        specification_standard=update_dict.get(
            "specification_standard", order.specification_standard
        ),
        msds_available=update_dict.get("msds_available", order.msds_available),
        carbon_intensity_gco2_mj=update_dict.get(
            "carbon_intensity_gco2_mj", order.carbon_intensity_gco2_mj
        ),
        carbon_intensity_method=update_dict.get(
            "carbon_intensity_method", order.carbon_intensity_method
        ),
        feedstock=update_dict.get("feedstock", order.feedstock),
        origin=update_dict.get("origin", order.origin),
        off_spec=update_dict.get("off_spec", order.off_spec),
        off_spec_notes=update_dict.get("off_spec_notes", order.off_spec_notes),
        is_anonymous=True,
    )
    material = {
        key: value
        for key, value in update_dict.items()
        if getattr(order, key, object()) != value
    }
    if not material:
        raise HTTPException(status_code=400, detail="Update does not change the listing")
    return update_dict, payload, remaining


async def _prepare_create_or_preview(
    db: AsyncSession,
    *,
    organization_id: UUID,
    body: AssistedOrderPreviewRequest | AssistedOrderCreateRequest,
    current_user: User,
    lock: bool,
) -> tuple[
    Organization,
    User,
    OrganizationSupportAuthorization,
    Product,
    DeliveryPoint | None,
    OrderBookOrder,
    PostOnlyAssessment,
    int,
]:
    authorization = await _load_authorization(
        db,
        organization_id=organization_id,
        authorization_id=body.support_authorization_id,
        required_scope=SupportAuthorizationScope.PUBLISH_POST_ONLY_EXECUTABLE_ORDER,
        acting_admin_id=current_user.id,
        lock=lock,
    )
    organization = await _load_real_approved_organization(db, organization_id, lock=False)
    accountable_user = await _load_accountable_supplier(
        db,
        user_id=body.accountable_user_id,
        organization=organization,
        lock=False,
    )
    _validate_order_against_authorization(
        order_data=body.order,
        authorization=authorization,
        now=datetime.now(UTC),
    )
    product, delivery_point = await _load_catalog(db, body.order)
    candidate = _build_candidate(
        order_data=body.order,
        organization=organization,
        accountable_user=accountable_user,
        product=product,
        delivery_point=delivery_point,
    )
    assessment = await assess_post_only(
        db,
        candidate,
        accountable_user=accountable_user,
        organization=organization,
        lock=lock,
    )
    similar_count = await _similar_open_order_count(
        db,
        organization_id=organization_id,
        order_data=body.order,
    )
    return (
        organization,
        accountable_user,
        authorization,
        product,
        delivery_point,
        candidate,
        assessment,
        similar_count,
    )


__all__ = [name for name in globals() if not name.startswith("__")]
