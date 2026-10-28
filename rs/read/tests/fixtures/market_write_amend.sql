-- Helpers for the amendment suite on a migrated disposable database.
CREATE OR REPLACE FUNCTION amend_fixture_minimum(lot numeric) RETURNS void LANGUAGE sql AS $$
    UPDATE products SET is_active=true,min_lot_size=lot WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673'
$$;
CREATE OR REPLACE FUNCTION amend_fixture_split(ord uuid, filled numeric) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE orderbook_orders SET quantity_mt=remaining_quantity_mt+filled WHERE id=ord;
END $$;

CREATE OR REPLACE FUNCTION amend_fixture_fill(ord uuid, quantity numeric) RETURNS void LANGUAGE plpgsql AS $$
DECLARE buyer uuid; remaining numeric;
BEGIN
    SELECT organization_id INTO STRICT buyer FROM orderbook_orders WHERE id=ord;
    SELECT remaining_quantity_mt-quantity INTO remaining FROM orderbook_orders WHERE id=ord;
    UPDATE orderbook_orders SET quantity_mt=quantity_mt+quantity,status='PARTIALLY_FILLED' WHERE id=ord;
    UPDATE orderbook_orders SET remaining_quantity_mt=remaining WHERE id=ord;
END $$;

CREATE OR REPLACE FUNCTION amend_fixture_expiring(ord uuid, at timestamptz) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET expires_at=at WHERE id=ord
$$;

CREATE OR REPLACE FUNCTION amend_fixture_status(ord uuid, state text) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET status=state WHERE id=ord
$$;

CREATE OR REPLACE FUNCTION amend_fixture_assist(ord uuid) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET creation_method='MARKET_SUPPORT' WHERE id=ord
$$;

CREATE OR REPLACE FUNCTION amend_fixture_owner(ord uuid, owner uuid) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET owner_user_id=owner, created_by_actor_user_id=owner WHERE id=ord
$$;

CREATE OR REPLACE FUNCTION amend_fixture_ownerless(ord uuid) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET owner_user_id=NULL, created_by_actor_user_id=NULL WHERE id=ord
$$;

CREATE OR REPLACE FUNCTION amend_fixture_other_member(org uuid, member uuid, member_role text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(org,'Amendment other','SHIPOWNER','APPROVED','REAL');
    PERFORM matching_fixture_member(org,member,member_role);
END $$;

CREATE OR REPLACE FUNCTION amend_fixture_inert_ask(target uuid) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET certification_declared=false WHERE id=target
$$;

CREATE OR REPLACE FUNCTION amend_fixture_last_ordinal() RETURNS bigint LANGUAGE sql AS $$
    SELECT last_value FROM orderbook_acceptance_ordinal_seq
$$;

CREATE OR REPLACE FUNCTION amend_fixture_set_price(target uuid, price numeric) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET price_per_mt_usd=price WHERE id=target
$$;

CREATE OR REPLACE FUNCTION amend_fixture_crossing(ord uuid, source uuid, actor uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE orderbook_orders SET price_per_mt_usd=100,
        owner_user_id=actor, created_by_actor_user_id=actor
    WHERE id=ord;
    PERFORM count(*) FROM orderbook_orders WHERE id IN (ord, source);
END $$;
COMMENT ON FUNCTION amend_fixture_crossing(uuid,uuid,uuid) IS 'The cloned crossing bid takes the source order''s immutable side and slice';
