"""Organization, principal, mandate, and listing-term validation."""
from app.routers.market_support_base import *  # noqa: F403,F405


async def _load_real_approved_organization(
    db: AsyncSession,
    organization_id: UUID,
    *,
    lock: bool,
) -> Organization:
    statement = select(Organization).where(Organization.id == organization_id)
    if lock:
        statement = statement.with_for_update().execution_options(populate_existing=True)
    organization = (await db.execute(statement)).scalar_one_or_none()
    if organization is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Organization not found")
    if str(organization.verification_status or "").upper() != "APPROVED":
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ORGANIZATION_NOT_EXECUTION_ELIGIBLE",
                "Organization is not approved for market activity",
            ),
        )
    if _enum_value(organization.provenance) != OrganizationProvenance.REAL.value:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ORGANIZATION_PROVENANCE_NOT_REAL",
                "Delegated listings are limited to REAL organizations",
            ),
        )
    return organization


async def _load_accountable_supplier(
    db: AsyncSession,
    *,
    user_id: UUID,
    organization: Organization,
    lock: bool,
    require_execution_eligible: bool = True,
) -> User:
    statement = select(User).where(User.id == user_id)
    if lock:
        statement = statement.with_for_update().execution_options(populate_existing=True)
    user = (await db.execute(statement)).scalar_one_or_none()
    if user is None or user.organization_id != organization.id:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ACCOUNTABLE_USER_ORGANIZATION_MISMATCH",
                "Accountable user does not belong to the target organization",
            ),
        )
    if user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ACCOUNTABLE_USER_ROLE_INVALID",
                "ASK listings require an eligible supplier user",
            ),
        )
    if require_execution_eligible and not await execution_party_is_eligible(
        db, user=user, organization=organization
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ACCOUNTABLE_USER_NOT_EXECUTION_ELIGIBLE",
                "Accountable user is not eligible for market execution",
            ),
        )
    return user


def _authorization_scopes(
    authorization: OrganizationSupportAuthorization,
) -> frozenset[str]:
    return frozenset(str(value) for value in (authorization.scopes or []))


async def _load_authorization(
    db: AsyncSession,
    *,
    organization_id: UUID,
    authorization_id: UUID,
    required_scope: SupportAuthorizationScope,
    acting_admin_id: UUID,
    lock: bool,
    allow_inactive_for_cancellation: bool = False,
) -> OrganizationSupportAuthorization:
    statement = select(OrganizationSupportAuthorization).where(
        OrganizationSupportAuthorization.id == authorization_id,
        OrganizationSupportAuthorization.organization_id == organization_id,
    )
    if lock:
        statement = statement.with_for_update().execution_options(populate_existing=True)
    authorization = (await db.execute(statement)).scalar_one_or_none()
    if authorization is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Support authorization not found")
    if required_scope.value not in _authorization_scopes(authorization):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=_detail(
                "SUPPORT_AUTHORIZATION_SCOPE_MISSING",
                "Support authorization does not cover this action",
            ),
        )

    now = datetime.now(UTC)
    active = (
        authorization.status == SupportAuthorizationStatus.ACTIVE
        and _as_utc(authorization.valid_from) <= now < _as_utc(authorization.valid_until)
    )
    if not active and not allow_inactive_for_cancellation:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "SUPPORT_AUTHORIZATION_INACTIVE",
                "Support authorization is not active",
            ),
        )
    if (
        required_scope != SupportAuthorizationScope.CANCEL_SUPPORT_MANAGED_ORDER
        and authorization.created_by_admin_user_id == acting_admin_id
    ):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=_detail(
                "SUPPORT_AUTHORIZATION_SELF_USE_FORBIDDEN",
                "The administrator who recorded a mandate cannot use it to publish or edit",
            ),
        )
    return authorization


async def _load_catalog(
    db: AsyncSession,
    order_data: OrderCreate,
) -> tuple[Product, DeliveryPoint | None]:
    product = (
        await db.execute(
            select(Product).where(
                Product.id == order_data.product_id,
                Product.is_active.is_(True),
                canonical_market_product_expression(Product).is_not(None),
            )
        )
    ).scalars().first()
    if product is None:
        raise HTTPException(status_code=400, detail="Invalid product_id")

    delivery_point: DeliveryPoint | None = None
    if order_data.delivery_point_id is not None:
        delivery_point = (
            await db.execute(
                select(DeliveryPoint).where(
                    DeliveryPoint.id == order_data.delivery_point_id,
                    canonical_delivery_point_clause(DeliveryPoint),
                )
            )
        ).scalars().first()
        if delivery_point is None:
            raise HTTPException(status_code=400, detail="Invalid delivery_point_id")
    return product, delivery_point


def _validate_order_against_authorization(
    *,
    order_data: OrderCreate,
    authorization: OrganizationSupportAuthorization,
    now: datetime,
) -> None:
    if order_data.side != OrderSide.ASK or authorization.allowed_side != OrderSide.ASK.value:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ASSISTED_SIDE_NOT_ALLOWED",
                "Phase 1 delegated listings are limited to ASK orders",
            ),
        )
    if not is_tradable_availability_window(order_data.availability_window):
        raise HTTPException(status_code=400, detail="Availability window is no longer open for new orders")
    if order_data.expires_at is None:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=_detail(
                "ASSISTED_ORDER_EXPIRY_REQUIRED",
                "Delegated listings require an explicit expiry",
            ),
        )

    expiry = _as_utc(order_data.expires_at)
    ttl_hours = min(
        authorization.max_order_ttl_hours,
        delegated_listing_max_ttl_hours(),
    )
    latest_expiry = min(_as_utc(authorization.valid_until), now + timedelta(hours=ttl_hours))
    if expiry > latest_expiry:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ASSISTED_ORDER_EXPIRY_OUT_OF_SCOPE",
                "Order expiry exceeds the support authorization or pilot lifetime",
                latest_permitted_expiry=latest_expiry.isoformat(),
            ),
        )
    if authorization.product_id is not None and order_data.product_id != authorization.product_id:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail("ASSISTED_PRODUCT_OUT_OF_SCOPE", "Product is outside the support authorization"),
        )
    if (
        authorization.delivery_point_id is not None
        and order_data.delivery_point_id != authorization.delivery_point_id
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ASSISTED_DELIVERY_POINT_OUT_OF_SCOPE",
                "Delivery point is outside the support authorization",
            ),
        )
    if order_data.quantity_mt > authorization.max_quantity_mt_per_order:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail(
                "ASSISTED_QUANTITY_OUT_OF_SCOPE",
                "Quantity exceeds the per-order support authorization limit",
            ),
        )
    if (
        authorization.min_price_per_mt_usd is not None
        and order_data.price_per_mt_usd < authorization.min_price_per_mt_usd
    ) or (
        authorization.max_price_per_mt_usd is not None
        and order_data.price_per_mt_usd > authorization.max_price_per_mt_usd
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail("ASSISTED_PRICE_OUT_OF_SCOPE", "Price is outside the support authorization"),
        )


def _build_candidate(
    *,
    order_data: OrderCreate,
    organization: Organization,
    accountable_user: User,
    product: Product,
    delivery_point: DeliveryPoint | None,
) -> OrderBookOrder:
    candidate = OrderBookOrder(
        organization_id=organization.id,
        owner_user_id=accountable_user.id,
        provenance=snapshot_organization_provenance(organization),
        side=OrderSide.ASK,
        product_id=order_data.product_id,
        delivery_point_id=order_data.delivery_point_id,
        port_id=order_data.port_id,
        vessel_id=order_data.vessel_id,
        quantity_mt=order_data.quantity_mt,
        remaining_quantity_mt=order_data.quantity_mt,
        price_per_mt_usd=order_data.price_per_mt_usd,
        availability_window=order_data.availability_window,
        expires_at=order_data.expires_at,
        certification_scheme=normalize_certification_scheme(order_data.certification_scheme),
    )
    candidate.certifications = list(order_data.certifications)
    for field, value in _supplier_metadata_payload(order_data).items():
        setattr(candidate, field, value)
    candidate.product = product
    candidate.delivery_point = delivery_point
    candidate.organization = organization
    return candidate


__all__ = [name for name in globals() if not name.startswith("__")]
