"""rename_rfq_to_orders

Revision ID: f10cf6fa2019
Revises: c5e4af769a3a
Create Date: 2026-02-01 16:34:13.011920

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = 'f10cf6fa2019'
down_revision: Union[str, Sequence[str], None] = 'c5e4af769a3a'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    """Upgrade schema."""
    op.rename_table('rfq_matches', 'orders')


def downgrade() -> None:
    """Downgrade schema."""
    op.rename_table('orders', 'rfq_matches')
