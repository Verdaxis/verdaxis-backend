"""Update supplier_tier values to match Enum keys

Revision ID: c820b25c0e18
Revises: c315709743c2
Create Date: 2026-02-03 10:31:46.472145

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = 'c820b25c0e18'
down_revision: Union[str, Sequence[str], None] = 'c315709743c2'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Update values to match Enum keys
    op.execute("UPDATE organizations SET supplier_tier = 'INDEPENDENT' WHERE supplier_tier = 'Independent Supplier'")
    op.execute("UPDATE organizations SET supplier_tier = 'TIER_1_PRODUCER' WHERE supplier_tier = 'Tier 1 Producer'")
    op.execute("UPDATE organizations SET supplier_tier = 'MAJOR_TRADER' WHERE supplier_tier = 'Major Trader'")
    op.execute("UPDATE organizations SET supplier_tier = 'REGIONAL_SUPPLIER' WHERE supplier_tier = 'Regional Supplier'")

    # Update default value
    op.alter_column('organizations', 'supplier_tier', server_default='INDEPENDENT')


def downgrade() -> None:
    # Revert values
    op.execute("UPDATE organizations SET supplier_tier = 'Independent Supplier' WHERE supplier_tier = 'INDEPENDENT'")
    op.execute("UPDATE organizations SET supplier_tier = 'Tier 1 Producer' WHERE supplier_tier = 'TIER_1_PRODUCER'")
    op.execute("UPDATE organizations SET supplier_tier = 'Major Trader' WHERE supplier_tier = 'MAJOR_TRADER'")
    op.execute("UPDATE organizations SET supplier_tier = 'Regional Supplier' WHERE supplier_tier = 'REGIONAL_SUPPLIER'")

    # Revert default value
    op.alter_column('organizations', 'supplier_tier', server_default='Independent Supplier')
