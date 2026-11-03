-- Producer fixture helpers; they run only in an explicitly migrated disposable database.

-- One outbox row as another writer appends it. A NULL evidence is the shape of
-- every writer without producer evidence; a value is a writer-shaped envelope.
CREATE OR REPLACE FUNCTION producer_fixture_row(evidence jsonb, organization uuid) RETURNS uuid
LANGUAGE plpgsql AS $$
DECLARE row_id uuid := gen_random_uuid();
BEGIN
    INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,
        payload,created_at,delivery_attempts,producer_evidence)
        VALUES(row_id,'order_updated','order',row_id::text,jsonb_build_array(organization),
            '{"schema_version":1}'::jsonb,now(),0,evidence);
    RETURN row_id;
END $$;

-- A sequence value that no committed row receives: an allocation hole.
CREATE OR REPLACE FUNCTION producer_fixture_burn() RETURNS bigint LANGUAGE sql AS $$
    SELECT nextval('market_event_stream_seq');
$$;

CREATE OR REPLACE FUNCTION producer_fixture_floor(floor_seq bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE market_event_replay_state SET discarded_through_seq=floor_seq WHERE singleton;
END $$;

CREATE OR REPLACE FUNCTION producer_fixture_fence(fence_mode text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE market_event_retention_control SET mode=fence_mode WHERE singleton;
END $$;

-- Moves the watermark back, as a restart from an older checkpoint would see it.
CREATE OR REPLACE FUNCTION producer_fixture_watermark(seq bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE watchlist_producer_state SET consumed_through_seq=seq WHERE singleton;
END $$;

-- A later display write by another writer.
CREATE OR REPLACE FUNCTION producer_fixture_display(target uuid, price numeric, remaining numeric)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE watchlist_targets SET snapshot_price_per_mt_usd=price,
        snapshot_remaining_quantity_mt=remaining WHERE id=target;
END $$;

-- A target in its own new watchlist of `owner`.
CREATE OR REPLACE FUNCTION producer_fixture_target(target uuid, owner uuid, kind text, ord uuid,
    product_code text, point uuid, window_code text) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE watch uuid := gen_random_uuid();
BEGIN
    INSERT INTO watchlists(id,user_id,name,kind) VALUES(watch,owner,'Producer ' || target::text,'CUSTOM');
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,
        delivery_point_id,availability_window_code)
        VALUES(target,watch,kind,ord,product_code,point,window_code);
    RETURN watch;
END $$;

CREATE OR REPLACE FUNCTION producer_fixture_drop_target(target uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM watchlist_targets WHERE id=target;
END $$;

CREATE OR REPLACE FUNCTION producer_fixture_refuse() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'fixture watermark refusal';
END $$;

-- Makes the watermark update fail after the event inserts of the same batch.
CREATE OR REPLACE FUNCTION producer_fixture_fail_watermark(enabled boolean) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS producer_fixture_refuse ON watchlist_producer_state;
    IF enabled THEN
        CREATE TRIGGER producer_fixture_refuse BEFORE UPDATE ON watchlist_producer_state
            FOR EACH ROW EXECUTE FUNCTION producer_fixture_refuse();
    END IF;
END $$;

-- Every row the consumer must leave untouched, as one comparable document.
CREATE OR REPLACE FUNCTION producer_fixture_untouched() RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'outbox',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM market_event_outbox o),
        'targets',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM watchlist_targets t),
        'orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM orderbook_orders o),
        'audit',(SELECT count(*) FROM audit_logs),
        'notifications',(SELECT count(*) FROM notifications),
        'trades',(SELECT count(*) FROM trades),
        'receipts',(SELECT count(*) FROM market_command_results),
        'benchmarks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM live_slice_benchmarks b));
$$;

-- `n` SLICE targets on one slice, each in its own watchlist of `owner`; returns
-- them in the writer's captured-target shape.
CREATE OR REPLACE FUNCTION producer_fixture_many_slices(owner uuid, product_code text, point uuid, n integer)
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE captured jsonb := '[]'::jsonb; target uuid; watch uuid;
BEGIN
    FOR i IN 1..n LOOP
        target := gen_random_uuid();
        watch := producer_fixture_target(target, owner, 'SLICE', NULL, product_code, point, 'SPOT');
        captured := captured || jsonb_build_array(jsonb_build_object('target_id',target,
            'watchlist_id',watch,'observer_id',owner,'target_type','SLICE','order_id',NULL,
            'market_product',product_code,'delivery_point_id',point,'availability_window','SPOT'));
    END LOOP;
    RETURN captured;
END $$;

CREATE OR REPLACE FUNCTION producer_fixture_slow() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_sleep(30);
    RETURN NULL;
END $$;

-- Makes every watchlist event insert outlast the consumer's statement deadline.
CREATE OR REPLACE FUNCTION producer_fixture_slow_inserts(enabled boolean) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS producer_fixture_slow ON watchlist_events;
    IF enabled THEN
        CREATE TRIGGER producer_fixture_slow BEFORE INSERT ON watchlist_events
            FOR EACH STATEMENT EXECUTE FUNCTION producer_fixture_slow();
    END IF;
END $$;

-- While a hold row exists, every outbox row the runtime role reads waits; the
-- test changes other committed state during that wait.
CREATE TABLE IF NOT EXISTS producer_fixture_hold(held boolean PRIMARY KEY);

CREATE OR REPLACE FUNCTION producer_fixture_gate() RETURNS boolean LANGUAGE plpgsql
SECURITY DEFINER SET search_path = pg_catalog, public AS $$
BEGIN
    WHILE EXISTS (SELECT 1 FROM public.producer_fixture_hold) LOOP
        PERFORM pg_sleep(0.01);
    END LOOP;
    RETURN true;
END $$;

CREATE OR REPLACE FUNCTION producer_fixture_hold_reads(enabled boolean) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF enabled THEN
        INSERT INTO producer_fixture_hold VALUES (true) ON CONFLICT DO NOTHING;
        ALTER TABLE market_event_outbox ENABLE ROW LEVEL SECURITY;
        DROP POLICY IF EXISTS producer_fixture_gate ON market_event_outbox;
        CREATE POLICY producer_fixture_gate ON market_event_outbox FOR SELECT TO rust_app
            USING (producer_fixture_gate());
        GRANT EXECUTE ON FUNCTION producer_fixture_gate() TO rust_app;
    ELSE
        DELETE FROM producer_fixture_hold;
    END IF;
END $$;
