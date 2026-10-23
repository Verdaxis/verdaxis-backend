"""Metadata for the watchlist producer watermark."""
from datetime import datetime

from sqlalchemy import BigInteger, Boolean, CheckConstraint, DateTime, text
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


class WatchlistProducerState(Base):
    __tablename__ = "watchlist_producer_state"

    singleton: Mapped[bool] = mapped_column(
        Boolean(), CheckConstraint("singleton"), primary_key=True, nullable=False,
    )
    consumed_through_seq: Mapped[int] = mapped_column(
        BigInteger(), CheckConstraint("consumed_through_seq >= 0"),
        nullable=False, server_default="0",
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False, server_default=text("now()"),
    )
