-- Installed by the database administrator in each cloned database of the
-- offboarding composition target, after fixtures/offboarding_market.sql.
--
-- A failure switch per table: while a table's name is in
-- composition_failpoints, any row change on that table raises, so a case can
-- stop the composed command inside its market work (trades) or after it
-- (individual_offboardings).
CREATE TABLE IF NOT EXISTS composition_failpoints (table_name text PRIMARY KEY);

CREATE OR REPLACE FUNCTION composition_fail() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM composition_failpoints WHERE table_name = TG_TABLE_NAME) THEN
        RAISE EXCEPTION 'injected failure after a change to %', TG_TABLE_NAME USING ERRCODE = 'P0001';
    END IF;
    RETURN NULL;
END $$;

CREATE OR REPLACE TRIGGER composition_fail AFTER INSERT OR UPDATE OR DELETE
    ON trades FOR EACH ROW EXECUTE FUNCTION composition_fail();
CREATE OR REPLACE TRIGGER composition_fail AFTER INSERT OR UPDATE OR DELETE
    ON individual_offboardings FOR EACH ROW EXECUTE FUNCTION composition_fail();

-- The departing person's credentials: two refresh families and one reset
-- token, and one pending request to join another organization.
CREATE OR REPLACE FUNCTION composition_credentials(subject uuid, other_organization uuid, join_request uuid)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(other_organization,'Composition ' || other_organization,'FUEL_SUPPLIER','APPROVED','REAL');
    FOR i IN 1..2 LOOP
        INSERT INTO refresh_sessions(id,user_id,family_id,jti_hash,device_id_hash,expires_at)
            VALUES(gen_random_uuid(),subject,gen_random_uuid(),md5(random()::text) || md5(random()::text),
                   md5(random()::text) || md5(random()::text),now() + interval '7 days');
    END LOOP;
    INSERT INTO password_reset_tokens(user_id,token_hash,expires_at,created_at)
        VALUES(subject,md5(random()::text) || md5(random()::text),now() + interval '1 hour',now());
    INSERT INTO organization_join_requests(id,user_id,organization_id,status,created_at)
        VALUES(join_request,subject,other_organization,'PENDING',now());
END $$;

-- Every row an offboarding can change, keyed by identity.
CREATE OR REPLACE FUNCTION composition_snapshot(subject uuid) RETURNS jsonb LANGUAGE sql AS $$
    SELECT offboarding_market_snapshot() || jsonb_build_object(
        'user',(SELECT to_jsonb(u) FROM users u WHERE id=subject),
        'sessions',(SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY id),'[]') FROM refresh_sessions s WHERE user_id=subject),
        'reset',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY token_hash),'[]') FROM password_reset_tokens r WHERE user_id=subject),
        'joins',(SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY id),'[]') FROM organization_join_requests j WHERE user_id=subject),
        'transitions',(SELECT count(*) FROM user_status_transitions WHERE user_id=subject),
        'records',(SELECT count(*) FROM individual_offboardings),
        'organizations',(SELECT coalesce(jsonb_agg(to_jsonb(o) ORDER BY id),'[]') FROM organizations o));
$$;
