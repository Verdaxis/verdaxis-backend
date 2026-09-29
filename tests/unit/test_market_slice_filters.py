"""Exact market-slice predicates preserve sparse selections and shrink full grids."""

from itertools import product
from uuid import uuid4

from sqlalchemy import Column, MetaData, String, Table, create_engine, select, tuple_
from sqlalchemy.dialects import sqlite
from sqlalchemy.types import Uuid

from app.services.market_slice_filters import exact_market_slice_clause


def _market_slice_model():
    metadata = MetaData()
    table = Table(
        "market_slices",
        metadata,
        Column("market_product", String, nullable=False),
        Column("delivery_point_id", Uuid(as_uuid=True), nullable=False),
        Column("availability_window", String, nullable=False),
    )

    class Model:
        market_product = table.c.market_product
        delivery_point_id = table.c.delivery_point_id
        availability_window = table.c.availability_window

    return table, Model


def _query_rows(table, model, requested_keys, stored_rows=None):
    stored_rows = requested_keys if stored_rows is None else stored_rows
    engine = create_engine("sqlite:///:memory:")
    table.metadata.create_all(engine)
    with engine.begin() as connection:
        connection.execute(table.insert(), [
            {
                "market_product": product_name,
                "delivery_point_id": delivery_point_id,
                "availability_window": window,
            }
            for product_name, delivery_point_id, window in stored_rows
        ])
    with engine.connect() as connection:
        rows = connection.execute(
            select(
                model.market_product,
                model.delivery_point_id,
                model.availability_window,
        ).where(exact_market_slice_clause(model, requested_keys))
        ).all()
    return set(rows)


def test_complete_grid_uses_independent_axis_filters_without_changing_results():
    table, model = _market_slice_model()
    delivery_points = [uuid4(), uuid4()]
    keys = list(product(
        ("BIO_METHANOL", "UCOME_B100"),
        delivery_points,
        ("SPOT", "2027-Q1"),
    ))
    outsider = ("BIO_METHANOL", delivery_points[0], "2027-Q2")

    rows = _query_rows(table, model, keys, [*keys, outsider])

    assert rows == set(keys)


def test_sparse_keys_do_not_admit_unrequested_cross_product():
    table, model = _market_slice_model()
    delivery_points = [uuid4(), uuid4()]
    keys = [
        ("BIO_METHANOL", delivery_points[0], "SPOT"),
        ("UCOME_B100", delivery_points[1], "2027-Q1"),
    ]
    cross_product_rows = [
        (product_name, delivery_point_id, window)
        for product_name, delivery_point_id, window in product(
            ("BIO_METHANOL", "UCOME_B100"), delivery_points, ("SPOT", "2027-Q1")
        )
        if (product_name, delivery_point_id, window) not in keys
    ]

    assert _query_rows(table, model, keys, [*keys, *cross_product_rows]) == set(keys)


def test_empty_keys_match_no_rows():
    table, model = _market_slice_model()
    key = ("BIO_METHANOL", uuid4(), "SPOT")

    assert _query_rows(table, model, [key]) == {key}
    assert _query_rows(table, model, [], [key]) == set()


def test_duplicate_keys_cannot_fake_a_complete_grid():
    table, model = _market_slice_model()
    delivery_points = [uuid4(), uuid4()]
    all_keys = list(product(
        ("BIO_METHANOL", "UCOME_B100"),
        delivery_points,
        ("SPOT", "2027-Q1"),
    ))
    sparse_keys = all_keys[:-1]

    # Seven unique keys plus one duplicate has the raw length of the 2x2x2 grid.
    requested_keys = [*sparse_keys, sparse_keys[0]]

    assert _query_rows(table, model, all_keys) == set(all_keys)
    assert _query_rows(table, model, requested_keys, all_keys) == set(sparse_keys)


def test_full_6_by_8_by_22_grid_reduces_bound_values():
    _, model = _market_slice_model()
    products = [f"PRODUCT-{index}" for index in range(6)]
    delivery_points = [uuid4() for _ in range(8)]
    windows = [f"20{27 + index // 4:02d}-Q{index % 4 + 1}" for index in range(22)]
    keys = list(product(products, delivery_points, windows))

    compact_statement = select(model.market_product).where(
        exact_market_slice_clause(model, keys)
    )
    compact_params = compact_statement.compile(
        dialect=sqlite.dialect(),
        compile_kwargs={"render_postcompile": True},
    ).params

    exact_statement = select(model.market_product).where(tuple_(
        model.market_product,
        model.delivery_point_id,
        model.availability_window,
    ).in_(keys))
    exact_params = exact_statement.compile(
        dialect=sqlite.dialect(),
        compile_kwargs={"render_postcompile": True},
    ).params

    assert len(keys) == 6 * 8 * 22 == 1_056
    assert len(compact_params) == 6 + 8 + 22 == 36
    assert len(exact_params) == len(keys) * 3
    assert len(compact_params) < len(exact_params)
