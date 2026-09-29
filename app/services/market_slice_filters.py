"""Exact product, delivery-point, and availability-window query filters."""
from collections.abc import Iterable
from uuid import UUID

from sqlalchemy import and_, false, tuple_
from sqlalchemy.sql.elements import ColumnElement


def exact_market_slice_clause(
    model,
    keys: Iterable[tuple[str, UUID, str]],
) -> ColumnElement[bool]:
    """Use compact axis filters only when the requested keys form a full grid."""
    unique_keys = list(dict.fromkeys(keys))
    if not unique_keys:
        return false()

    market_products = list(dict.fromkeys(key[0] for key in unique_keys))
    delivery_point_ids = list(dict.fromkeys(key[1] for key in unique_keys))
    windows = list(dict.fromkeys(key[2] for key in unique_keys))
    # Every key belongs to these axes. Equal cardinality proves that no grid
    # combination is missing; sparse selections must retain their exact tuples.
    grid_size = len(market_products) * len(delivery_point_ids) * len(windows)
    if len(unique_keys) == grid_size:
        return and_(
            model.market_product.in_(market_products),
            model.delivery_point_id.in_(delivery_point_ids),
            model.availability_window.in_(windows),
        )
    return tuple_(
        model.market_product,
        model.delivery_point_id,
        model.availability_window,
    ).in_(unique_keys)
