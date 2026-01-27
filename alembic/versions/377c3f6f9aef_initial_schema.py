"""initial_schema

Revision ID: 377c3f6f9aef
Revises: 
Create Date: 2024-01-27 14:52:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql
import geoalchemy2

# revision identifiers, used by Alembic.
revision: str = '377c3f6f9aef'
down_revision: Union[str, None] = None
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Extensions
    op.execute("CREATE EXTENSION IF NOT EXISTS postgis")

    # Enums
    op.execute("CREATE TYPE user_role AS ENUM ('BUYER', 'SUPPLIER', 'ADMIN')")
    op.execute("CREATE TYPE org_type AS ENUM ('SHIPPING_LINE', 'FUEL_SUPPLIER', 'PORT_AUTHORITY')")
    op.execute("CREATE TYPE congestion_level AS ENUM ('Low', 'Moderate', 'High')")
    op.execute("CREATE TYPE fuel_type AS ENUM ('Methanol', 'Biofuel', 'LNG', 'Ammonia', 'LSMGO')")
    op.execute("CREATE TYPE quote_status AS ENUM ('Draft', 'Pending', 'Quoted', 'Negotiating', 'Confirmed', 'Rejected', 'Completed')")

    # Organizations
    op.create_table('organizations',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('name', sa.String(), nullable=False),
        sa.Column('type', sa.Enum('SHIPPING_LINE', 'FUEL_SUPPLIER', 'PORT_AUTHORITY', name='org_type'), nullable=False),
        sa.Column('tax_id', sa.String(), nullable=True),
        sa.Column('country_code', sa.String(length=2), nullable=True),
        sa.Column('verification_status', sa.String(), server_default='PENDING', nullable=False),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.PrimaryKeyConstraint('id')
    )

    # Users
    op.create_table('users',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('email', sa.String(), nullable=False),
        sa.Column('password_hash', sa.String(), nullable=False),
        sa.Column('first_name', sa.String(), nullable=True),
        sa.Column('last_name', sa.String(), nullable=True),
        sa.Column('role', sa.Enum('BUYER', 'SUPPLIER', 'ADMIN', name='user_role'), nullable=False),
        sa.Column('organization_id', sa.UUID(), nullable=True),
        sa.Column('last_login', sa.DateTime(timezone=True), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.ForeignKeyConstraint(['organization_id'], ['organizations.id'], ),
        sa.PrimaryKeyConstraint('id'),
        sa.UniqueConstraint('email')
    )

    # Ports
    op.create_table('ports',
        sa.Column('id', sa.String(), nullable=False),
        sa.Column('name', sa.String(), nullable=False),
        sa.Column('country', sa.String(), nullable=False),
        sa.Column('location', geoalchemy2.types.Geography(geometry_type='POINT', srid=4326, from_text='ST_GeogFromText', name='geography'), nullable=True),
        sa.Column('timezone', sa.String(), nullable=True),
        sa.Column('is_active', sa.Boolean(), server_default='true', nullable=False),
        sa.PrimaryKeyConstraint('id')
    )
    # Port Intelligence
    op.create_table('port_intelligence',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('port_id', sa.String(), nullable=False),
        sa.Column('congestion_level', sa.Enum('Low', 'Moderate', 'High', name='congestion_level'), nullable=True),
        sa.Column('methanol_price_avg', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('biofuel_price_avg', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('captured_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.ForeignKeyConstraint(['port_id'], ['ports.id'], ),
        sa.PrimaryKeyConstraint('id')
    )

    # Vessels
    op.create_table('vessels',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('organization_id', sa.UUID(), nullable=True),
        sa.Column('name', sa.String(), nullable=False),
        sa.Column('imo_number', sa.String(), nullable=False),
        sa.Column('vessel_type', sa.String(), nullable=True),
        sa.Column('flag_state', sa.String(), nullable=True),
        sa.Column('dwt', sa.Numeric(), nullable=True),
        sa.Column('cii_rating', sa.String(length=1), nullable=True),
        sa.Column('eu_ets_status', sa.String(), nullable=True),
        sa.Column('fueleu_status', sa.String(), nullable=True),
        sa.Column('current_location', geoalchemy2.types.Geography(geometry_type='POINT', srid=4326, from_text='ST_GeogFromText', name='geography'), nullable=True),
        sa.Column('updated_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.ForeignKeyConstraint(['organization_id'], ['organizations.id'], ),
        sa.PrimaryKeyConstraint('id'),
        sa.UniqueConstraint('imo_number')
    )

    # Inventory Items
    op.create_table('inventory_items',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('supplier_id', sa.UUID(), nullable=True),
        sa.Column('port_id', sa.String(), nullable=True),
        sa.Column('fuel_type', sa.Enum('Methanol', 'Biofuel', 'LNG', 'Ammonia', 'LSMGO', name='fuel_type'), nullable=False),
        sa.Column('product_name', sa.String(), nullable=True),
        sa.Column('current_stock_mt', sa.Numeric(precision=10, scale=2), nullable=False),
        sa.Column('incoming_stock_mt', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('reserved_stock_mt', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('price_per_mt_usd', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('energy_density_mj_kg', sa.Numeric(precision=5, scale=2), nullable=True),
        sa.Column('is_certified', sa.Boolean(), server_default='false', nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.ForeignKeyConstraint(['port_id'], ['ports.id'], ),
        sa.ForeignKeyConstraint(['supplier_id'], ['organizations.id'], ),
        sa.PrimaryKeyConstraint('id')
    )

    # Quote Requests
    op.create_table('quote_requests',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('buyer_id', sa.UUID(), nullable=True),
        sa.Column('vessel_id', sa.UUID(), nullable=True),
        sa.Column('port_id', sa.String(), nullable=True),
        sa.Column('fuel_type', sa.Enum('Methanol', 'Biofuel', 'LNG', 'Ammonia', 'LSMGO', name='fuel_type'), nullable=False),
        sa.Column('quantity_mt', sa.Numeric(precision=10, scale=2), nullable=False),
        sa.Column('delivery_window_start', sa.Date(), nullable=True),
        sa.Column('delivery_window_end', sa.Date(), nullable=True),
        sa.Column('status', sa.Enum('Draft', 'Pending', 'Quoted', 'Negotiating', 'Confirmed', 'Rejected', 'Completed', name='quote_status'), nullable=False),
        sa.Column('awarded_supplier_id', sa.UUID(), nullable=True),
        sa.Column('final_price_usd', sa.Numeric(precision=12, scale=2), nullable=True),
        sa.Column('final_price_per_mt', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.ForeignKeyConstraint(['awarded_supplier_id'], ['organizations.id'], ),
        sa.ForeignKeyConstraint(['buyer_id'], ['organizations.id'], ),
        sa.ForeignKeyConstraint(['port_id'], ['ports.id'], ),
        sa.ForeignKeyConstraint(['vessel_id'], ['vessels.id'], ),
        sa.PrimaryKeyConstraint('id')
    )

    # Compliance & Traceability
    op.create_table('traceability_events',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('quote_id', sa.UUID(), nullable=True),
        sa.Column('stage', sa.String(), nullable=False),
        sa.Column('location_name', sa.String(), nullable=True),
        sa.Column('timestamp', sa.DateTime(timezone=True), nullable=True),
        sa.Column('verification_type', sa.String(), nullable=True),
        sa.Column('verification_doc_url', sa.String(), nullable=True),
        sa.Column('verification_hash', sa.String(), nullable=True),
        sa.Column('is_verified', sa.Boolean(), server_default='false', nullable=False),
        sa.ForeignKeyConstraint(['quote_id'], ['quote_requests.id'], ),
        sa.PrimaryKeyConstraint('id')
    )

    op.create_table('compliance_ledger',
        sa.Column('id', sa.UUID(), server_default=sa.text('gen_random_uuid()'), nullable=False),
        sa.Column('organization_id', sa.UUID(), nullable=True),
        sa.Column('transaction_type', sa.String(), nullable=True),
        sa.Column('amount', sa.Numeric(precision=12, scale=2), nullable=True),
        sa.Column('currency', sa.String(), server_default='EUR', nullable=False),
        sa.Column('units', sa.Numeric(precision=10, scale=2), nullable=True),
        sa.Column('description', sa.String(), nullable=True),
        sa.Column('reference_id', sa.String(), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.text('now()'), nullable=False),
        sa.ForeignKeyConstraint(['organization_id'], ['organizations.id'], ),
        sa.PrimaryKeyConstraint('id')
    )


def downgrade() -> None:
    op.drop_table('compliance_ledger')
    op.drop_table('traceability_events')
    op.drop_table('quote_requests')
    op.drop_table('inventory_items')
    op.drop_table('vessels')
    op.drop_table('port_intelligence')
    op.drop_table('ports')
    op.drop_table('users')
    op.drop_table('organizations')
    
    op.execute("DROP TYPE quote_status")
    op.execute("DROP TYPE fuel_type")
    op.execute("DROP TYPE congestion_level")
    op.execute("DROP TYPE org_type")
    op.execute("DROP TYPE user_role")
