"""Declarations only for migrator-owned public cache metadata."""
from sqlalchemy import BigInteger, Boolean, CheckConstraint, Text, false
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


class PublicReadCacheGeneration(Base):
    __tablename__ = "public_read_cache_generations"
    __table_args__ = (CheckConstraint("generation >= 0", name="ck_public_cache_generation_nonnegative"),)
    domain: Mapped[str] = mapped_column(Text, primary_key=True)
    generation: Mapped[int] = mapped_column(BigInteger, nullable=False, server_default="0")


class PublicReadCacheEvent(Base):
    __tablename__ = "public_read_cache_events"
    __table_args__ = (CheckConstraint("generation > 0", name="ck_public_cache_event_positive"),)
    domain: Mapped[str] = mapped_column(Text, primary_key=True)
    generation: Mapped[int] = mapped_column(BigInteger, primary_key=True)


class PublicReadCacheIncarnation(Base):
    __tablename__ = "public_read_cache_incarnation"
    __table_args__ = (CheckConstraint("singleton", name="ck_public_cache_singleton"),)
    singleton: Mapped[bool] = mapped_column(Boolean, primary_key=True)
    database_epoch: Mapped[str] = mapped_column(Text, nullable=False)
    primary_identity: Mapped[str] = mapped_column(Text, nullable=False)
    authority_revision: Mapped[int] = mapped_column(BigInteger, nullable=False, server_default="0")
    active: Mapped[bool] = mapped_column(Boolean, nullable=False, server_default=false())
