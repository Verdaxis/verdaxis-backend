"""Shared disposable fixture rows for the Phase 2 watchlist route scenarios.

Identifiers are fixed so a scenario's state assertions can name rows. Orders sit
in separate slice keys. A disposable run may hold a market-visible order only
through an allowlisted DEMO organization: a database trigger fixes REAL and DEMO
provenance, and approving the fixture organization would reclassify it.
"""
import json
from datetime import UTC, datetime, timedelta

ORGANIZATION = "10000000-0000-4000-8000-000000000001"
OWNER = "20000000-0000-4000-8000-000000000001"
# Allowlisted deterministic DEMO organization.
DEMO_ORGANIZATION = "4da7b285-34ee-5443-9406-f96b4ed1a251"
# A PENDING-KYC member of the DEMO organization: its orders are public only
# while the KYC execution gate is off. Also serves as the "other user".
GATED_USER = "20000000-0000-4000-8000-000000000002"

B30 = "c4cecebc-3d2d-5840-8021-57a9a11bc673"
B100 = "74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5"
DALIAN = "78281fb3-e726-5396-802e-34b01fda21e8"
BUSAN = "262d36ae-6f35-5785-b9e8-9e221b0f1b78"
SHANGHAI = "633c0593-f9b9-52ad-abe5-a0285f6888d3"

# Orders. Each sits in its own slice key so counts do not interact.
PUBLIC_ORDER = "74000000-0000-4000-8000-000000000001"  # public, Dalian
UNKNOWN_ORDER = "74000000-0000-4000-8000-000000000002"  # never public, Busan
GATED_ORDER = "74000000-0000-4000-8000-000000000003"  # public only without the KYC gate, Shanghai

BASE_TIME = datetime(2026, 10, 19, tzinfo=UTC)
FAR_FUTURE = datetime(2100, 1, 1, tzinfo=UTC)


def at(days=0, minutes=0):
    return BASE_TIME + timedelta(days=days, minutes=minutes)


def bearer(secret, user):
    import jwt

    now = datetime.now(UTC)
    token = jwt.encode(
        {"sub": user, "type": "access", "iss": "verdaxis-test-api", "aud": "verdaxis-test-web",
         "iat": int(now.timestamp()), "exp": int((now + timedelta(minutes=30)).timestamp())},
        secret, algorithm="HS256")
    return "Bearer " + token


def insert_user(connection, user_id, email, organization_id=ORGANIZATION):
    connection.execute(
        "INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,"
        "must_change_password) VALUES(%s,%s,'not-a-login','BUYER','APPROVED',%s,true,false)",
        (user_id, email, organization_id))


def insert_container(connection, container_id, user_id, name, created_at, kind="CUSTOM"):
    connection.execute(
        "INSERT INTO watchlists(id,user_id,name,kind,created_at) VALUES(%s,%s,%s,%s,%s)",
        (container_id, user_id, name, kind, created_at))


def insert_slice(connection, target_id, container_id, product_code, point_id, window, created_at):
    connection.execute(
        "INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,"
        "delivery_point_id,availability_window_code,created_at) VALUES(%s,%s,'SLICE',%s,%s,%s,%s)",
        (target_id, container_id, product_code, point_id, window, created_at))


def insert_pin(connection, target_id, container_id, order_id, point_id, created_at):
    connection.execute(
        "INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,"
        "delivery_point_id,availability_window_code,order_id,snapshot_price_per_mt_usd,"
        "snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status,snapshot_side,"
        "snapshot_market_product,snapshot_availability_window,created_at) "
        "VALUES(%s,%s,'PIN','B30',%s,'SPOT',%s,612.5,100,100,'OPEN','BID','B30','SPOT',%s)",
        (target_id, container_id, point_id, order_id, created_at))


def insert_event(connection, event_id, container_id, target_id, event_type, payload, created_at,
                 is_read=False):
    connection.execute(
        "INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,"
        "event_payload,is_read,created_at) VALUES(%s,%s,%s,%s,%s::json,%s,%s)",
        (event_id, container_id, target_id, event_type, json.dumps(payload), is_read, created_at))


def insert_entry(connection, entry_id, container_id, product_id, point_id, created_at):
    connection.execute(
        "INSERT INTO watchlist_entries(id,watchlist_id,product_id,delivery_point_id,created_at) "
        "VALUES(%s,%s,%s,%s,%s)",
        (entry_id, container_id, product_id, point_id, created_at))


def order_event(order_id):
    return {"source_kind": "LIVE_ORDER", "demo_status": "DEMO", "scope": "DELIVERY_POINT",
            "order_id": order_id, "old_price_per_mt_usd": 590.0, "new_price_per_mt_usd": 612.5}


def insert_order(connection, order_id, organization_id, owner_id, provenance, point_id, expires_at):
    connection.execute(
        "INSERT INTO orderbook_orders(id,organization_id,owner_user_id,provenance,side,product_id,"
        "delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,"
        "status,expires_at,created_at,updated_at) "
        "VALUES(%s,%s,%s,%s,'BID',%s,%s,100,100,600,'SPOT','OPEN',%s,%s,%s)",
        (order_id, organization_id, owner_id, provenance, B30, point_id, expires_at, BASE_TIME, BASE_TIME))


def seed_users_and_orders(connection):
    connection.execute(
        "INSERT INTO organizations(id,name,domain,type,verification_status,provenance) "
        "VALUES(%s,'Phase 2 demo fixture','demo.phase2.invalid','FUEL_BUYER','APPROVED','DEMO')",
        (DEMO_ORGANIZATION,))
    insert_user(connection, GATED_USER, "gated@phase2.invalid", DEMO_ORGANIZATION)
    insert_order(connection, PUBLIC_ORDER, DEMO_ORGANIZATION, None, "DEMO", DALIAN, FAR_FUTURE)
    insert_order(connection, UNKNOWN_ORDER, ORGANIZATION, None, "UNKNOWN", BUSAN, None)
    insert_order(connection, GATED_ORDER, DEMO_ORGANIZATION, GATED_USER, "DEMO", SHANGHAI, FAR_FUTURE)
