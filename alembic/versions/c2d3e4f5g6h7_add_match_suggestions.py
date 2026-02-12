"""add match_suggestions table

Revision ID: c2d3e4f5g6h7
Revises: b1c2d3e4f5g6
Create Date: 2026-02-12 23:10:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID


# revision identifiers, used by Alembic.
revision: str = 'c2d3e4f5g6h7'
down_revision: Union[str, None] = 'b1c2d3e4f5g6'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        'match_suggestions',
        sa.Column('id', UUID(as_uuid=True), primary_key=True, server_default=sa.text('gen_random_uuid()')),
        sa.Column('bid_order_id', UUID(as_uuid=True), sa.ForeignKey('orderbook_orders.id'), nullable=False),
        sa.Column('ask_order_id', UUID(as_uuid=True), sa.ForeignKey('orderbook_orders.id'), nullable=False),
        sa.Column('score', sa.Numeric(5, 2), nullable=False),
        sa.Column('match_reasons', sa.JSON(), server_default='[]'),
        sa.Column('status', sa.String(20), server_default='SUGGESTED'),
        sa.Column('recipient_org_id', UUID(as_uuid=True), sa.ForeignKey('organizations.id'), nullable=False),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.func.now()),
    )


def downgrade() -> None:
    op.drop_table('match_suggestions')
