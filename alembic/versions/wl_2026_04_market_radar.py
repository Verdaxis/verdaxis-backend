"""add watchlist market radar targets and events

Revision ID: wl_2026_04_radar
Revises: wl_2026_04_watchlist_unique
Create Date: 2026-04-13
"""
from __future__ import annotations

import uuid

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID


revision = "wl_2026_04_radar"
down_revision = "wl_2026_04_watchlist_unique"
branch_labels = None
depends_on = None


def _derive_market_product(name: str, fuel_type: str, fuel_grade: str) -> str | None:
    normalized_name = (name or "").strip().lower()
    normalized_type = (fuel_type or "").strip().lower()
    normalized_grade = (fuel_grade or "").strip().lower()

    if normalized_name in {"bio methanol", "methanol green"}:
        return "BIO_METHANOL"
    if normalized_name == "e-methanol":
        return "E_METHANOL"
    if normalized_name in {"bio ethanol", "ethanol green"}:
        return "BIO_ETHANOL"
    if normalized_name == "synthetic ethanol":
        return "SYNTHETIC_ETHANOL"

    if normalized_type == "methanol" and normalized_grade in {"bio", "green"}:
        return "BIO_METHANOL"
    if normalized_type == "methanol" and normalized_grade in {"e", "synthetic"}:
        return "E_METHANOL"
    if normalized_type == "ethanol" and normalized_grade in {"bio", "green"}:
        return "BIO_ETHANOL"
    if normalized_type == "ethanol" and normalized_grade == "synthetic":
        return "SYNTHETIC_ETHANOL"

    return None


def upgrade() -> None:
    op.add_column(
        "watchlists",
        sa.Column("kind", sa.String(length=32), nullable=False, server_default="CUSTOM"),
    )
    op.create_index("ix_watchlists_user_kind", "watchlists", ["user_id", "kind"])
    op.create_index(
        "uq_watchlists_user_default_radar",
        "watchlists",
        ["user_id"],
        unique=True,
        sqlite_where=sa.text("kind = 'RADAR_DEFAULT'"),
        postgresql_where=sa.text("kind = 'RADAR_DEFAULT'"),
    )

    op.create_table(
        "watchlist_targets",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("watchlist_id", UUID(as_uuid=True), nullable=False),
        sa.Column("target_type", sa.String(length=16), nullable=False),
        sa.Column("market_product_code", sa.String(length=64), nullable=True),
        sa.Column("delivery_point_id", UUID(as_uuid=True), nullable=True),
        sa.Column("availability_window_code", sa.String(length=32), nullable=True),
        sa.Column("order_id", UUID(as_uuid=True), nullable=True),
        sa.Column("snapshot_price_per_mt_usd", sa.Float(), nullable=True),
        sa.Column("snapshot_quantity_mt", sa.Float(), nullable=True),
        sa.Column("snapshot_remaining_quantity_mt", sa.Float(), nullable=True),
        sa.Column("snapshot_status", sa.String(length=32), nullable=True),
        sa.Column("snapshot_side", sa.String(length=8), nullable=True),
        sa.Column("snapshot_market_product", sa.String(length=64), nullable=True),
        sa.Column("snapshot_delivery_point_name", sa.String(length=120), nullable=True),
        sa.Column("snapshot_availability_window", sa.String(length=32), nullable=True),
        sa.Column("snapshot_counterparty_label", sa.String(length=120), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.text("now()"), nullable=False),
        sa.ForeignKeyConstraint(["watchlist_id"], ["watchlists.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"]),
        sa.ForeignKeyConstraint(["order_id"], ["orderbook_orders.id"]),
        sa.PrimaryKeyConstraint("id"),
        sa.CheckConstraint(
            "(target_type = 'SLICE' AND order_id IS NULL AND market_product_code IS NOT NULL AND delivery_point_id IS NOT NULL AND availability_window_code IS NOT NULL) "
            "OR (target_type = 'PIN' AND order_id IS NOT NULL AND market_product_code IS NOT NULL AND delivery_point_id IS NOT NULL AND availability_window_code IS NOT NULL)",
            name="ck_watchlist_targets_valid_shape",
        ),
    )
    op.create_index("ix_watchlist_targets_watchlist_id", "watchlist_targets", ["watchlist_id"])
    op.create_index("ix_watchlist_targets_order_id", "watchlist_targets", ["order_id"])
    op.create_index(
        "uq_watchlist_targets_slice_key",
        "watchlist_targets",
        ["watchlist_id", "market_product_code", "delivery_point_id", "availability_window_code"],
        unique=True,
        sqlite_where=sa.text("target_type = 'SLICE'"),
        postgresql_where=sa.text("target_type = 'SLICE'"),
    )
    op.create_index(
        "uq_watchlist_targets_pin_order",
        "watchlist_targets",
        ["watchlist_id", "order_id"],
        unique=True,
        sqlite_where=sa.text("target_type = 'PIN'"),
        postgresql_where=sa.text("target_type = 'PIN'"),
    )

    op.create_table(
        "watchlist_events",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("watchlist_id", UUID(as_uuid=True), nullable=False),
        sa.Column("watchlist_target_id", UUID(as_uuid=True), nullable=False),
        sa.Column("event_type", sa.String(length=32), nullable=False),
        sa.Column("event_payload", sa.JSON(), nullable=False, server_default=sa.text("'{}'")),
        sa.Column("is_read", sa.Boolean(), nullable=False, server_default=sa.text("false")),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.text("now()"), nullable=False),
        sa.ForeignKeyConstraint(["watchlist_id"], ["watchlists.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(["watchlist_target_id"], ["watchlist_targets.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_watchlist_events_target_created_at", "watchlist_events", ["watchlist_target_id", "created_at"])
    op.create_index("ix_watchlist_events_watchlist_created_at", "watchlist_events", ["watchlist_id", "created_at", "id"])
    op.create_index("ix_watchlist_events_watchlist_is_read_created", "watchlist_events", ["watchlist_id", "is_read", "created_at"])

    connection = op.get_bind()
    watchlists = sa.table(
        "watchlists",
        sa.column("id", UUID(as_uuid=True)),
        sa.column("user_id", UUID(as_uuid=True)),
        sa.column("name", sa.String()),
        sa.column("kind", sa.String()),
    )
    entries = sa.table(
        "watchlist_entries",
        sa.column("watchlist_id", UUID(as_uuid=True)),
        sa.column("product_id", UUID(as_uuid=True)),
        sa.column("delivery_point_id", UUID(as_uuid=True)),
    )
    products = sa.table(
        "products",
        sa.column("id", UUID(as_uuid=True)),
        sa.column("name", sa.String()),
        sa.column("fuel_type", sa.String()),
        sa.column("fuel_grade", sa.String()),
    )
    targets = sa.table(
        "watchlist_targets",
        sa.column("id", UUID(as_uuid=True)),
        sa.column("watchlist_id", UUID(as_uuid=True)),
        sa.column("target_type", sa.String()),
        sa.column("market_product_code", sa.String()),
        sa.column("delivery_point_id", UUID(as_uuid=True)),
        sa.column("availability_window_code", sa.String()),
        sa.column("snapshot_market_product", sa.String()),
        sa.column("snapshot_availability_window", sa.String()),
    )

    legacy_watchlists = connection.execute(sa.select(watchlists.c.id, watchlists.c.user_id)).all()
    user_rows = connection.execute(sa.select(sa.distinct(watchlists.c.user_id)).where(watchlists.c.user_id.is_not(None))).all()
    radar_by_user: dict[uuid.UUID, uuid.UUID] = {}
    for (user_id,) in user_rows:
        radar_id = uuid.uuid4()
        radar_by_user[user_id] = radar_id
        connection.execute(
            watchlists.insert().values(
                id=radar_id,
                user_id=user_id,
                name="Market Radar",
                kind="RADAR_DEFAULT",
            )
        )

    if legacy_watchlists:
        connection.execute(
            watchlists.update()
            .where(watchlists.c.id.in_([row.id for row in legacy_watchlists if row.user_id in radar_by_user]))
            .values(kind="LEGACY_ARCHIVED")
        )

    join_stmt = (
        sa.select(
            watchlists.c.user_id,
            entries.c.delivery_point_id,
            products.c.name,
            products.c.fuel_type,
            products.c.fuel_grade,
        )
        .select_from(entries.join(watchlists, entries.c.watchlist_id == watchlists.c.id).join(products, entries.c.product_id == products.c.id))
        .where(entries.c.delivery_point_id.is_not(None))
    )
    deduped: set[tuple[uuid.UUID, str, uuid.UUID, str]] = set()
    for row in connection.execute(join_stmt).all():
        market_product_code = _derive_market_product(row.name, row.fuel_type, row.fuel_grade)
        if not market_product_code or row.user_id not in radar_by_user:
            continue
        key = (row.user_id, market_product_code, row.delivery_point_id, "SPOT")
        if key in deduped:
            continue
        deduped.add(key)
        connection.execute(
            targets.insert().values(
                id=uuid.uuid4(),
                watchlist_id=radar_by_user[row.user_id],
                target_type="SLICE",
                market_product_code=market_product_code,
                delivery_point_id=row.delivery_point_id,
                availability_window_code="SPOT",
                snapshot_market_product=market_product_code,
                snapshot_availability_window="SPOT",
            )
        )


def downgrade() -> None:
    op.drop_index("ix_watchlist_events_watchlist_is_read_created", table_name="watchlist_events")
    op.drop_index("ix_watchlist_events_watchlist_created_at", table_name="watchlist_events")
    op.drop_index("ix_watchlist_events_target_created_at", table_name="watchlist_events")
    op.drop_table("watchlist_events")

    op.drop_index("uq_watchlist_targets_pin_order", table_name="watchlist_targets")
    op.drop_index("uq_watchlist_targets_slice_key", table_name="watchlist_targets")
    op.drop_index("ix_watchlist_targets_order_id", table_name="watchlist_targets")
    op.drop_index("ix_watchlist_targets_watchlist_id", table_name="watchlist_targets")
    op.drop_table("watchlist_targets")

    op.drop_index("uq_watchlists_user_default_radar", table_name="watchlists")
    op.drop_index("ix_watchlists_user_kind", table_name="watchlists")
    op.drop_column("watchlists", "kind")
