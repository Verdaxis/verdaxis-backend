-- Test fixture: the disposable schema of the integrated race suite.
--
-- Table and column names, types and constraints follow the production tables
-- that the Phase 4 business commands read and write, so that one schema serves
-- every command (account admission, organization and join review, KYC
-- submission and review, invitations, subscriptions and the market
-- invalidation they call). The role grants are applied by the suite after this
-- file and mirror deploy/postgres/app_acl_policy.sql.
--
-- The file lives outside the Rust literal sources, which is where the order-book
-- write-boundary scan reads: the suite seeds order-book rows from fixture files,
-- never from a Rust literal.

CREATE TABLE organizations(
    id uuid PRIMARY KEY,
    name text NOT NULL DEFAULT 'Synthetic Organization',
    domain text UNIQUE,
    type text NOT NULL DEFAULT 'FUEL_BUYER',
    verification_status text NOT NULL DEFAULT 'PENDING_REVIEW',
    provenance text NOT NULL DEFAULT 'UNKNOWN',
    tax_id text,
    country_code text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users(
    id uuid PRIMARY KEY,
    email text NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text || '@example.test'),
    password_hash text NOT NULL DEFAULT 'synthetic',
    first_name text,
    last_name text,
    role text,
    status text NOT NULL DEFAULT 'APPROVED',
    organization_id uuid REFERENCES organizations(id),
    email_verified boolean NOT NULL DEFAULT true,
    must_change_password boolean NOT NULL DEFAULT false,
    password_changed_at timestamptz,
    authentication_revoked_at timestamptz,
    email_verification_token_hash text,
    email_verification_token_expires_at timestamptz,
    referral_code text UNIQUE,
    referred_by_id uuid,
    password_reset_token_hash text,
    password_reset_expires timestamptz,
    last_login timestamptz,
    kyc_status text NOT NULL DEFAULT 'PENDING',
    kyc_organization_id uuid,
    kyc_rejection_reason text,
    kyc_external_evidence_reference text,
    kyc_review_note text,
    kyc_reviewed_by uuid,
    kyc_reviewed_at timestamptz,
    pending_approval_email_transition_id uuid,
    pending_approval_email_payload jsonb,
    pending_approval_email_retry_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE organization_join_requests(
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    organization_id uuid NOT NULL REFERENCES organizations(id),
    status text NOT NULL DEFAULT 'PENDING',
    reviewed_by uuid,
    reviewed_at timestamptz,
    review_note text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(user_id, organization_id)
);

CREATE TABLE audit_logs(
    id uuid PRIMARY KEY,
    user_id uuid,
    action text,
    resource_type text,
    resource_id text,
    changes jsonb,
    ip_address text,
    request_id text,
    timestamp timestamptz
);

CREATE TABLE user_status_transitions(
    id uuid PRIMARY KEY,
    user_id uuid,
    organization_id uuid,
    role text,
    from_status text,
    to_status text,
    effective_at timestamptz,
    provenance text
);

CREATE TABLE refresh_sessions(
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id),
    family_id uuid NOT NULL,
    jti_hash text NOT NULL UNIQUE,
    device_id_hash text,
    replaced_by_jti_hash text,
    rotation_grace_until timestamptz,
    revoked boolean NOT NULL DEFAULT false,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE referrals(
    id uuid PRIMARY KEY,
    referrer_id uuid NOT NULL,
    referred_user_id uuid NOT NULL UNIQUE,
    referral_code_used text NOT NULL,
    status text NOT NULL,
    created_at timestamptz,
    verified_at timestamptz,
    activated_at timestamptz
);

CREATE TABLE kyc_documents(
    id uuid PRIMARY KEY,
    submission_id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    kind text NOT NULL,
    content_type text NOT NULL,
    size_bytes integer NOT NULL,
    key_id text NOT NULL,
    wrapped_key bytea NOT NULL,
    key_nonce bytea NOT NULL,
    data_nonce bytea NOT NULL,
    ciphertext bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE kyc_execution_grace(
    user_id uuid,
    organization_id uuid,
    expires_at timestamptz
);

CREATE TABLE subscriptions(
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL UNIQUE REFERENCES organizations(id),
    tier text NOT NULL,
    started_at timestamptz DEFAULT now(),
    expires_at timestamptz,
    is_active boolean NOT NULL,
    seller_fee_per_mt_usd numeric CHECK(seller_fee_per_mt_usd IS NULL OR (seller_fee_per_mt_usd >= 0 AND seller_fee_per_mt_usd <= 100000 AND seller_fee_per_mt_usd * 100 = trunc(seller_fee_per_mt_usd * 100)))
);

CREATE TABLE products(id uuid PRIMARY KEY, name text, fuel_type text, fuel_grade text, is_active boolean DEFAULT true);
CREATE TABLE delivery_points(id uuid PRIMARY KEY, name text, region text, is_active boolean DEFAULT true);

CREATE TABLE inventory_items(
    id uuid PRIMARY KEY,
    current_stock_mt numeric NOT NULL CHECK(current_stock_mt >= 0),
    reserved_stock_mt numeric NOT NULL CHECK(reserved_stock_mt >= 0)
);

CREATE TABLE orderbook_orders(
    id uuid PRIMARY KEY,
    organization_id uuid,
    side text,
    product_id uuid,
    delivery_point_id uuid,
    availability_window text,
    status text DEFAULT 'OPEN',
    quantity_mt numeric DEFAULT 100 CHECK(quantity_mt >= 0),
    remaining_quantity_mt numeric DEFAULT 60 CHECK(remaining_quantity_mt >= 0 AND remaining_quantity_mt <= quantity_mt),
    version int DEFAULT 1 CHECK(version >= 1),
    inventory_item_id uuid,
    price_per_mt_usd numeric DEFAULT 200,
    expires_at timestamptz,
    created_at timestamptz DEFAULT '2026-10-01Z',
    updated_at timestamptz,
    provenance text DEFAULT 'REAL',
    owner_user_id uuid,
    created_by_actor_user_id uuid,
    creation_method text DEFAULT 'DIRECT',
    off_spec boolean DEFAULT false,
    msds_available boolean DEFAULT true,
    certification_declared boolean DEFAULT true,
    certification_scheme text DEFAULT 'test',
    specification_standard text DEFAULT 'test',
    carbon_intensity_gco2_mj numeric DEFAULT 10,
    carbon_intensity_method text,
    energy_density_mj_kg numeric,
    feedstock text DEFAULT 'test',
    origin text DEFAULT 'test',
    fame_terms jsonb,
    lng_terms jsonb
);
CREATE INDEX ON orderbook_orders(organization_id, status, id);

CREATE TABLE trades(
    id uuid PRIMARY KEY,
    buyer_id uuid,
    seller_id uuid,
    bid_order_id uuid,
    ask_order_id uuid,
    product_id uuid,
    delivery_point_id uuid,
    availability_window text,
    status text,
    quantity_mt numeric,
    buyer_user_id uuid,
    seller_user_id uuid,
    fee_snapshot jsonb,
    terms_snapshot jsonb
);
CREATE INDEX ON trades(buyer_id, status, id);
CREATE INDEX ON trades(seller_id, status, id);

CREATE TABLE live_slice_benchmarks(
    id uuid PRIMARY KEY,
    side varchar(3),
    market_product varchar(64),
    delivery_point_id uuid NOT NULL,
    availability_window varchar(16),
    benchmark_price_per_mt_usd numeric(10,2),
    total_remaining_quantity_mt numeric(12,2),
    order_count int,
    source varchar(32),
    created_at timestamptz,
    updated_at timestamptz,
    CONSTRAINT uq_live_slice_benchmarks_slice_key UNIQUE(side, market_product, delivery_point_id, availability_window)
);

CREATE TABLE watchlist_targets(
    id uuid PRIMARY KEY,
    watchlist_id uuid,
    target_type text,
    order_id uuid,
    market_product_code text,
    delivery_point_id uuid,
    availability_window_code text,
    snapshot_price_per_mt_usd double precision,
    snapshot_quantity_mt double precision,
    snapshot_remaining_quantity_mt double precision,
    snapshot_status text,
    snapshot_side text,
    snapshot_market_product text,
    snapshot_delivery_point_name text,
    snapshot_availability_window text,
    snapshot_counterparty_label text
);
CREATE TABLE watchlist_events(
    id uuid PRIMARY KEY,
    watchlist_id uuid,
    watchlist_target_id uuid,
    event_type text,
    event_payload jsonb,
    is_read boolean DEFAULT false,
    created_at timestamptz
);

CREATE TABLE market_event_outbox(
    id uuid PRIMARY KEY,
    event_type text,
    aggregate_type text,
    aggregate_id text,
    participant_org_ids jsonb,
    payload jsonb,
    created_at timestamptz,
    dispatched_at timestamptz,
    delivery_attempts int DEFAULT 0,
    stream_seq bigint
);
