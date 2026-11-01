-- Installed only in fresh disposable databases by the producer chain and
-- transport cells of the residual writer coexistence suite. Every row these
-- cells need is written here; the Rust target only calls these functions.

-- An approved REAL organization with one approved account that only watches
-- the market: it is never a party to the economic writes of these cells.
CREATE OR REPLACE FUNCTION chain_observer(org uuid, account uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
    VALUES(org,'Chain observer','SHIPOWNER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
    VALUES(account,'chain-observer@example.test','unused','BUYER','APPROVED',org,true,false,'APPROVED',org);
END $$;

-- Session advisory barrier: a Rust command whose pool carries the held
-- application name waits here after its outbox row is inserted and before it
-- commits, so the row exists only inside the open writer transaction.
CREATE OR REPLACE FUNCTION chain_hold() RETURNS void LANGUAGE sql AS $$ SELECT pg_advisory_lock(120120002) $$;
CREATE OR REPLACE FUNCTION chain_release() RETURNS boolean LANGUAGE sql AS $$ SELECT pg_advisory_unlock(120120002) $$;
CREATE OR REPLACE FUNCTION chain_outbox_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE writer_bound text := current_setting('lock_timeout');
BEGIN
    IF current_setting('application_name')='chain-rust-held' THEN
        -- The observation barrier is separate from the writer's own lock bound.
        PERFORM set_config('lock_timeout','10s',true);
        PERFORM pg_advisory_xact_lock(120120002);
        PERFORM set_config('lock_timeout',writer_bound,true);
    END IF;
    RETURN NULL;
END $$;
DROP TRIGGER IF EXISTS chain_outbox_barrier ON market_event_outbox;
CREATE TRIGGER chain_outbox_barrier AFTER INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION chain_outbox_barrier();

-- The outbox rows of one aggregate, as committed. Producer evidence is reduced
-- to its mutation identity and captured target set; the transport columns are
-- returned whole.
CREATE OR REPLACE FUNCTION chain_outbox(aggregate text) RETURNS jsonb LANGUAGE sql AS $$
    SELECT coalesce(jsonb_agg(jsonb_build_object(
        'id',id,'seq',stream_seq,'event_type',event_type,
        'participants',participant_org_ids::jsonb,'payload',payload::jsonb,
        'attempts',delivery_attempts,'dispatched',dispatched_at IS NOT NULL,
        'mutation',producer_evidence->>'mutation_id',
        'targets',producer_evidence->'targets') ORDER BY created_at,id),'[]'::jsonb)
    FROM market_event_outbox WHERE aggregate_id=aggregate
$$;

-- The same view for every row sequenced after `after_seq`.
CREATE OR REPLACE FUNCTION chain_sequenced_after(after_seq bigint) RETURNS jsonb LANGUAGE sql AS $$
    SELECT coalesce(jsonb_agg(jsonb_build_object(
        'id',id,'seq',stream_seq,'event_type',event_type,'aggregate_id',aggregate_id,
        'participants',participant_org_ids::jsonb,'payload',payload::jsonb,
        'attempts',delivery_attempts,
        'mutation',producer_evidence->>'mutation_id',
        'targets',producer_evidence->'targets') ORDER BY stream_seq),'[]'::jsonb)
    FROM market_event_outbox WHERE stream_seq>after_seq
$$;

CREATE OR REPLACE FUNCTION chain_high_water() RETURNS bigint LANGUAGE sql AS $$
    SELECT COALESCE(max(stream_seq),0) FROM market_event_outbox
$$;

CREATE OR REPLACE FUNCTION chain_unsequenced() RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM market_event_outbox WHERE stream_seq IS NULL
$$;

-- The backend that holds the shared stream sequencer lock, if any.
CREATE OR REPLACE FUNCTION chain_sequencer_holder() RETURNS integer LANGUAGE sql AS $$
    SELECT l.pid FROM pg_locks l
    WHERE l.locktype='advisory' AND l.granted AND l.objsubid=1
      AND l.database=(SELECT oid FROM pg_database WHERE datname=current_database())
      AND ((l.classid::bigint << 32) | l.objid::bigint)=7264537260226625
    LIMIT 1
$$;

CREATE OR REPLACE FUNCTION chain_backends(app text) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND application_name=app
$$;

-- The trusted watchlist events, one document per row.
CREATE OR REPLACE FUNCTION chain_events() RETURNS jsonb LANGUAGE sql AS $$
    SELECT coalesce(jsonb_agg(jsonb_build_object(
        'target',watchlist_target_id,'class',event_type,'key',derived_event_key,
        'producer',producer,'source',source_event_id,'seq',source_stream_seq,
        'payload',event_payload::jsonb) ORDER BY watchlist_target_id,derived_event_key),'[]'::jsonb)
    FROM watchlist_events
$$;

CREATE OR REPLACE FUNCTION chain_watermark() RETURNS bigint LANGUAGE sql AS $$
    SELECT consumed_through_seq FROM watchlist_producer_state
$$;
