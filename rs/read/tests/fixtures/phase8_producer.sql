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
