"""Legacy tables retained only to resolve historical model foreign keys.

These tables are owned by older migrations and are intentionally excluded from
Alembic model/schema comparison. The lightweight definitions keep metadata
sorting and ``alembic check`` from crashing on historical FKs.
"""

from sqlalchemy import Column, Table
from sqlalchemy.dialects.postgresql import UUID

from app.database import Base


orders = Table(
    "orders",
    Base.metadata,
    Column("id", UUID(as_uuid=True), primary_key=True),
)

direct_orders = Table(
    "direct_orders",
    Base.metadata,
    Column("id", UUID(as_uuid=True), primary_key=True),
)
