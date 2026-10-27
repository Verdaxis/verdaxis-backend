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
