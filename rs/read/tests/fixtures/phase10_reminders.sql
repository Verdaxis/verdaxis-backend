-- Test fixture: writers for the Phase 10 order-expiry reminder target
-- (rs/read/tests/phase10_reminders_postgres.rs). The target creates these
-- functions as the disposable schema owner and calls them by name, so no Rust
-- literal outside the order-book write boundary writes or locks an order-book
-- row. Every function is test-only and runs on a disposable *_test database.

-- An approved organization (the approval trigger classifies it REAL) with one
-- approved, verified member.
CREATE OR REPLACE FUNCTION reminder_fixture_party(org uuid, member uuid, address text)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id, name, type)
    VALUES (org, 'Reminder fixture ' || org::text, 'FUEL_BUYER');
    UPDATE organizations SET verification_status = 'APPROVED' WHERE id = org;
    INSERT INTO users(id, email, password_hash, role, organization_id, status, email_verified)
    VALUES (member, address, 'unused', 'BUYER', org, 'APPROVED', true);
END $$;

-- Another member of an existing organization.
CREATE OR REPLACE FUNCTION reminder_fixture_member(org uuid, member uuid, address text, verified boolean)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO users(id, email, password_hash, role, organization_id, status, email_verified)
    VALUES (member, address, 'unused', 'BUYER', org, 'APPROVED', verified);
$$;

CREATE OR REPLACE FUNCTION reminder_fixture_product(product uuid)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO products(id, name, fuel_type, fuel_grade)
    VALUES (product, 'Reminder fixture fuel ' || product::text, 'Methanol', 'Bio');
$$;

-- One open real order of 100 MT.
CREATE OR REPLACE FUNCTION reminder_fixture_order(
    order_id uuid, org uuid, owner uuid, product uuid, order_side text, expiry timestamptz)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO orderbook_orders(
        id, organization_id, owner_user_id, product_id, provenance, side,
        quantity_mt, remaining_quantity_mt, price_per_mt_usd, availability_window,
        status, expires_at)
    VALUES (order_id, org, owner, product, 'REAL', order_side, 100, 100, 500, 'SPOT',
        'OPEN', expiry);
$$;

CREATE OR REPLACE FUNCTION reminder_fixture_set_expiry(order_id uuid, expiry timestamptz)
RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET expires_at = expiry WHERE id = order_id;
$$;

CREATE OR REPLACE FUNCTION reminder_fixture_set_owner(order_id uuid, owner uuid)
RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET owner_user_id = owner WHERE id = order_id;
$$;

CREATE OR REPLACE FUNCTION reminder_fixture_set_email(member uuid, address text)
RETURNS void LANGUAGE sql AS $$
    UPDATE users SET email = address WHERE id = member;
$$;

CREATE OR REPLACE FUNCTION reminder_fixture_preferences(member uuid, email_updates boolean, inapp_updates boolean)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO user_preferences(id, user_id, namespace, value, updated_at)
    VALUES (gen_random_uuid(), member, 'notifications',
        json_build_object('email_trade_updates', email_updates, 'inapp_trade_updates', inapp_updates),
        now())
    ON CONFLICT (user_id, namespace) DO UPDATE SET value = EXCLUDED.value;
$$;

-- Leave nothing for the next case: no live order and no reminder.
CREATE OR REPLACE FUNCTION reminder_fixture_quiesce()
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE orderbook_orders SET status = 'CANCELLED' WHERE status IN ('OPEN', 'PARTIALLY_FILLED');
    DELETE FROM notifications;
END $$;

-- The reminder scan key, held until the caller's transaction ends.
CREATE OR REPLACE FUNCTION reminder_fixture_hold_scan_key()
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(8246192048);
END $$;

-- The market cancel path's two row locks, in its order: the actor's user row,
-- then the order row and its status change.
CREATE OR REPLACE FUNCTION reminder_fixture_lock_user(member uuid)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM users WHERE id = member FOR UPDATE;
END $$;

CREATE OR REPLACE FUNCTION reminder_fixture_cancel_order(order_id uuid)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM orderbook_orders WHERE id = order_id FOR UPDATE;
    UPDATE orderbook_orders SET status = 'CANCELLED' WHERE id = order_id;
END $$;

-- The order-first scan shape: the order row lock, then a notification insert
-- whose foreign key check needs the recipient's user row.
CREATE OR REPLACE FUNCTION reminder_fixture_lock_order(order_id uuid)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM orderbook_orders WHERE id = order_id FOR UPDATE;
END $$;

CREATE OR REPLACE FUNCTION reminder_fixture_insert_notice(recipient uuid)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO notifications(id, recipient_id, type, title, message, data, is_read, created_at)
    VALUES (gen_random_uuid(), recipient, 'SYSTEM', 'Fixture', 'Fixture', '{}'::json, false, now());
$$;

-- Lock probes. Each returns false when the lock is held elsewhere and never
-- waits; run in autocommit, an acquired lock ends with the statement.
CREATE OR REPLACE FUNCTION reminder_fixture_order_lock_free(order_id uuid)
RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM orderbook_orders WHERE id = order_id FOR UPDATE NOWAIT;
    RETURN true;
EXCEPTION WHEN lock_not_available THEN
    RETURN false;
END $$;

CREATE OR REPLACE FUNCTION reminder_fixture_user_update_lock_free(member uuid)
RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM users WHERE id = member FOR UPDATE NOWAIT;
    RETURN true;
EXCEPTION WHEN lock_not_available THEN
    RETURN false;
END $$;

CREATE OR REPLACE FUNCTION reminder_fixture_user_no_key_lock_free(member uuid)
RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM users WHERE id = member FOR NO KEY UPDATE NOWAIT;
    RETURN true;
EXCEPTION WHEN lock_not_available THEN
    RETURN false;
END $$;
