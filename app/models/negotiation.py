"""Negotiation models — buyer/seller counteroffer flow alongside the orderbook."""
import enum
import uuid
from datetime import datetime, UTC
from decimal import Decimal

from sqlalchemy import ForeignKey, Enum, Index, Numeric, DateTime, Text, Integer, String, UniqueConstraint, text
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base


class NegotiationStatus(str, enum.Enum):
    OPEN = "OPEN"           # Initiated, awaiting counterparty response
    COUNTERED = "COUNTERED" # Counterparty submitted a counter-price
    AGREED = "AGREED"       # One party accepted → trade created
    DECLINED = "DECLINED"   # Explicitly declined
    EXPIRED = "EXPIRED"     # Time limit exceeded


class Negotiation(Base):
    __tablename__ = "negotiations"
    __table_args__ = (
        Index("ix_negotiations_initiator_org", "initiator_org_id"),
        Index("ix_negotiations_counterparty_org", "counterparty_org_id"),
        Index("ix_negotiations_status", "status"),
        Index("ix_negotiations_expires_at", "expires_at", postgresql_where=text("status IN ('OPEN', 'COUNTERED')")),
        Index("ix_negotiations_status_created", "status", text("created_at DESC")),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    bid_order_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("orderbook_orders.id", ondelete="SET NULL"), nullable=True
    )
    ask_order_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("orderbook_orders.id", ondelete="SET NULL"), nullable=True
    )
    initiator_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    counterparty_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    # Nullable only for pre-hardening rows. Such rows remain non-executable
    # until product chooses quarantine or an audited provenance mapping.
    initiator_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True, index=True
    )
    counterparty_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True, index=True
    )
    accepted_by_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True
    )
    # "BUYER" or "SELLER" — the initiator's role in this trade.
    # Set at creation from the user's org role; used to derive buyer_id/seller_id on acceptance.
    initiator_side: Mapped[str] = mapped_column(
        String(10), nullable=False, server_default="BUYER"
    )
    product_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("products.id"), nullable=False
    )
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    # The current "live" price on the table — updated each round
    current_price: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    status: Mapped[NegotiationStatus] = mapped_column(
        Enum(NegotiationStatus, native_enum=False, length=10),
        default=NegotiationStatus.OPEN,
        nullable=False,
    )
    # Who submitted the current_price (the other party must respond)
    last_actor_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    trade_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("trades.id", ondelete="SET NULL"), nullable=True
    )
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        onupdate=lambda: datetime.now(UTC),
    )

    rounds: Mapped[list["NegotiationRound"]] = relationship(
        back_populates="negotiation",
        cascade="all, delete-orphan",
        order_by="NegotiationRound.round_number",
    )


class NegotiationRound(Base):
    __tablename__ = "negotiation_rounds"
    __table_args__ = (
        UniqueConstraint("negotiation_id", "round_number", name="uq_neg_rounds_negotiation_round"),
        Index("ix_negotiation_rounds_negotiation", "negotiation_id"),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    negotiation_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("negotiations.id"), nullable=False
    )
    round_number: Mapped[int] = mapped_column(Integer, nullable=False)
    proposer_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    proposer_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True
    )
    proposed_price: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    negotiation: Mapped["Negotiation"] = relationship(back_populates="rounds")
