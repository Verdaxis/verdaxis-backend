"""Non-executable supplier indications for discovery and targeted RFQs."""

from datetime import UTC, datetime
from decimal import Decimal
from uuid import UUID, uuid4

from sqlalchemy import (
    CheckConstraint,
    DateTime,
    ForeignKey,
    Index,
    Integer,
    JSON,
    Numeric,
    String,
    UniqueConstraint,
)
from sqlalchemy.orm import Mapped, mapped_column

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.market_constraints import postgresql_check
from app.model_base import Base


class SupplierOffer(Base):
    __tablename__ = "supplier_offers"
    __table_args__ = (
        postgresql_check(
            f"product_id = '{PRODUCT_IDS['UCOME_B100']}' AND delivery_point_id = '{DELIVERY_POINT_IDS['Singapore']}'",
            name="ck_supplier_offers_catalog",
        ),
        postgresql_check(
            "quantity_mt >= 1 AND quantity_mt <= 100000 AND quantity_mt * 100 = trunc(quantity_mt * 100) "
            "AND min_fill_mt >= 1 AND min_fill_mt <= quantity_mt AND min_fill_mt * 100 = trunc(min_fill_mt * 100) "
            "AND price_per_mt_usd >= 0.01 AND price_per_mt_usd <= 1000000 AND price_per_mt_usd * 100 = trunc(price_per_mt_usd * 100)",
            name="ck_supplier_offers_numeric_values",
        ),
        CheckConstraint(
            "status IN ('OPEN', 'WITHDRAWN')", name="ck_supplier_offers_status"
        ),
        CheckConstraint("revision >= 1", name="ck_supplier_offers_revision"),
        CheckConstraint(
            "(idempotency_key IS NULL AND idempotency_request_hash IS NULL) OR "
            "(idempotency_key IS NOT NULL AND idempotency_request_hash IS NOT NULL)",
            name="ck_supplier_offers_idempotency",
        ),
        UniqueConstraint(
            "supplier_org_id",
            "idempotency_key",
            name="uq_supplier_offers_org_idempotency",
        ),
        Index("ix_supplier_offers_supplier_org_id", "supplier_org_id"),
        Index(
            "ix_supplier_offers_public",
            "status",
            "product_id",
            "delivery_point_id",
            "expires_at",
        ),
    )

    id: Mapped[UUID] = mapped_column(primary_key=True, default=uuid4)
    supplier_org_id: Mapped[UUID] = mapped_column(ForeignKey("organizations.id"))
    supplier_user_id: Mapped[UUID] = mapped_column(ForeignKey("users.id"))
    product_id: Mapped[UUID] = mapped_column(ForeignKey("products.id"))
    delivery_point_id: Mapped[UUID] = mapped_column(ForeignKey("delivery_points.id"))
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric())
    min_fill_mt: Mapped[Decimal] = mapped_column(Numeric())
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric())
    availability_window: Mapped[str] = mapped_column(String(16))
    listing_terms: Mapped[dict] = mapped_column(JSON(none_as_null=True))
    status: Mapped[str] = mapped_column(
        String(16), default="OPEN", server_default="OPEN"
    )
    revision: Mapped[int] = mapped_column(Integer, default=1, server_default="1")
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True))
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )
    idempotency_key: Mapped[str | None] = mapped_column(String(255))
    idempotency_request_hash: Mapped[str | None] = mapped_column(String(64))
