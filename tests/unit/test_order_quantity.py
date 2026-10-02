from decimal import Decimal
from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from app.services.order_quantity import require_minimum_order_quantity


def _product(minimum):
    return SimpleNamespace(
        min_lot_size=minimum,
        unit="MT",
        name="Bio Methanol",
    )


def test_minimum_order_quantity_accepts_exact_boundary():
    quantity = require_minimum_order_quantity(
        quantity_mt=Decimal("200.00"),
        product=_product(Decimal("200.00")),
    )

    assert quantity == Decimal("200.00")


def test_minimum_order_quantity_rejects_below_boundary_with_clear_detail():
    with pytest.raises(HTTPException) as failure:
        require_minimum_order_quantity(
            quantity_mt=Decimal("199.99"),
            product=_product(Decimal("200.00")),
        )

    assert failure.value.status_code == 400
    assert failure.value.detail == (
        "Order quantity must be at least 200.00 MT for Bio Methanol"
    )


@pytest.mark.parametrize(
    "minimum",
    [None, Decimal("0.00"), Decimal("NaN"), Decimal("200.001")],
)
def test_minimum_order_quantity_fails_closed_for_invalid_catalog_values(minimum):
    with pytest.raises(HTTPException) as failure:
        require_minimum_order_quantity(
            quantity_mt=Decimal("200.00"),
            product=_product(minimum),
        )

    assert failure.value.status_code == 409
    assert failure.value.detail == "Product minimum lot is unavailable"


def test_minimum_order_quantity_rejects_nonfinite_requested_total():
    with pytest.raises(HTTPException) as failure:
        require_minimum_order_quantity(
            quantity_mt=Decimal("NaN"),
            product=_product(Decimal("200.00")),
        )

    assert failure.value.status_code == 400
    assert failure.value.detail == "quantity_mt must be finite"
