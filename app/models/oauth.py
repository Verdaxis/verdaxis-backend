"""
OAuthClient model — S7-001.

Stores API client credentials for the OAuth2 client_credentials flow.
Clients are scoped to their creator (user) and carry a rate_limit_tier
that gates data-product access.
"""
import uuid
import enum
from datetime import datetime, UTC

from sqlalchemy import String, ForeignKey, DateTime, JSON
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base


class RateLimitTier(str, enum.Enum):
    PUBLIC = "public"
    FREE = "free"
    PAID = "paid"


class OAuthClient(Base):
    __tablename__ = "oauth_clients"

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    client_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), unique=True, nullable=False, default=uuid.uuid4
    )
    client_secret_hash: Mapped[str] = mapped_column(String, nullable=False)
    name: Mapped[str] = mapped_column(String(100), nullable=False)
    scopes: Mapped[list] = mapped_column(JSON, nullable=False, default=list)
    rate_limit_tier: Mapped[str] = mapped_column(
        String(20), nullable=False, default=RateLimitTier.FREE.value
    )
    created_by: Mapped[uuid.UUID] = mapped_column(
        ForeignKey("users.id"), nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    creator: Mapped["User"] = relationship("User", foreign_keys=[created_by])
