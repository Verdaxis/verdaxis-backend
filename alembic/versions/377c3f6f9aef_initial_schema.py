"""initial_schema

Revision ID: 377c3f6f9aef
Revises: 
Create Date: 2024-01-27 14:52:00.000000

"""
from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision = '377c3f6f9aef'
down_revision = None
branch_labels = None
depends_on = None

def upgrade() -> None:
    # Use raw SQL to bypass SQLAlchemy's redundant ENUM creation
    op.execute(\"\"\"
    CREATE EXTENSION IF NOT EXISTS postgis;

    DO $$$$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'user_role') THEN
            CREATE TYPE user_role AS ENUM ('BUYER', 'SUPPLIER', 'ADMIN');
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'org_type') THEN
            CREATE TYPE org_type AS ENUM ('SHIPPING_LINE', 'FUEL_SUPPLIER', 'PORT_AUTHORITY');
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'congestion_level') THEN
            CREATE TYPE congestion_level AS ENUM ('Low', 'Moderate', 'High');
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'fuel_type') THEN
            CREATE TYPE fuel_type AS ENUM ('Methanol', 'Biofuel', 'LNG', 'Ammonia', 'LSMGO');
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'quote_status') THEN
            CREATE TYPE quote_status AS ENUM ('Draft', 'Pending', 'Quoted', 'Negotiating', 'Confirmed', 'Rejected', 'Completed');
        END IF;
    END $$$$;

    CREATE TABLE IF NOT EXISTS organizations (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        name VARCHAR NOT NULL,
        type org_type NOT NULL,
        tax_id VARCHAR,
        country_code VARCHAR(2),
        verification_status VARCHAR NOT NULL DEFAULT 'PENDING',
        created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS users (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        email VARCHAR UNIQUE NOT NULL,
        password_hash VARCHAR NOT NULL,
        first_name VARCHAR,
        last_name VARCHAR,
        role user_role NOT NULL,
        organization_id UUID REFERENCES organizations(id),
        last_login TIMESTAMP WITH TIME ZONE,
        created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS ports (
        id VARCHAR PRIMARY KEY,
        name VARCHAR NOT NULL,
        country VARCHAR NOT NULL,
        location geography(POINT, 4326),
        timezone VARCHAR,
        is_active BOOLEAN NOT NULL DEFAULT true
    );

    CREATE TABLE IF NOT EXISTS port_intelligence (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        port_id VARCHAR NOT NULL REFERENCES ports(id),
        congestion_level congestion_level,
        methanol_price_avg NUMERIC(10, 2),
        biofuel_price_avg NUMERIC(10, 2),
        captured_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS vessels (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        organization_id UUID REFERENCES organizations(id),
        name VARCHAR NOT NULL,
        imo_number VARCHAR UNIQUE NOT NULL,
        vessel_type VARCHAR,
        flag_state VARCHAR,
        dwt NUMERIC,
        cii_rating VARCHAR(1),
        eu_ets_status VARCHAR,
        fueleu_status VARCHAR,
        current_location geography(POINT, 4326),
        updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS inventory_items (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        supplier_id UUID REFERENCES organizations(id),
        port_id VARCHAR REFERENCES ports(id),
        fuel_type fuel_type NOT NULL,
        product_name VARCHAR,
        current_stock_mt NUMERIC(10, 2) NOT NULL,
        incoming_stock_mt NUMERIC(10, 2),
        reserved_stock_mt NUMERIC(10, 2),
        price_per_mt_usd NUMERIC(10, 2),
        energy_density_mj_kg NUMERIC(5, 2),
        is_certified BOOLEAN NOT NULL DEFAULT false,
        updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS quote_requests (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        buyer_id UUID REFERENCES organizations(id),
        vessel_id UUID REFERENCES vessels(id),
        port_id VARCHAR REFERENCES ports(id),
        fuel_type fuel_type NOT NULL,
        quantity_mt NUMERIC(10, 2) NOT NULL,
        delivery_window_start DATE,
        delivery_window_end DATE,
        status quote_status NOT NULL,
        awarded_supplier_id UUID REFERENCES organizations(id),
        final_price_usd NUMERIC(12, 2),
        final_price_per_mt NUMERIC(10, 2),
        created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
        updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS traceability_events (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        quote_id UUID REFERENCES quote_requests(id),
        stage VARCHAR NOT NULL,
        location_name VARCHAR,
        timestamp TIMESTAMP WITH TIME ZONE,
        verification_type VARCHAR,
        verification_doc_url VARCHAR,
        verification_hash VARCHAR,
        is_verified BOOLEAN NOT NULL DEFAULT false
    );

    CREATE TABLE IF NOT EXISTS compliance_ledger (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        organization_id UUID REFERENCES organizations(id),
        transaction_type VARCHAR,
        amount NUMERIC(12, 2),
        currency VARCHAR NOT NULL DEFAULT 'EUR',
        units NUMERIC(10, 2),
        description VARCHAR,
        reference_id VARCHAR,
        created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now()
    );
    \"\"\")

def downgrade() -> None:
    op.execute(\"\"\"
    DROP TABLE IF EXISTS compliance_ledger;
    DROP TABLE IF EXISTS traceability_events;
    DROP TABLE IF EXISTS quote_requests;
    DROP TABLE IF EXISTS inventory_items;
    DROP TABLE IF EXISTS vessels;
    DROP TABLE IF EXISTS port_intelligence;
    DROP TABLE IF EXISTS ports;
    DROP TABLE IF EXISTS users;
    DROP TABLE IF EXISTS organizations;
    
    DROP TYPE IF EXISTS quote_status;
    DROP TYPE IF EXISTS fuel_type;
    DROP TYPE IF EXISTS congestion_level;
    DROP TYPE IF EXISTS org_type;
    DROP TYPE IF EXISTS user_role;
    \"\"\")
