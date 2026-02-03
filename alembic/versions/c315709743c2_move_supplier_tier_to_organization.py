"""Move supplier_tier to Organization

Revision ID: c315709743c2
Revises: 14b159f248ac
Create Date: 2026-02-03 10:19:51.302562

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = 'c315709743c2'
down_revision: Union[str, Sequence[str], None] = '14b159f248ac'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Add supplier_tier to organizations
    op.add_column('organizations', sa.Column('supplier_tier', sa.String(length=50), nullable=False, server_default='Independent Supplier'))
    
    # Remove tier_label from public_listings
    op.drop_column('public_listings', 'tier_label')


def downgrade() -> None:
    # Add tier_label back to public_listings
    op.add_column('public_listings', sa.Column('tier_label', sa.String(length=50), nullable=False, server_default='Regional Supplier'))
    
    # Remove supplier_tier from organizations
    op.drop_column('organizations', 'supplier_tier')
