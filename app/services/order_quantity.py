"""Catalog minimum-lot admission for new order totals."""
from __future__ import annotations

from decimal import Decimal

from fastapi import HTTPException, status

from app.models.catalog import Product
from app.schemas.market_integrity import finite_decimal


def require_minimum_order_quantity(
    *,
    quantity_mt: Decimal,
    product: Product,
) -> Decimal:
    """Validate a requested total without constraining fills or remainders."""
    try:
        quantity = finite_decimal(quantity_mt, field_name="quantity_mt")
    except ValueError as exc:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=str(exc),
        ) from exc
    if quantity <= 0:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Order quantity must be positive",
        )

    raw_minimum = product.min_lot_size
    if raw_minimum is None:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Product minimum lot is unavailable",
        )
    try:
        minimum = finite_decimal(
            Decimal(str(raw_minimum)),
            field_name="min_lot_size",
        )
    except (ValueError, ArithmeticError) as exc:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Product minimum lot is unavailable",
        ) from exc
    if minimum <= 0:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Product minimum lot is unavailable",
        )

    if quantity < minimum:
        unit = (product.unit or "MT").strip() or "MT"
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=(
                f"Order quantity must be at least {minimum:.2f} {unit} "
                f"for {product.name}"
            ),
        )
    return quantity
