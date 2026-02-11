"""Unified orderbook migration - create orderbook_orders and trades tables

Revision ID: a1b2c3d4e5f6
Revises: e3b25629c526
Create Date: 2026-02-11 00:00:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID


# revision identifiers, used by Alembic.
revision: str = 'a1b2c3d4e5f6'
down_revision: Union[str, Sequence[str], None] = 'e3b25629c526'
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # ========== 1. Create orderbook_orders table ==========
    op.create_table(
        'orderbook_orders',
        sa.Column('id', UUID(as_uuid=True), primary_key=True, server_default=sa.text('gen_random_uuid()')),
        sa.Column('organization_id', UUID(as_uuid=True), sa.ForeignKey('organizations.id'), nullable=False),
        sa.Column('side', sa.String(10), nullable=False),  # BID or ASK
        sa.Column('fuel_type', sa.String(50), nullable=False),
        sa.Column('fuel_grade', sa.String(50), server_default='Conventional'),
        sa.Column('region', sa.String(50), nullable=False),
        sa.Column('port_id', sa.String(50), nullable=True),
        sa.Column('vessel_id', UUID(as_uuid=True), sa.ForeignKey('vessels.id'), nullable=True),
        sa.Column('quantity_mt', sa.Numeric(12, 2), nullable=False),
        sa.Column('remaining_quantity_mt', sa.Numeric(12, 2), nullable=False),
        sa.Column('price_per_mt_usd', sa.Numeric(10, 2), nullable=False),
        sa.Column('availability_window', sa.String(50), server_default='Spot'),
        sa.Column('delivery_window_start', sa.Date, nullable=True),
        sa.Column('delivery_window_end', sa.Date, nullable=True),
        sa.Column('certifications', sa.JSON, server_default='[]'),
        sa.Column('is_verdaxis_verified', sa.Boolean, server_default='false'),
        sa.Column('status', sa.String(20), server_default='OPEN'),
        sa.Column('expires_at', sa.DateTime(timezone=True), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.func.now()),
        sa.Column('updated_at', sa.DateTime(timezone=True), server_default=sa.func.now()),
    )
    op.create_index('ix_orderbook_orders_side', 'orderbook_orders', ['side'])
    op.create_index('ix_orderbook_orders_status', 'orderbook_orders', ['status'])
    op.create_index('ix_orderbook_orders_region', 'orderbook_orders', ['region'])
    op.create_index('ix_orderbook_orders_fuel_type', 'orderbook_orders', ['fuel_type'])
    op.create_index('ix_orderbook_orders_org', 'orderbook_orders', ['organization_id'])

    # ========== 2. Create trades table ==========
    op.create_table(
        'trades',
        sa.Column('id', UUID(as_uuid=True), primary_key=True, server_default=sa.text('gen_random_uuid()')),
        sa.Column('bid_order_id', UUID(as_uuid=True), sa.ForeignKey('orderbook_orders.id'), nullable=True),
        sa.Column('ask_order_id', UUID(as_uuid=True), sa.ForeignKey('orderbook_orders.id'), nullable=True),
        sa.Column('buyer_id', UUID(as_uuid=True), sa.ForeignKey('organizations.id'), nullable=False),
        sa.Column('seller_id', UUID(as_uuid=True), sa.ForeignKey('organizations.id'), nullable=False),
        sa.Column('initiated_by', sa.String(10), nullable=False),  # BUYER or SELLER
        sa.Column('quantity_mt', sa.Numeric(12, 2), nullable=False),
        sa.Column('price_per_mt_usd', sa.Numeric(10, 2), nullable=False),
        sa.Column('status', sa.String(30), server_default='PENDING_CONFIRMATION'),
        sa.Column('final_quantity_mt', sa.Numeric(12, 2), nullable=True),
        sa.Column('final_price_per_mt', sa.Numeric(10, 2), nullable=True),
        sa.Column('final_total_usd', sa.Numeric(14, 2), nullable=True),
        sa.Column('commission_rate_pct', sa.Numeric(5, 3), server_default='0.5'),
        sa.Column('commission_amount_usd', sa.Numeric(12, 2), nullable=True),
        sa.Column('confirmed_at', sa.DateTime(timezone=True), nullable=True),
        sa.Column('delivered_at', sa.DateTime(timezone=True), nullable=True),
        sa.Column('paid_at', sa.DateTime(timezone=True), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.func.now()),
    )
    op.create_index('ix_trades_buyer', 'trades', ['buyer_id'])
    op.create_index('ix_trades_seller', 'trades', ['seller_id'])
    op.create_index('ix_trades_status', 'trades', ['status'])

    # ========== 3. Add trade_id FK to commissions table ==========
    op.add_column('commissions', sa.Column('trade_id', UUID(as_uuid=True), nullable=True))
    op.create_foreign_key('fk_commissions_trade_id', 'commissions', 'trades', ['trade_id'], ['id'])

    # ========== 4. Migrate public_listings → orderbook_orders (side=ASK) ==========
    op.execute("""
        INSERT INTO orderbook_orders (
            id, organization_id, side, fuel_type, fuel_grade, region,
            quantity_mt, remaining_quantity_mt, price_per_mt_usd,
            availability_window, certifications, is_verdaxis_verified,
            status, expires_at, created_at, updated_at
        )
        SELECT
            id, supplier_id, 'ASK', fuel_type, fuel_grade, region,
            quantity_mt, quantity_mt, price_per_mt_usd,
            availability_window, certifications, is_verdaxis_verified,
            CASE
                WHEN status = 'ACTIVE' THEN 'OPEN'
                WHEN status = 'INACTIVE' THEN 'CANCELLED'
                WHEN status = 'EXPIRED' THEN 'EXPIRED'
                ELSE 'CANCELLED'
            END,
            expires_at, created_at, updated_at
        FROM public_listings
    """)

    # ========== 5. Migrate direct_orders → orderbook_orders (side=BID) ==========
    # DirectOrder has no price_per_mt, so we use final_price_per_mt or 0
    op.execute("""
        INSERT INTO orderbook_orders (
            id, organization_id, side, fuel_type, fuel_grade, region,
            port_id, vessel_id, quantity_mt, remaining_quantity_mt,
            price_per_mt_usd, delivery_window_start, delivery_window_end,
            status, created_at, updated_at
        )
        SELECT
            id, buyer_id, 'BID', fuel_type, 'Conventional',
            COALESCE(port_id, 'Unknown'),
            port_id, vessel_id, quantity_mt, quantity_mt,
            COALESCE(final_price_per_mt, 0),
            delivery_window_start, delivery_window_end,
            CASE
                WHEN status = 'Pending' THEN 'OPEN'
                WHEN status = 'Draft' THEN 'OPEN'
                WHEN status = 'Quoted' THEN 'OPEN'
                WHEN status = 'Negotiating' THEN 'OPEN'
                WHEN status = 'Confirmed' THEN 'FILLED'
                WHEN status = 'Completed' THEN 'FILLED'
                WHEN status = 'Rejected' THEN 'CANCELLED'
                ELSE 'CANCELLED'
            END,
            created_at, updated_at
        FROM direct_orders
    """)

    # ========== 6. Migrate legacy orders → trades ==========
    # Legacy orders table: buyer hits a public_listing (ASK), so it's a buyer-initiated trade
    # We need the listing's supplier_id as seller_id
    op.execute("""
        INSERT INTO trades (
            id, ask_order_id, buyer_id, seller_id, initiated_by,
            quantity_mt, price_per_mt_usd, status,
            final_quantity_mt, final_price_per_mt, final_total_usd,
            commission_rate_pct, commission_amount_usd,
            confirmed_at, delivered_at, paid_at, created_at
        )
        SELECT
            o.id,
            o.listing_id,  -- listing_id maps to ASK order in orderbook_orders (same UUID)
            o.buyer_id,
            pl.supplier_id,
            'BUYER',
            COALESCE(o.requested_quantity_mt, pl.quantity_mt),
            pl.price_per_mt_usd,
            CASE
                WHEN o.status = 'PENDING' THEN 'PENDING_CONFIRMATION'
                WHEN o.status = 'CONFIRMED' THEN 'CONFIRMED'
                WHEN o.status = 'DECLINED' THEN 'DECLINED'
                WHEN o.status = 'DELIVERED' THEN 'DELIVERED'
                WHEN o.status = 'COMPLETED' THEN 'DELIVERED'
                WHEN o.status = 'PAID' THEN 'PAID'
                WHEN o.status = 'CANCELLED' THEN 'CANCELLED'
                ELSE 'CANCELLED'
            END,
            o.final_quantity_mt,
            o.final_price_per_mt,
            o.final_total_usd,
            o.commission_rate_pct,
            o.commission_amount_usd,
            o.supplier_responded_at,
            o.completed_at,
            CASE WHEN o.status = 'PAID' THEN o.completed_at ELSE NULL END,
            o.created_at
        FROM orders o
        JOIN public_listings pl ON o.listing_id = pl.id
    """)

    # ========== 7. Migrate accepted direct_order_offers → trades ==========
    # Supplier hits a buyer's BID, so it's a seller-initiated trade
    op.execute("""
        INSERT INTO trades (
            bid_order_id, buyer_id, seller_id, initiated_by,
            quantity_mt, price_per_mt_usd, status,
            created_at
        )
        SELECT
            doo.direct_order_id,  -- direct_order_id maps to BID order in orderbook_orders (same UUID)
            dord.buyer_id,
            doo.supplier_id,
            'SELLER',
            dord.quantity_mt,
            doo.price_per_mt_usd,
            CASE WHEN doo.is_accepted THEN 'CONFIRMED' ELSE 'PENDING_CONFIRMATION' END,
            doo.created_at
        FROM direct_order_offers doo
        JOIN direct_orders dord ON doo.direct_order_id = dord.id
        WHERE doo.is_accepted = true
    """)

    # ========== 8. Update commissions.trade_id from commissions.match_id ==========
    # match_id currently points to old orders table; those orders now exist as trades with same UUID
    op.execute("""
        UPDATE commissions
        SET trade_id = match_id
        WHERE match_id IN (SELECT id FROM trades)
    """)

    # ========== 9. Update remaining_quantity_mt for ASK orders that had confirmed trades ==========
    op.execute("""
        UPDATE orderbook_orders oo
        SET remaining_quantity_mt = oo.quantity_mt - COALESCE(
            (SELECT SUM(t.quantity_mt)
             FROM trades t
             WHERE t.ask_order_id = oo.id
             AND t.status NOT IN ('CANCELLED', 'DECLINED')),
            0
        )
        WHERE oo.side = 'ASK'
    """)

    # Update status for partially/fully filled ASK orders
    op.execute("""
        UPDATE orderbook_orders
        SET status = 'FILLED'
        WHERE side = 'ASK' AND remaining_quantity_mt <= 0 AND status = 'OPEN'
    """)
    op.execute("""
        UPDATE orderbook_orders
        SET status = 'PARTIALLY_FILLED'
        WHERE side = 'ASK' AND remaining_quantity_mt > 0
        AND remaining_quantity_mt < quantity_mt AND status = 'OPEN'
    """)


def downgrade() -> None:
    # Remove trade_id FK and column from commissions
    op.drop_constraint('fk_commissions_trade_id', 'commissions', type_='foreignkey')
    op.drop_column('commissions', 'trade_id')

    # Drop tables (data migration is one-way)
    op.drop_index('ix_trades_status', 'trades')
    op.drop_index('ix_trades_seller', 'trades')
    op.drop_index('ix_trades_buyer', 'trades')
    op.drop_table('trades')

    op.drop_index('ix_orderbook_orders_org', 'orderbook_orders')
    op.drop_index('ix_orderbook_orders_fuel_type', 'orderbook_orders')
    op.drop_index('ix_orderbook_orders_region', 'orderbook_orders')
    op.drop_index('ix_orderbook_orders_status', 'orderbook_orders')
    op.drop_index('ix_orderbook_orders_side', 'orderbook_orders')
    op.drop_table('orderbook_orders')
