"""rename_quote_request_to_direct_order

Revision ID: 14b159f248ac
Revises: f10cf6fa2019
Create Date: 2026-02-02 14:21:06.702121

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision: str = '14b159f248ac'
down_revision: Union[str, Sequence[str], None] = 'f10cf6fa2019'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    """Upgrade schema."""
    # 1. Handle Foreign Keys first to avoid dependency issues during rename
    # Drop FK from traceability_events -> quote_requests
    op.drop_constraint('traceability_events_quote_id_fkey', 'traceability_events', type_='foreignkey')
    
    # Drop FK from quote_offers -> quote_requests
    op.drop_constraint('quote_offers_request_id_fkey', 'quote_offers', type_='foreignkey')

    # 2. Rename tables
    op.rename_table('quote_requests', 'direct_orders')
    op.rename_table('quote_offers', 'direct_order_offers')

    # 3. Update Columns and Foreign Keys for direct_order_offers
    # Rename request_id -> direct_order_id
    op.alter_column('direct_order_offers', 'request_id', new_column_name='direct_order_id')
    
    # Re-create FK to direct_orders
    op.create_foreign_key(None, 'direct_order_offers', 'direct_orders', ['direct_order_id'], ['id'])

    # 4. Update Columns and Foreign Keys for traceability_events
    # Rename quote_id -> direct_order_id
    op.alter_column('traceability_events', 'quote_id', new_column_name='direct_order_id')
    
    # Re-create FK to direct_orders
    op.create_foreign_key(None, 'traceability_events', 'direct_orders', ['direct_order_id'], ['id'])


def downgrade() -> None:
    """Downgrade schema."""
    # Reverse of upgrade
    
    # 1. Drop new FKs
    op.drop_constraint(None, 'traceability_events', type_='foreignkey')
    op.drop_constraint(None, 'direct_order_offers', type_='foreignkey')

    # 2. Rename columns back
    op.alter_column('traceability_events', 'direct_order_id', new_column_name='quote_id')
    op.alter_column('direct_order_offers', 'direct_order_id', new_column_name='request_id')

    # 3. Rename tables back
    op.rename_table('direct_order_offers', 'quote_offers')
    op.rename_table('direct_orders', 'quote_requests')

    # 4. Re-create original FKs
    op.create_foreign_key('quote_offers_request_id_fkey', 'quote_offers', 'quote_requests', ['request_id'], ['id'])
    op.create_foreign_key('traceability_events_quote_id_fkey', 'traceability_events', 'quote_requests', ['quote_id'], ['id'])

