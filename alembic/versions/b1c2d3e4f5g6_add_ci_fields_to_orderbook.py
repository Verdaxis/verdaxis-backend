"""add CI fields to orderbook_orders

Revision ID: b1c2d3e4f5g6
Revises: a1b2c3d4e5f6
Create Date: 2026-02-12 23:00:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = 'b1c2d3e4f5g6'
down_revision: Union[str, None] = 'a1b2c3d4e5f6'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column('orderbook_orders', sa.Column(
        'carbon_intensity_gco2_mj', sa.Numeric(8, 2), nullable=True,
        comment='gCO2eq/MJ well-to-wake'
    ))
    op.add_column('orderbook_orders', sa.Column(
        'energy_density_mj_kg', sa.Numeric(6, 2), nullable=True,
        comment='MJ/kg lower heating value'
    ))


def downgrade() -> None:
    op.drop_column('orderbook_orders', 'energy_density_mj_kg')
    op.drop_column('orderbook_orders', 'carbon_intensity_gco2_mj')
