from sqlalchemy import String, Integer, DateTime, Index, Text
from sqlalchemy.orm import Mapped, mapped_column
from sqlalchemy.dialects.postgresql import UUID
import uuid
from datetime import datetime, UTC
from typing import Optional
from app.database import Base


class NewsItem(Base):
    __tablename__ = "news_items"
    __table_args__ = (
        Index("ix_news_items_published_at", "published_at"),
        Index("ix_news_items_category", "category"),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    title: Mapped[str] = mapped_column(String(500), nullable=False)
    summary: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    source: Mapped[str] = mapped_column(String(100), nullable=False)
    source_url: Mapped[str] = mapped_column(String(500), nullable=False)
    url: Mapped[str] = mapped_column(String(1000), nullable=False, unique=True)
    category: Mapped[str] = mapped_column(
        String(50), nullable=False, default="markets"
    )
    relevance: Mapped[int] = mapped_column(Integer, nullable=False, default=3)
    published_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False
    )
    fetched_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False, default=lambda: datetime.now(UTC)
    )
