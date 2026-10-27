-- Helpers are installed only in the migrated disposable test database.
CREATE OR REPLACE FUNCTION matching_fixture_seed(aggressor uuid, n integer, qty numeric)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    buyer uuid := '00000000-0000-0000-0000-000000001000';
    seller uuid := '00000000-0000-0000-0000-000000002000';
    buyer_user uuid := '00000000-0000-0000-0000-000000001001';
    seller_user uuid := '00000000-0000-0000-0000-000000002001';
    product uuid := 'c4cecebc-3d2d-5840-8021-57a9a11bc673';
    point uuid;
    candidate uuid;
    i integer;
BEGIN
    TRUNCATE organizations, watchlists, market_event_outbox, live_slice_benchmarks, audit_logs CASCADE;
    DROP TRIGGER IF EXISTS matching_fixture_trade_check ON trades;
    SELECT id INTO STRICT point FROM delivery_points WHERE name='Rotterdam';
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(buyer,'Matching buyer','SHIPOWNER','APPROVED','REAL'),
              (seller,'Matching seller','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
        VALUES(buyer_user,'matching-buyer@example.test','unused','BUYER','APPROVED',buyer,true,false,'APPROVED',buyer),
              (seller_user,'matching-seller@example.test','unused','SUPPLIER','APPROVED',seller,true,false,'APPROVED',seller);
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
    VALUES(aggressor,buyer,buyer_user,buyer_user,'SELF_SERVICE','REAL','BID',product,point,qty,qty,110,'SPOT','OPEN',
        false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL');
    FOR i IN 1..n LOOP
        candidate := ('00000000-0000-0000-0000-' || lpad(to_hex(i+1),12,'0'))::uuid;
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
            product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
            off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
            carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,created_at)
        VALUES(candidate,seller,seller_user,seller_user,'SELF_SERVICE','REAL','ASK',product,point,1,1,100,'SPOT','OPEN',
            false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL',
            '2026-10-24T12:00:00Z'::timestamptz - i*interval '1 second');
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_unavailable(expired uuid, cancelled uuid, at_time timestamptz)
RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE orderbook_orders SET expires_at=at_time WHERE id=expired;
    UPDATE orderbook_orders SET status='CANCELLED' WHERE id=cancelled;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_priority(first_id uuid, second_id uuid, third_id uuid)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE ordinal bigint;
BEGIN
    SELECT nextval('orderbook_acceptance_ordinal_seq') INTO ordinal;
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal+3 WHERE id=first_id;
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal+2 WHERE id=second_id;
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal+1 WHERE id=third_id;
    PERFORM setval('orderbook_acceptance_ordinal_seq',ordinal+3);
    UPDATE orderbook_orders SET created_at='2026-10-24T12:00:00Z'::timestamptz WHERE id=first_id;
    UPDATE orderbook_orders SET created_at='2026-10-24T12:00:01Z'::timestamptz WHERE id=second_id;
    UPDATE orderbook_orders SET created_at='2026-10-24T12:00:02Z'::timestamptz WHERE id=third_id;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_reject_trade(resting uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    EXECUTE 'CREATE OR REPLACE FUNCTION matching_fixture_trade_check() RETURNS trigger LANGUAGE plpgsql AS $body$ BEGIN IF NEW.ask_order_id=' || quote_literal(resting) || '::uuid THEN RAISE EXCEPTION ''fixture trade refusal'' USING ERRCODE=''23514''; END IF; RETURN NEW; END $body$';
    CREATE TRIGGER matching_fixture_trade_check BEFORE INSERT ON trades FOR EACH ROW EXECUTE FUNCTION matching_fixture_trade_check();
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_back_inventory(ord uuid,item uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source orderbook_orders%ROWTYPE;
BEGIN
    SELECT * INTO STRICT source FROM orderbook_orders WHERE id=ord;
    INSERT INTO inventory_items(id,supplier_id,owner_user_id,fuel_type,current_stock_mt,reserved_stock_mt,incoming_stock_mt,updated_at)
        SELECT item,organization_id,owner_user_id,'Biofuel',0,remaining_quantity_mt,0,'2026-10-24T12:00:00Z' FROM orderbook_orders WHERE id=ord;
    DELETE FROM orderbook_orders WHERE id=ord;
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,inventory_item_id)
    VALUES(source.id,source.organization_id,source.owner_user_id,source.created_by_actor_user_id,source.creation_method,source.provenance,source.side,
        source.product_id,source.delivery_point_id,source.quantity_mt,source.remaining_quantity_mt,source.price_per_mt_usd,source.availability_window,source.status,
        source.off_spec,source.certification_declared,source.certification_scheme,source.specification_standard,source.msds_available,
        source.carbon_intensity_gco2_mj,source.carbon_intensity_method,source.feedstock,source.origin,item);
END $$;
CREATE OR REPLACE FUNCTION matching_fixture_second_aggressor(source_id uuid,new_id uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
        SELECT new_id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin FROM orderbook_orders WHERE id=source_id;
END $$;

-- Eligibility helpers. Times are relative to the tests' fixed clock,
-- 2026-10-25T12:00:00Z.
CREATE OR REPLACE FUNCTION matching_fixture_other_organization() RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE other uuid := '00000000-0000-0000-0000-000000003000';
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(other,'Matching other','SHIPOWNER','APPROVED','REAL') ON CONFLICT (id) DO NOTHING;
    RETURN other;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_party(target uuid, change text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE home uuid;
BEGIN
    SELECT organization_id INTO home FROM users WHERE id=target;
    CASE change
    WHEN 'role_admin' THEN UPDATE users SET role='ADMIN' WHERE id=target;
    WHEN 'pending' THEN UPDATE users SET status='PENDING' WHERE id=target;
    WHEN 'rejected' THEN UPDATE users SET status='REJECTED' WHERE id=target;
    WHEN 'email_unverified' THEN UPDATE users SET email_verified=false WHERE id=target;
    WHEN 'password_change' THEN UPDATE users SET must_change_password=true WHERE id=target;
    WHEN 'moved' THEN UPDATE users SET organization_id=matching_fixture_other_organization() WHERE id=target;
    WHEN 'absent' THEN UPDATE users SET organization_id=NULL WHERE id=target;
    WHEN 'organization_pending' THEN UPDATE organizations SET verification_status='PENDING' WHERE id=home;
    WHEN 'kyc_pending' THEN UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id=target;
    WHEN 'kyc_rejected' THEN UPDATE users SET kyc_status='REJECTED' WHERE id=target;
    WHEN 'kyc_other_organization' THEN
        UPDATE users SET kyc_organization_id=matching_fixture_other_organization() WHERE id=target;
    WHEN 'grace' THEN
        INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by)
            VALUES(gen_random_uuid(),target,home,'PENDING','2026-10-01T00:00:00Z','2026-10-25T13:00:00Z','fixture');
    WHEN 'grace_expired' THEN UPDATE kyc_execution_grace SET expires_at='2026-10-25T11:00:00Z' WHERE user_id=target;
    ELSE RAISE EXCEPTION 'unknown party change %', change;
    END CASE;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_admin(admin uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    -- Staff KYC is irrelevant to assisted execution, so it is left rejected.
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status)
        VALUES(admin,'matching-admin-' || admin || '@example.test','unused','ADMIN','APPROVED',NULL,true,false,'REJECTED')
        ON CONFLICT (id) DO NOTHING;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_assist(target uuid, admin uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    PERFORM matching_fixture_admin(admin);
    UPDATE orderbook_orders SET owner_user_id=admin,created_by_actor_user_id=admin,creation_method='MARKET_SUPPORT'
        WHERE id=target;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_member(organization uuid, member uuid, member_role text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
        VALUES(member,'matching-member-' || member || '@example.test','unused',member_role,'APPROVED',organization,true,false,'APPROVED',organization);
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_order(target uuid, change text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    CASE change
    WHEN 'off_spec' THEN UPDATE orderbook_orders SET off_spec=true WHERE id=target;
    WHEN 'scheme_redcert' THEN UPDATE orderbook_orders SET certification_scheme='REDcert-EU' WHERE id=target;
    WHEN 'ownerless' THEN
        UPDATE orderbook_orders SET owner_user_id=NULL,created_by_actor_user_id=NULL,creation_method='LEGACY_UNKNOWN'
            WHERE id=target;
    ELSE RAISE EXCEPTION 'unknown order change %', change;
    END CASE;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_price(target uuid, price numeric) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE orderbook_orders SET price_per_mt_usd=price WHERE id=target;
END $$;

-- The demo activity pair with an unpaired order of the paired seller and a
-- DEMO order of another demo organization. Synthetic orders have no owner.
CREATE OR REPLACE FUNCTION matching_fixture_demo(bid uuid, ask uuid, unpaired uuid, foreign_ask uuid)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    buyer uuid := '7cc77115-0a9f-4ec4-8c74-05aa10050111';
    seller uuid := 'd1e43e55-3fb0-4b5e-9f0b-93aa10050222';
    foreign_org uuid := '4da7b285-34ee-5443-9406-f96b4ed1a251';
    product uuid := 'c4cecebc-3d2d-5840-8021-57a9a11bc673';
    point uuid;
BEGIN
    SELECT id INTO STRICT point FROM delivery_points WHERE name='Rotterdam';
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(buyer,'Demo activity buyer','SHIPOWNER','APPROVED','DEMO'),
              (seller,'Demo activity seller','FUEL_SUPPLIER','APPROVED','DEMO'),
              (foreign_org,'Demo buyer','FUEL_SUPPLIER','APPROVED','DEMO');
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,expires_at)
    SELECT row_id,organization,NULL::uuid,NULL::uuid,'SYSTEM','DEMO',side,product,point,1,1,price,'SPOT','OPEN',
        false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL','2026-10-26T12:00:00Z'::timestamptz
    FROM (VALUES (bid,buyer,'BID',110),(ask,seller,'ASK',100),(unpaired,seller,'ASK',99),(foreign_ask,foreign_org,'ASK',98))
        AS demo(row_id,organization,side,price);
END $$;
