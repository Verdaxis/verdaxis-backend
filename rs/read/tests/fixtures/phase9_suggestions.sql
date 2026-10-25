-- section: schema
CREATE TABLE organizations(id uuid PRIMARY KEY, verification_status text DEFAULT 'APPROVED', provenance text DEFAULT 'REAL');
CREATE TABLE users(id uuid PRIMARY KEY, organization_id uuid REFERENCES organizations, role text DEFAULT 'SUPPLIER', status text DEFAULT 'APPROVED', email_verified boolean DEFAULT true, must_change_password boolean DEFAULT false, password_changed_at timestamptz, authentication_revoked_at timestamptz, kyc_status text DEFAULT 'APPROVED', kyc_organization_id uuid);
CREATE TABLE kyc_execution_grace(user_id uuid, organization_id uuid, expires_at timestamptz);
CREATE TABLE products(id uuid PRIMARY KEY, name text, fuel_type text, fuel_grade text, is_active boolean DEFAULT true);
CREATE TABLE delivery_points(id uuid PRIMARY KEY, name text, region text, is_active boolean DEFAULT true);
CREATE TABLE orderbook_orders(
 id uuid PRIMARY KEY, organization_id uuid REFERENCES organizations, owner_user_id uuid REFERENCES users,
 created_by_actor_user_id uuid, creation_method text DEFAULT 'SELF_SERVICE', product_id uuid REFERENCES products,
 delivery_point_id uuid REFERENCES delivery_points, side text DEFAULT 'ASK', provenance text DEFAULT 'REAL',
 status text DEFAULT 'OPEN', expires_at timestamptz, quantity_mt numeric DEFAULT 1000,
 remaining_quantity_mt numeric DEFAULT 1000, price_per_mt_usd numeric DEFAULT 500,
 availability_window text DEFAULT 'SPOT', certification_scheme text DEFAULT 'ISCC EU',
 certification_declared boolean DEFAULT true, specification_standard text DEFAULT 'declared standard',
 msds_available boolean DEFAULT true, carbon_intensity_gco2_mj numeric DEFAULT 20,
 carbon_intensity_method text DEFAULT 'lifecycle', feedstock text DEFAULT 'declared feedstock', origin text DEFAULT 'SG',
 off_spec boolean DEFAULT false, off_spec_notes text, fame_terms jsonb, lng_terms jsonb,
 energy_density_mj_kg numeric DEFAULT 20, certifications jsonb DEFAULT '[]', is_verdaxis_verified boolean DEFAULT false,
 tier_label text DEFAULT 'Declared', version integer DEFAULT 1, port_id text, created_at timestamptz DEFAULT now(), updated_at timestamptz DEFAULT now(),
 private_evidence text DEFAULT 'private-document-reference'
);
CREATE TABLE match_suggestions(id uuid PRIMARY KEY, bid_order_id uuid REFERENCES orderbook_orders, ask_order_id uuid REFERENCES orderbook_orders, score numeric NOT NULL, match_reasons json NOT NULL, status text NOT NULL, recipient_org_id uuid REFERENCES organizations, created_at timestamptz DEFAULT now());
CREATE TABLE audit_logs(id uuid PRIMARY KEY);
CREATE TABLE notifications(id uuid PRIMARY KEY);
CREATE TABLE market_event_outbox(id uuid PRIMARY KEY);
CREATE TABLE trades(id uuid PRIMARY KEY);
-- section: seed
WITH input AS (SELECT $1::jsonb AS j)
INSERT INTO organizations(id) SELECT value::uuid FROM input, jsonb_array_elements_text(j->'organizations');
-- section: users
INSERT INTO users(id,organization_id,role,kyc_organization_id)
SELECT id,organization_id,role,organization_id FROM jsonb_to_recordset($1::jsonb) AS u(id uuid,organization_id uuid,role text);
-- section: product
INSERT INTO products(id,name,fuel_type,fuel_grade) VALUES($1,$2,$3,$4);
-- section: point
INSERT INTO delivery_points(id,name,region) VALUES($1,$2,$3);
-- section: orders
INSERT INTO orderbook_orders(id,organization_id,owner_user_id,product_id,delivery_point_id,side,provenance,price_per_mt_usd,expires_at,specification_standard)
SELECT id,organization_id,owner_user_id,$2,$3,side,provenance,price,expires_at,$4 FROM jsonb_to_recordset($1::jsonb) AS o(id uuid,organization_id uuid,owner_user_id uuid,side text,provenance text,price numeric,expires_at timestamptz);
-- section: reject
UPDATE users SET status='REJECTED' WHERE id=$1;
-- section: incomplete
UPDATE orderbook_orders SET specification_standard=NULL WHERE id=$1;
-- section: role
UPDATE users SET role=$2 WHERE id=$1;
-- section: kyc
UPDATE users SET kyc_status=$2,kyc_organization_id=$3 WHERE id=$1;
-- section: duplicate
INSERT INTO match_suggestions(id,bid_order_id,ask_order_id,score,match_reasons,status,recipient_org_id) VALUES($1,$2,$2,0,'[]','DISMISSED',$3);
-- section: abort
CREATE FUNCTION refuse_suggestion() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'transaction stopped' USING ERRCODE='P0001'; END $$;
CREATE TRIGGER refuse_suggestion BEFORE INSERT ON match_suggestions FOR EACH ROW EXECUTE FUNCTION refuse_suggestion();
-- section: resume
DROP TRIGGER refuse_suggestion ON match_suggestions;
