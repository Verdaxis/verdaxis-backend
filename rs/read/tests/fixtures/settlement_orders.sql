-- Test-only settlement fixture for the KYC review lane.
--
-- These are the production settlement tables that a KYC decision must leave
-- untouched. The DDL is the reviewed production shape (the same bounded
-- column set the Phase 4 market-invalidation fixture uses), so a
-- production-shaped statement would accept these rows.
--
-- The file lives outside the Rust literal sources and outside rs/read/sql,
-- which is exactly what the sealed order-book write-boundary scan reads. That
-- keeps the no-order-DML rule absolute for production code while still
-- letting a test seed real order-book rows. Do not move this seeding back
-- into a Rust string literal: the scanner refuses it by design.

CREATE TABLE orderbook_orders (
    id uuid PRIMARY KEY,
    acceptance_ordinal bigint,
    organization_id uuid NOT NULL,
    owner_user_id uuid,
    created_by_actor_user_id uuid,
    side text NOT NULL,
    product_id uuid,
    delivery_point_id uuid,
    availability_window text,
    status text NOT NULL DEFAULT 'OPEN',
    quantity_mt numeric NOT NULL CHECK (quantity_mt >= 0),
    remaining_quantity_mt numeric NOT NULL CHECK (
        remaining_quantity_mt >= 0 AND remaining_quantity_mt <= quantity_mt
    ),
    version integer NOT NULL DEFAULT 1 CHECK (version >= 1),
    inventory_item_id uuid,
    price_per_mt_usd numeric,
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT '2026-10-01Z',
    updated_at timestamptz,
    provenance text NOT NULL DEFAULT 'REAL',
    creation_method text NOT NULL DEFAULT 'DIRECT'
);
CREATE INDEX ON orderbook_orders (organization_id, status, id);

CREATE TABLE trades (
    id uuid PRIMARY KEY,
    bid_order_id uuid,
    ask_order_id uuid,
    buyer_id uuid NOT NULL,
    seller_id uuid NOT NULL,
    buyer_user_id uuid,
    seller_user_id uuid,
    initiator_org_id uuid NOT NULL,
    initiated_by text NOT NULL DEFAULT 'BUYER',
    is_anonymous boolean NOT NULL DEFAULT false,
    product_id uuid,
    delivery_point_id uuid,
    availability_window text,
    status text NOT NULL,
    quantity_mt numeric NOT NULL,
    price_per_mt_usd numeric NOT NULL,
    commission_rate_pct numeric NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT '2026-10-01Z'
);
CREATE INDEX ON trades (buyer_id, status, id);
CREATE INDEX ON trades (seller_id, status, id);

CREATE TABLE inventory_items (
    id uuid PRIMARY KEY,
    supplier_id uuid,
    owner_user_id uuid,
    port_id text,
    fuel_type text NOT NULL,
    current_stock_mt numeric NOT NULL CHECK (current_stock_mt >= 0),
    incoming_stock_mt numeric NOT NULL DEFAULT 0 CHECK (incoming_stock_mt >= 0),
    reserved_stock_mt numeric NOT NULL DEFAULT 0 CHECK (reserved_stock_mt >= 0),
    price_per_mt_usd numeric,
    updated_at timestamptz NOT NULL DEFAULT '2026-10-01Z'
);

-- Two open order-book rows in different states of the acceptance lifecycle.
INSERT INTO orderbook_orders (
    id, acceptance_ordinal, organization_id, side, status,
    quantity_mt, remaining_quantity_mt, version, price_per_mt_usd
) VALUES
    ('00000000-0000-4000-8000-0000000000a1', 1,
     '00000000-0000-4000-8000-0000000000c1', 'ASK', 'OPEN', 29, 29, 3, 210),
    ('00000000-0000-4000-8000-0000000000a2', 2,
     '00000000-0000-4000-8000-0000000000c2', 'BID', 'OPEN', 40, 25, 5, 195);

-- Every persisted trade state; a KYC decision must not withdraw or settle any.
INSERT INTO trades (
    id, buyer_id, seller_id, initiator_org_id, status, quantity_mt,
    price_per_mt_usd
) VALUES
    ('00000000-0000-4000-8000-0000000000b1', '00000000-0000-4000-8000-0000000000c1',
     '00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1',
     'PENDING_CONFIRMATION', 17, 200),
    ('00000000-0000-4000-8000-0000000000b2', '00000000-0000-4000-8000-0000000000c1',
     '00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1',
     'CONFIRMED', 17, 200),
    ('00000000-0000-4000-8000-0000000000b3', '00000000-0000-4000-8000-0000000000c1',
     '00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1',
     'DELIVERED', 17, 200),
    ('00000000-0000-4000-8000-0000000000b4', '00000000-0000-4000-8000-0000000000c1',
     '00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1',
     'PAID', 17, 200),
    ('00000000-0000-4000-8000-0000000000b5', '00000000-0000-4000-8000-0000000000c1',
     '00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1',
     'CANCELLED', 17, 200),
    ('00000000-0000-4000-8000-0000000000b6', '00000000-0000-4000-8000-0000000000c1',
     '00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000c1',
     'DECLINED', 17, 200);

-- One uncommitted reservation that no KYC decision may release.
INSERT INTO inventory_items (
    id, supplier_id, fuel_type, current_stock_mt, incoming_stock_mt,
    reserved_stock_mt, price_per_mt_usd
) VALUES
    ('00000000-0000-4000-8000-0000000000d1', '00000000-0000-4000-8000-0000000000c2',
     'Methanol', 100, 0, 17, 205);
