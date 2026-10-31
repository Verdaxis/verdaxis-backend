-- Disposable schema for the P02–P04 component cases (Task 17b).
--
-- Column names, types and nullability are the production shape of the facts
-- these three reads touch. Only the columns the projections read are declared;
-- no constraint or index here is a claim about the deployed schema.
CREATE TABLE organizations (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    domain text UNIQUE,
    type text NOT NULL,
    supplier_tier text,
    tax_id text,
    country_code varchar(2),
    verification_status text NOT NULL DEFAULT 'PENDING',
    provenance text NOT NULL DEFAULT 'UNKNOWN',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id uuid PRIMARY KEY,
    email text NOT NULL UNIQUE,
    password_hash text NOT NULL DEFAULT '',
    first_name text,
    last_name text,
    role text,
    status text NOT NULL DEFAULT 'PENDING',
    organization_id uuid REFERENCES organizations (id),
    last_login timestamptz,
    password_changed_at timestamptz,
    authentication_revoked_at timestamptz,
    must_change_password boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    email_verified boolean NOT NULL DEFAULT false
);

CREATE TABLE orderbook_orders (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id),
    side text NOT NULL,
    status text NOT NULL,
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE trades (
    id uuid PRIMARY KEY,
    buyer_id uuid NOT NULL REFERENCES organizations (id),
    seller_id uuid NOT NULL REFERENCES organizations (id),
    status text NOT NULL,
    quantity_mt numeric NOT NULL,
    final_total_usd numeric,
    commission_amount_usd numeric,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX ix_users_organization_id_id ON users (organization_id, id);
CREATE INDEX ix_orderbook_orders_organization_created ON orderbook_orders (organization_id, created_at);
CREATE INDEX ix_trades_buyer_created_at ON trades (buyer_id, created_at);
CREATE INDEX ix_trades_seller_created_at ON trades (seller_id, created_at);
