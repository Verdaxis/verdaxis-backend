"""add producer_projects table

Revision ID: d3e4f5g6h7i8
Revises: c2d3e4f5g6h7
Create Date: 2026-02-12 23:20:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID
from geoalchemy2 import Geography


# revision identifiers, used by Alembic.
revision: str = 'd3e4f5g6h7i8'
down_revision: Union[str, None] = 'c2d3e4f5g6h7'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        'producer_projects',
        sa.Column('id', UUID(as_uuid=True), primary_key=True, server_default=sa.text('gen_random_uuid()')),
        sa.Column('name', sa.String(), nullable=False),
        sa.Column('fuel_type', sa.String(50), nullable=False),
        sa.Column('capacity_kt_per_year', sa.Numeric(10, 2), nullable=True),
        sa.Column('country', sa.String(100), nullable=False),
        sa.Column('region', sa.String(100), nullable=True),
        sa.Column('location', Geography(geometry_type='POINT', srid=4326), nullable=True),
        sa.Column('cod_date', sa.Date(), nullable=True, comment='Commercial Operation Date'),
        sa.Column('cod_year', sa.Integer(), nullable=True, comment='COD year for filtering'),
        sa.Column('status', sa.String(30), server_default='ANNOUNCED'),
        sa.Column('data_source', sa.String(50), nullable=True, comment='GENA, manual, etc.'),
        sa.Column('gena_project_id', sa.String(100), unique=True, nullable=True),
        sa.Column('organization_id', UUID(as_uuid=True), sa.ForeignKey('organizations.id'), nullable=True),
        sa.Column('feedstock', sa.String(200), nullable=True),
        sa.Column('technology', sa.String(200), nullable=True),
        sa.Column('carbon_intensity_gco2_mj', sa.Numeric(8, 2), nullable=True),
        sa.Column('notes', sa.Text(), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.func.now()),
        sa.Column('updated_at', sa.DateTime(timezone=True), server_default=sa.func.now()),
    )


def downgrade() -> None:
    op.drop_table('producer_projects')
