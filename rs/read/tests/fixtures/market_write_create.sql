-- Installed only by the disposable component suite's migrator.
CREATE OR REPLACE FUNCTION create_fixture_seed(n integer) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM matching_fixture_seed('00000000-0000-0000-0000-000000000001',n,200);
    DELETE FROM orderbook_orders WHERE id='00000000-0000-0000-0000-000000000001';
    PERFORM matching_fixture_admin('00000000-0000-0000-0000-000000003001');
    INSERT INTO staff_capability_assignments(id,user_id,capability,reason,granted_by_user_id,granted_at)
    VALUES ('00000000-0000-0000-0000-000000003101','00000000-0000-0000-0000-000000003001','MARKET_SUPPORT_LISTINGS','Fixture authority','00000000-0000-0000-0000-000000003001',now()),
           ('00000000-0000-0000-0000-000000003102','00000000-0000-0000-0000-000000003001','MARKET_SUPPORT_AUTHORIZATIONS','Fixture authority','00000000-0000-0000-0000-000000003001',now());
    INSERT INTO market_support_contexts(id,actor_user_id,organization_id,accountable_user_id,support_reference,scope,started_at,expires_at,status,version)
    VALUES ('00000000-0000-0000-0000-000000003201','00000000-0000-0000-0000-000000003001','00000000-0000-0000-0000-000000001000','00000000-0000-0000-0000-000000003001','Fixture instructions','ASSISTED_ORDER_ENTRY',now()-interval '1 minute',now()+interval '1 hour','ACTIVE',1);
END $$;

CREATE OR REPLACE FUNCTION create_fixture_current(ord uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE orderbook_orders SET status='CANCELLED',version=version+1 WHERE id=ord;
END $$;

CREATE OR REPLACE FUNCTION create_fixture_authorization_collision(k text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,market_support_context_id,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,is_anonymous,certifications,certification_declared,msds_available,off_spec,terms_digest,evidence_reference,instruction_at,acknowledge_exact_terms,acknowledge_executable_standing_order,commercial_consent_version,commercial_consent_reference,support_case_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id,status)
    SELECT gen_random_uuid(),c.organization_id,c.actor_user_id,c.id,p.id,d.id,'SPOT',200,110,'BID',c.expires_at,true,'[]',false,false,false,repeat('a',64),'Fixture instructions',now()-interval '1 minute',true,true,'fixture','fixture','fixture',k,repeat('b',64),c.actor_user_id,'ACTIVE'
    FROM market_support_contexts c CROSS JOIN products p CROSS JOIN delivery_points d
    WHERE c.id='00000000-0000-0000-0000-000000003201' AND p.id='c4cecebc-3d2d-5840-8021-57a9a11bc673' AND d.name='Rotterdam';
END $$;

CREATE OR REPLACE FUNCTION create_fixture_last_ordinal() RETURNS bigint LANGUAGE sql AS $$ SELECT last_value FROM orderbook_acceptance_ordinal_seq $$;
CREATE OR REPLACE FUNCTION create_fixture_off_spec() RETURNS void LANGUAGE sql AS $$ UPDATE orderbook_orders SET off_spec=true $$;

CREATE OR REPLACE FUNCTION create_fixture_support_change(change text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 CASE change
 WHEN 'capability_revoked' THEN UPDATE staff_capability_assignments SET revoked_at=now() WHERE capability='MARKET_SUPPORT_LISTINGS';
 WHEN 'capability_expired' THEN UPDATE staff_capability_assignments SET expires_at=now()-interval '1 second' WHERE capability='MARKET_SUPPORT_AUTHORIZATIONS';
 WHEN 'context_exited' THEN UPDATE market_support_contexts SET status='EXITED',ended_at=now();
 WHEN 'context_foreign' THEN UPDATE market_support_contexts SET actor_user_id='00000000-0000-0000-0000-000000002001';
 WHEN 'organization_pending' THEN UPDATE organizations SET verification_status='PENDING' WHERE id='00000000-0000-0000-0000-000000001000';
 WHEN 'organization_unknown' THEN UPDATE organizations SET provenance='UNKNOWN' WHERE id='00000000-0000-0000-0000-000000001000';
 WHEN 'member_absent' THEN UPDATE users SET organization_id=NULL WHERE id='00000000-0000-0000-0000-000000001001';
 WHEN 'member_rejected' THEN UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000001001';
 WHEN 'member_pending' THEN UPDATE users SET status='PENDING' WHERE id='00000000-0000-0000-0000-000000001001';
 END CASE;
END $$;
CREATE OR REPLACE FUNCTION create_fixture_catalog_change(change text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 CASE change WHEN 'minimum_zero' THEN UPDATE products SET min_lot_size=0 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673';
 WHEN 'minimum_negative' THEN UPDATE products SET min_lot_size=-1 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673';
 WHEN 'product_inactive' THEN UPDATE products SET is_active=false WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673'; END CASE;
END $$;
CREATE OR REPLACE FUNCTION create_fixture_minimum_one() RETURNS void LANGUAGE sql AS $$ UPDATE products SET min_lot_size=1 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673' $$;
CREATE OR REPLACE FUNCTION create_fixture_grace(rejected boolean) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 UPDATE users SET kyc_status=CASE WHEN rejected THEN 'REJECTED' ELSE 'PENDING' END,kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000001001';
 INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000001001','00000000-0000-0000-0000-000000001000','PENDING',now(),now()+interval '1 hour','fixture') ON CONFLICT DO NOTHING;
END $$;
CREATE SEQUENCE IF NOT EXISTS create_fixture_attempts;
CREATE OR REPLACE FUNCTION create_fixture_abort_once(state text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 PERFORM setval('create_fixture_attempts',1,false);
 EXECUTE 'CREATE OR REPLACE FUNCTION create_fixture_abort() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=public,pg_temp AS $body$ BEGIN IF nextval(''create_fixture_attempts'')=1 THEN RAISE EXCEPTION ''transaction attempt'' USING ERRCODE=' || quote_literal(state) || '; END IF; RETURN NEW; END $body$';
 DROP TRIGGER IF EXISTS create_fixture_abort ON orderbook_orders;
 CREATE TRIGGER create_fixture_abort BEFORE INSERT ON orderbook_orders FOR EACH ROW EXECUTE FUNCTION create_fixture_abort();
END $$;
CREATE OR REPLACE FUNCTION create_fixture_abort_remove() RETURNS void LANGUAGE plpgsql AS $$ BEGIN DROP TRIGGER IF EXISTS create_fixture_abort ON orderbook_orders; END $$;
