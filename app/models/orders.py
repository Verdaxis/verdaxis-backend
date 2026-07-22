from sqlalchemy import String, ForeignKey, Enum, Numeric, Date, DateTime
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime, date
from decimal import Decimal
from app.model_base import Base


class CommissionStatus(str, enum.Enum):
    PENDING = "PENDING"       # Order completed, commission calculated
    INVOICED = "INVOICED"     # Invoice sent to parties
    PAID = "PAID"             # Payment received


class Commission(Base):
    """
    Tracks commission owed to Verdaxis from completed trades.
    Used by Admin dashboard to monitor revenue.
    """
    __tablename__ = "commissions"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)

    # Legacy identifier retained for historical commission rows. The old
    # ``orders`` table is not part of the current Alembic parent schema, so it
    # is intentionally an identifier rather than a dangling ORM foreign key.
    match_id: Mapped[uuid.UUID] = mapped_column(unique=True, nullable=False)
    # New FK to trades table
    trade_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("trades.id"), nullable=True)

    # Financials
    amount_usd: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    status: Mapped[CommissionStatus] = mapped_column(
        Enum(CommissionStatus, native_enum=False, length=8),
        default=CommissionStatus.PENDING
    )

    # Invoice tracking
    invoice_number: Mapped[str | None] = mapped_column(String(50))
    invoice_date: Mapped[date | None] = mapped_column(Date)
    payment_date: Mapped[date | None] = mapped_column(Date)

    notes: Mapped[str | None] = mapped_column(String(500))

    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow)

    # Relationships
    trade = relationship("Trade", back_populates="commission")
