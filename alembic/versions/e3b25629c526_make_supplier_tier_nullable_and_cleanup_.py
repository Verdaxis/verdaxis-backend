"""Make supplier_tier nullable and cleanup non-suppliers

Revision ID: e3b25629c526
Revises: c820b25c0e18
Create Date: 2026-02-03 10:37:50.803335

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = 'e3b25629c526'
down_revision: Union[str, Sequence[str], None] = 'c820b25c0e18'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # 1. Make nullable and remove server default
    op.alter_column('organizations', 'supplier_tier', existing_type=sa.VARCHAR(length=50), nullable=True, server_default=None)
    
    # 2. Set to NULL for non-FUEL_SUPPLIER
    op.execute("UPDATE organizations SET supplier_tier = NULL WHERE type != 'FUEL_SUPPLIER'")


def downgrade() -> None:
    # 1. Set default for NULLs back to INDEPENDENT
    op.execute("UPDATE organizations SET supplier_tier = 'INDEPENDENT' WHERE supplier_tier IS NULL")
    
    # 2. Revert nullable and server default
    op.alter_column('organizations', 'supplier_tier', existing_type=sa.VARCHAR(length=50), nullable=False, server_default='INDEPENDENT')
