"""Read-only Forward Curve monitoring signal models."""
from __future__ import annotations

import uuid
from datetime import datetime, timezone
from decimal import Decimal

from sqlalchemy import Boolean, CheckConstraint, DateTime, ForeignKey, Index, Numeric, String, text
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.database import Base


def _utcnow() -> datetime:
    return datetime.now(timezone.utc)


_MARKET_PRODUCT_CHECK = (
    "market_product IN ('BIO_METHANOL', 'E_METHANOL', 'BIO_ETHANOL', 'SYNTHETIC_ETHANOL')"
)
_WINDOW_CHECK = (
    "availability_window = 'SPOT' "
    "OR availability_window LIKE '____-__' "
    "OR availability_window LIKE '____-Q_' "
    "OR availability_window LIKE '____-CAL'"
)
_REAL_VERIFICATION_CHECK = "is_verified_real = false OR is_demo = false"


class MarketSignalIngestionRun(Base):
    """Trusted ingestion run used to distinguish verified real feeds from manual rows."""

    __tablename__ = "market_signal_ingestion_runs"
    __table_args__ = (
        CheckConstraint(
            "signal_family IN ('MARKET_INDICATION', 'FAIR_PRICE_BAND', 'PHYSICAL_STEM')",
            name="ck_market_signal_ingestion_runs_family",
        ),
        CheckConstraint(
            "source_kind IN ('MARKET_INDICATION', 'FAIR_PRICE_MODEL', 'PHYSICAL_STEM')",
            name="ck_market_signal_ingestion_runs_source_kind",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    signal_family: Mapped[str] = mapped_column(String(32), nullable=False)
    source: Mapped[str] = mapped_column(String(64), nullable=False)
    source_kind: Mapped[str] = mapped_column(String(32), nullable=False)
    started_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=_utcnow,
        server_default=text("CURRENT_TIMESTAMP"),
    )
    verified_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=_utcnow,
        server_default=text("CURRENT_TIMESTAMP"),
    )


class MarketIndication(Base):
    """Append-only non-executable market indication."""

    __tablename__ = "market_indications"
    __table_args__ = (
        CheckConstraint(_MARKET_PRODUCT_CHECK, name="ck_market_indications_market_product"),
        CheckConstraint(_WINDOW_CHECK, name="ck_market_indications_availability_window"),
        CheckConstraint("side IN ('BID', 'ASK', 'MID')", name="ck_market_indications_side"),
        CheckConstraint("price_per_mt_usd > 0", name="ck_market_indications_price_positive"),
        CheckConstraint("quantity_mt IS NULL OR quantity_mt > 0", name="ck_market_indications_quantity_positive"),
        CheckConstraint(_REAL_VERIFICATION_CHECK, name="ck_market_indications_real_verification"),
        Index(
            "ix_market_indications_board_lookup",
            "market_product",
            "delivery_point_id",
            "availability_window",
            "observed_at",
            "created_at",
            "id",
        ),
        Index(
            "ix_market_indications_latest_side",
            "market_product",
            "delivery_point_id",
            "availability_window",
            "side",
            "observed_at",
            "created_at",
            "id",
        ),
        Index(
            "uq_market_indications_source_event",
            "source",
            "source_event_id",
            unique=True,
            sqlite_where=text("source_event_id IS NOT NULL"),
            postgresql_where=text("source_event_id IS NOT NULL"),
        ),
        Index(
            "ix_market_indications_source_record",
            "source",
            "source_record_id",
            "observed_at",
            "created_at",
            "id",
            sqlite_where=text("source_record_id IS NOT NULL"),
            postgresql_where=text("source_record_id IS NOT NULL"),
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    market_product: Mapped[str] = mapped_column(String(64), nullable=False)
    delivery_point_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=False
    )
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False)
    side: Mapped[str] = mapped_column(String(8), nullable=False)
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    quantity_mt: Mapped[Decimal | None] = mapped_column(Numeric(12, 2), nullable=True)
    source: Mapped[str] = mapped_column(String(64), nullable=False)
    source_record_id: Mapped[str | None] = mapped_column(String(128), nullable=True)
    source_event_id: Mapped[str | None] = mapped_column(String(128), nullable=True)
    trusted_ingestion_run_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("market_signal_ingestion_runs.id"), nullable=True
    )
    is_demo: Mapped[bool] = mapped_column(Boolean, nullable=False, default=True, server_default="true")
    is_verified_real: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False, server_default="false")
    verified_real_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    observed_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=_utcnow,
        server_default=text("CURRENT_TIMESTAMP"),
    )


class FairPriceBand(Base):
    """Append-only fair-price band signal."""

    __tablename__ = "fair_price_bands"
    __table_args__ = (
        CheckConstraint(_MARKET_PRODUCT_CHECK, name="ck_fair_price_bands_market_product"),
        CheckConstraint(_WINDOW_CHECK, name="ck_fair_price_bands_availability_window"),
        CheckConstraint("low_price_per_mt_usd > 0", name="ck_fair_price_bands_low_positive"),
        CheckConstraint(
            "mid_price_per_mt_usd >= low_price_per_mt_usd",
            name="ck_fair_price_bands_mid_above_low",
        ),
        CheckConstraint(
            "high_price_per_mt_usd >= mid_price_per_mt_usd",
            name="ck_fair_price_bands_high_above_mid",
        ),
        CheckConstraint(_REAL_VERIFICATION_CHECK, name="ck_fair_price_bands_real_verification"),
        Index(
            "ix_fair_price_bands_board_lookup",
            "market_product",
            "delivery_point_id",
            "availability_window",
            "observed_at",
            "created_at",
            "id",
        ),
        Index(
            "uq_fair_price_bands_source_event",
            "source",
            "source_event_id",
            unique=True,
            sqlite_where=text("source_event_id IS NOT NULL"),
            postgresql_where=text("source_event_id IS NOT NULL"),
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    market_product: Mapped[str] = mapped_column(String(64), nullable=False)
    delivery_point_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=False
    )
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False)
    low_price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    mid_price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    high_price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    model_name: Mapped[str] = mapped_column(String(64), nullable=False)
    model_version: Mapped[str | None] = mapped_column(String(64), nullable=True)
    source: Mapped[str] = mapped_column(String(64), nullable=False)
    source_event_id: Mapped[str | None] = mapped_column(String(128), nullable=True)
    trusted_ingestion_run_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("market_signal_ingestion_runs.id"), nullable=True
    )
    is_demo: Mapped[bool] = mapped_column(Boolean, nullable=False, default=True, server_default="true")
    is_verified_real: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False, server_default="false")
    verified_real_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    observed_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=_utcnow,
        server_default=text("CURRENT_TIMESTAMP"),
    )


class PhysicalStem(Base):
    """Append-only physical availability signal."""

    __tablename__ = "physical_stems"
    __table_args__ = (
        CheckConstraint(_MARKET_PRODUCT_CHECK, name="ck_physical_stems_market_product"),
        CheckConstraint(_WINDOW_CHECK, name="ck_physical_stems_availability_window"),
        CheckConstraint("quantity_mt > 0", name="ck_physical_stems_quantity_positive"),
        CheckConstraint(
            "status IN ('AVAILABLE', 'TENTATIVE', 'ALLOCATED', 'CANCELLED')",
            name="ck_physical_stems_status",
        ),
        CheckConstraint(
            "stem_end IS NULL OR stem_start IS NULL OR stem_end >= stem_start",
            name="ck_physical_stems_date_order",
        ),
        CheckConstraint("stem_uid <> ''", name="ck_physical_stems_stem_uid_nonempty"),
        CheckConstraint(_REAL_VERIFICATION_CHECK, name="ck_physical_stems_real_verification"),
        Index(
            "ix_physical_stems_board_lookup",
            "market_product",
            "delivery_point_id",
            "availability_window",
            "observed_at",
            "created_at",
            "id",
        ),
        Index(
            "ix_physical_stems_latest_uid",
            "market_product",
            "delivery_point_id",
            "availability_window",
            "source",
            "stem_uid",
            "observed_at",
            "created_at",
            "id",
        ),
        Index(
            "uq_physical_stems_source_event",
            "source",
            "source_event_id",
            unique=True,
            sqlite_where=text("source_event_id IS NOT NULL"),
            postgresql_where=text("source_event_id IS NOT NULL"),
        ),
        Index(
            "ix_physical_stems_source_record",
            "source",
            "source_record_id",
            "observed_at",
            "created_at",
            "id",
            sqlite_where=text("source_record_id IS NOT NULL"),
            postgresql_where=text("source_record_id IS NOT NULL"),
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    market_product: Mapped[str] = mapped_column(String(64), nullable=False)
    delivery_point_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=False
    )
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False)
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    stem_start: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    stem_end: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    status: Mapped[str] = mapped_column(String(24), nullable=False)
    source: Mapped[str] = mapped_column(String(64), nullable=False)
    stem_uid: Mapped[str] = mapped_column(String(128), nullable=False)
    source_record_id: Mapped[str | None] = mapped_column(String(128), nullable=True)
    source_event_id: Mapped[str | None] = mapped_column(String(128), nullable=True)
    trusted_ingestion_run_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("market_signal_ingestion_runs.id"), nullable=True
    )
    is_demo: Mapped[bool] = mapped_column(Boolean, nullable=False, default=True, server_default="true")
    is_verified_real: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False, server_default="false")
    verified_real_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    observed_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=_utcnow,
        server_default=text("CURRENT_TIMESTAMP"),
    )
