DROP TRIGGER IF EXISTS cancel_fixture_fail_audit ON audit_logs;
CREATE OR REPLACE FUNCTION cancel_fixture_seed(ord uuid, actor uuid, member_id uuid, stranger uuid, org uuid, context_id uuid, product uuid, point uuid, code text, standard text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE buyer_org uuid:=gen_random_uuid(); item uuid:=gen_random_uuid();
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(org,'Cancellation fixture','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id) VALUES(actor,actor::text||'@example.test','unused','SUPPLIER','APPROVED',org,true,false,'APPROVED',org);
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(buyer_org,'Order buyer fixture','SHIPOWNER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
    VALUES(member_id,member_id::text||'@example.test','unused','SUPPLIER','APPROVED',org,true,false,'APPROVED',org),
          (stranger,stranger::text||'@example.test','unused','BUYER','APPROVED',buyer_org,true,false,'APPROVED',buyer_org);
    INSERT INTO inventory_items(id,supplier_id,owner_user_id,fuel_type,current_stock_mt,reserved_stock_mt,incoming_stock_mt,updated_at)
    VALUES(item,org,actor,'METHANOL',0,10,0,now());
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,inventory_item_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
    VALUES(ord,org,actor,actor,'SELF_SERVICE','REAL','ASK',product,point,item,10,6,100,'SPOT','PARTIALLY_FILLED',false,true,'ISCC EU',standard,true,10,'LIFECYCLE','residue','NL');
    INSERT INTO watchlists(id,user_id,name,kind) VALUES(context_id,actor,'Cancellation fixture','CUSTOM');
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code,snapshot_price_per_mt_usd,snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status) VALUES(gen_random_uuid(),context_id,'PIN',ord,code,point,'SPOT',100,10,6,'PARTIALLY_FILLED');
    INSERT INTO trades(id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,initiated_by,quantity_mt,price_per_mt_usd,status,buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version)
    SELECT gen_random_uuid(),ord,buyer_org,org,stranger,actor,buyer_org,'BUYER',4,100,'PENDING_CONFIRMATION','REAL','REAL',p.id,p.name,p.fuel_type,p.fuel_grade,code,d.id,d.name,d.region,'SPOT',1 FROM products p JOIN delivery_points d ON d.id=point WHERE p.id=product;
    INSERT INTO market_support_contexts(id,actor_user_id,organization_id,accountable_user_id,support_reference,started_at,expires_at,status,version)
    VALUES(context_id,actor,org,actor,'Fixture request',now(),now()+interval '1 hour','ACTIVE',1);
END $$;

CREATE OR REPLACE FUNCTION cancel_fixture_change(ord uuid,context_id uuid,change text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE actor uuid; org uuid; member_id uuid; stranger uuid; auth uuid:=gen_random_uuid();
BEGIN
    PERFORM set_config('session_replication_role','replica',true);
    SELECT owner_user_id,organization_id INTO actor,org FROM orderbook_orders WHERE id=ord;
    SELECT id INTO member_id FROM users WHERE organization_id=org AND id<>actor LIMIT 1;
    SELECT id INTO stranger FROM users WHERE organization_id<>org LIMIT 1;
    IF change='ordinary' THEN RETURN;
    ELSIF change='legacy' THEN UPDATE orderbook_orders SET owner_user_id=NULL,created_by_actor_user_id=NULL,creation_method='LEGACY_UNKNOWN' WHERE id=ord;
    ELSIF change='detached' THEN UPDATE users SET organization_id=NULL WHERE id=actor;
    ELSIF change='rejected' THEN UPDATE users SET status='REJECTED',authentication_revoked_at=now()+interval '1 day' WHERE id=actor;
    ELSIF change='pending' THEN UPDATE users SET status='PENDING' WHERE id=actor;
    ELSIF change='unverified' THEN UPDATE users SET email_verified=false WHERE id=actor;
    ELSIF change='password_required' THEN UPDATE users SET must_change_password=true WHERE id=actor;
    ELSIF change='kyc_rejected' THEN UPDATE users SET kyc_status='REJECTED' WHERE id=actor;
    ELSIF change='organization_rejected' THEN UPDATE organizations SET verification_status='REJECTED' WHERE id=org;
    ELSIF change='password_cutoff' THEN UPDATE users SET password_changed_at=now()+interval '1 day' WHERE id=actor;
    ELSIF change='revocation_cutoff' THEN UPDATE users SET authentication_revoked_at=now()+interval '1 day' WHERE id=actor;
    ELSIF change='filled' THEN UPDATE orderbook_orders SET remaining_quantity_mt=0,status='FILLED' WHERE id=ord;
    ELSIF change='no_point' THEN UPDATE orderbook_orders SET delivery_point_id=NULL WHERE id=ord;
    ELSIF change='tier' THEN UPDATE organizations SET supplier_tier='MAJOR_TRADER' WHERE id=org;
    ELSIF change='admin_owner' THEN UPDATE users SET role='ADMIN' WHERE id=actor;
    ELSIF change='legacy_window' THEN UPDATE orderbook_orders SET availability_window='Q3_2026' WHERE organization_id=org AND id<>ord;
    ELSIF change='assisted' THEN
        UPDATE users SET role='ADMIN' WHERE id=actor;
        INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,order_side,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,certification_declared,certification_scheme,msds_available,authorization_expires_at,status,created_by_actor_user_id,market_support_context_id,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash)
        SELECT auth,org,actor,side,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,true,'ISCC EU',true,now()+interval '1 hour','CONSUMED',actor,context_id,repeat('a',64),'Fixture request','fixture-v1','Fixture request',auth::text,repeat('b',64) FROM orderbook_orders WHERE id=ord;
        UPDATE orderbook_orders SET creation_method='MARKET_SUPPORT',support_authorization_id=auth WHERE id=ord;
        INSERT INTO staff_capability_assignments(id,user_id,capability,reason,granted_by_user_id) VALUES(gen_random_uuid(),actor,'MARKET_SUPPORT_AUTHORIZATIONS','Fixture request',actor),(gen_random_uuid(),actor,'MARKET_SUPPORT_LISTINGS','Fixture request',actor);
    ELSIF change='context_valid' THEN RETURN;
    ELSIF change='context_customer_rejected' THEN UPDATE users SET kyc_status='REJECTED',status='REJECTED' WHERE id=member_id;
    ELSIF change='context_foreign' THEN UPDATE market_support_contexts SET actor_user_id=stranger WHERE id=context_id;
    ELSIF change='context_expired' THEN UPDATE market_support_contexts SET started_at=now()-interval '2 hours',expires_at=now()-interval '1 second' WHERE id=context_id;
    ELSIF change='context_revoked' THEN UPDATE market_support_contexts SET status='REVOKED',ended_at=now() WHERE id=context_id;
    ELSIF change='context_other_org' THEN UPDATE market_support_contexts SET organization_id=(SELECT organization_id FROM users WHERE id=stranger) WHERE id=context_id;
    ELSIF change='context_capability_revoked' THEN UPDATE staff_capability_assignments SET revoked_at=now(),revoked_by_user_id=actor,revocation_reason='Fixture request' WHERE user_id=actor;
    ELSIF change='owner_rows' THEN
        PERFORM effect_fixture_insert_order(ord,gen_random_uuid());
        PERFORM effect_fixture_insert_order(ord,gen_random_uuid());
        DELETE FROM orderbook_orders WHERE organization_id=org AND side='BID';
        UPDATE orderbook_orders SET created_at='2026-10-01T00:00:00Z' WHERE organization_id=org;
        UPDATE orderbook_orders SET off_spec_notes='Private owner instructions' WHERE id=ord;
    ELSIF change='typed_owner' THEN
        UPDATE orderbook_orders SET created_at=now(),product_id=(SELECT id FROM products WHERE name='UCOME B100' LIMIT 1),delivery_point_id=(SELECT id FROM delivery_points WHERE name='Singapore' LIMIT 1),fame_terms='{"schema_version": 1, "neat_fame": true, "nomination_status": "IDENTIFIED", "batch_reference": "PRIVATE-BATCH-001", "producing_site": "PRIVATE-PRODUCTION-SITE", "production_origin": "Malaysia", "feedstock_origin": "Malaysia", "shipping_location": "Singapore", "uco_mass_pct": 100, "standard": "EN_14214", "standard_edition": "2012+A2:2019", "en_climate_class": "A", "cfpp_c": "-2", "cloud_point_c": "1", "ci_gco2e_mj": "20", "ci_methodology": "ISCC EU declared method", "ci_boundary": "Well-to-wheel", "ci_basis": "ACTUAL", "sustainability_scheme": "ISCC_EU", "certificate_reference": "PRIVATE-CERTIFICATE-ID", "certificate_holder": "PRIVATE-CERTIFICATE-HOLDER", "certificate_valid_until": "2030-12-31", "certificate_scope": "PRIVATE-CERTIFICATE-SCOPE", "evidence_status": "AVAILABLE", "document_references": ["PRIVATE-DOCUMENT-REFERENCE"], "lhv_mj_kg": "37.2", "quality_evidence": {"status": "AVAILABLE", "reference": "PRIVATE-QUALITY-REFERENCE", "batch_reference": "PRIVATE-BATCH-001", "laboratory": "PRIVATE-LABORATORY", "sampled_on": "2026-10-25", "tested_on": "2026-10-25", "results": [{"property": "ESTER_CONTENT_MASS_PCT", "value": "97.5", "method": "EN 14103"}]}, "sustainability_evidence": {"status": "AVAILABLE", "document_type": "POS", "reference": "PRIVATE-SUSTAINABILITY-REFERENCE", "issuer": "PRIVATE-ISSUER", "quantity_mt": "100.00", "supply_date": "2026-10-25", "due": "BEFORE_LOADING"}, "side": "ASK", "evidence_due": "BEFORE_LOADING"}'::json WHERE id=ord;
    ELSIF change='templates' THEN
        PERFORM effect_fixture_insert_order(ord,gen_random_uuid());
        UPDATE orderbook_orders SET expires_at=now()-interval '1 second',created_at=now()+interval '1 hour',price_per_mt_usd=999 WHERE organization_id=org AND id<>ord AND side='ASK';
        UPDATE orderbook_orders SET off_spec=true,off_spec_notes='Owner note' WHERE id=ord;
    ELSIF change='all_expired' THEN UPDATE orderbook_orders SET expires_at=now()-interval '1 second' WHERE organization_id=org;
    ELSIF change='audit_failure' THEN
        EXECUTE 'CREATE OR REPLACE FUNCTION cancel_fixture_fail_audit() RETURNS trigger LANGUAGE plpgsql AS $body$ BEGIN RAISE EXCEPTION ''fixture audit refusal''; END $body$';
        EXECUTE 'CREATE TRIGGER cancel_fixture_fail_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION cancel_fixture_fail_audit()';
    ELSE RAISE EXCEPTION 'unknown fixture change';
    END IF;
    PERFORM set_config('session_replication_role','origin',true);
END $$;

CREATE OR REPLACE FUNCTION cancel_fixture_state(ord uuid) RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object('current',i.current_stock_mt::text,'reserved',i.reserved_stock_mt::text,'remaining',o.remaining_quantity_mt::text,'status',o.status,'version',o.version,
 'trade',(SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM trades t WHERE t.ask_order_id=ord),
 'audit_count',(SELECT count(*) FROM audit_logs),'event_count',(SELECT count(*) FROM market_event_outbox),'notification_count',(SELECT count(*) FROM notifications),
 'receipt_count',(SELECT count(*) FROM market_command_results),
 'audit',(SELECT changes FROM audit_logs ORDER BY created_at DESC LIMIT 1),
 'participants',(SELECT participant_org_ids::jsonb FROM market_event_outbox ORDER BY created_at DESC LIMIT 1),
 'pin_status',(SELECT snapshot_status FROM watchlist_targets WHERE order_id=ord LIMIT 1),
 'before_status',(SELECT producer_evidence->'orders'->0->'before'->'status' FROM market_event_outbox ORDER BY created_at DESC LIMIT 1),
 'after_status',(SELECT producer_evidence->'orders'->0->'after'->'status' FROM market_event_outbox ORDER BY created_at DESC LIMIT 1),
 'payload',(SELECT payload::jsonb FROM market_event_outbox ORDER BY created_at DESC LIMIT 1),
 'observed_at_matches_update',(SELECT (payload->>'observed_at')::timestamptz=o.updated_at FROM market_event_outbox ORDER BY created_at DESC LIMIT 1))
FROM orderbook_orders o LEFT JOIN inventory_items i ON i.id=o.inventory_item_id WHERE o.id=ord
$$;

CREATE OR REPLACE FUNCTION cancel_fixture_slice_key(ord uuid) RETURNS bigint LANGUAGE sql AS $$
SELECT ('x'||substr(encode(sha256(convert_to(product_id::text||'|'||coalesce(delivery_point_id::text,'')||'|'||availability_window,'UTF8')),'hex'),1,16))::bit(64)::bigint FROM orderbook_orders WHERE id=ord
$$;
CREATE OR REPLACE FUNCTION cancel_fixture_hold(key bigint) RETURNS void LANGUAGE sql AS $$ SELECT pg_advisory_xact_lock(key) $$;

CREATE OR REPLACE FUNCTION cancel_fixture_hold_owner_rows(org uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM id FROM organizations WHERE id=org FOR UPDATE;
    PERFORM id FROM orderbook_orders WHERE organization_id=org ORDER BY id FOR UPDATE;
END $$;
