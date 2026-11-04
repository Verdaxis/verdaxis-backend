"""Rollback proofs for the Phases 8-9 market-write cutover on disposable PostgreSQL.

Four cases cover what a rollback after the Rust market-write switch can and
cannot do:

* ``downgrade_to_p5_is_lossless_before_any_rust_write``: the real Alembic
  downgrade from the freeze revision (or the producer revision) to p5 keeps
  every row of every p5 table and leaves exactly the p5 schema.
* ``downgrade_refuses_after_a_rust_write_or_a_freeze_row``: producer evidence,
  a trusted watchlist row or a freeze row (active or cleared) stops the same
  downgrade, and the refusal leaves schema and data untouched.
* ``python_replays_a_rust_receipt_or_answers_409``: Python finds receipts that
  Rust wrote and either replays the stored result or answers 409. The test
  records what it observed for each of the six receipt-backed operations; it
  is not a parity gate for the response shape.
* ``python_settlement_on_a_frozen_trade_writes_nothing``: with an active freeze
  row Python's trade routes fail and leave no partial write, no receipt and no
  consumed idempotency key.

Rust-written receipt rows are inserted with the columns, operation names and
request-hash encoding the Rust boundary uses. The hash encoder below is
checked against the shared hash vectors that the Rust unit tests also read.
"""
from __future__ import annotations

import hashlib
import json
from datetime import UTC, datetime, timedelta, timezone
from decimal import Decimal
from pathlib import Path
from uuid import uuid4

import pytest
from app.models.command_result import MarketCommandResult
from app.models.user import User, UserRole, UserStatus
from sqlalchemy import text

from tests.postgres.test_market_routes import (  # noqa: F401
    _headers,
    _reviewed_terms_digest,
    route_market,
)
from tests.postgres.test_phase8_producer_schema import (
    SOURCE,
    insert_event,
    seed_target,
)
from tests.postgres.test_phase9_schema import (
    ACTOR,
    BUYER,
    SELLER,
    TRADE,
    catalog,
    clear_freeze,
    freeze_insert,
    insert_trade,
    seed_trade,
)
from tests.postgres.test_supplier_offer_migration_roundtrip import (
    supplier_scratch_database,  # noqa: F401
)

ROOT = Path(__file__).resolve().parents[2]
P5 = "p5_20261020_read_indexes"
PRODUCER = "p89_20261022_watchlist_producer"
FREEZE = "p89_20261022_trade_freeze"
SECOND_TRADE = "88888888-9999-0000-0000-000000000005"
LEGACY_EVENT = "88888888-7777-0000-0000-000000000006"
TRUSTED_EVENT = "88888888-7777-0000-0000-000000000007"
TRUSTED_COLUMNS = ",producer,source_event_id,source_stream_seq,scope_attested_at,derived_event_key"
TRUSTED_VALUES = f",'rust-market-v1','{SOURCE}',1,now(),'PIN_PRICE_CHANGED|order|ASK|0'"
EXCLUDED_TABLES = {"alembic_version", "spatial_ref_sys"}


# -- schema and data fingerprints ------------------------------------------------


def schema_fingerprint(sql) -> dict:
    """Everything the downgrade must restore: catalog, columns, defaults and ACLs."""
    fingerprint = catalog(sql)
    fingerprint["columns"] = sql("""
        SELECT c.relname || '.' || a.attname || ':' || format_type(a.atttypid, a.atttypmod)
               || ':' || a.attnotnull::text || ':' || coalesce(pg_get_expr(d.adbin, d.adrelid), '')
               || ':' || coalesce(a.attacl::text, '')
        FROM pg_attribute a
        JOIN pg_class c ON c.oid = a.attrelid
        LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
        WHERE c.relnamespace = 'public'::regnamespace AND c.relkind = 'r'
          AND a.attnum > 0 AND NOT a.attisdropped
        ORDER BY 1""").splitlines()
    fingerprint["owners_and_acls"] = sql("""
        SELECT relname || ':' || relkind::text || ':' || pg_get_userbyid(relowner)
               || ':' || coalesce(relacl::text, '')
        FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'S')
        ORDER BY 1""").splitlines()
    return fingerprint


def table_columns(sql) -> dict[str, list[str]]:
    rows = sql("""
        SELECT c.relname || ' ' || a.attname FROM pg_attribute a
        JOIN pg_class c ON c.oid = a.attrelid
        WHERE c.relnamespace = 'public'::regnamespace AND c.relkind = 'r'
          AND a.attnum > 0 AND NOT a.attisdropped ORDER BY 1""")
    columns: dict[str, list[str]] = {}
    for row in rows.splitlines():
        table, column = row.split(" ")
        if table not in EXCLUDED_TABLES:
            columns.setdefault(table, []).append(column)
    return columns


def data_snapshot(sql, columns: dict[str, list[str]]) -> dict[str, str]:
    """Row count and an order-independent digest of the named columns, per table."""
    parts = []
    for table, names in sorted(columns.items()):
        keep = "ARRAY[" + ",".join(f"'{name}'" for name in names) + "]"
        parts.append(f"""
            SELECT '{table}', count(*) || ':' || coalesce(md5(string_agg(j::text, '|' ORDER BY j::text)), '')
            FROM (SELECT (SELECT jsonb_object_agg(k, v) FROM jsonb_each(to_jsonb(t)) AS e(k, v)
                          WHERE k = ANY({keep})) AS j FROM "{table}" t) s""")
    rows = sql(" UNION ALL ".join(parts))
    return dict(row.split("|", 1) for row in rows.splitlines())


def rust_only_data(sql, *, with_freeze_table: bool) -> dict[str, int]:
    """The refusal conditions the switch pack reads before it runs the downgrade."""
    queries = {
        "outbox_producer_evidence": "SELECT count(*) FROM market_event_outbox WHERE producer_evidence IS NOT NULL",
        "watchlist_events_rust_market_v1": "SELECT count(*) FROM watchlist_events WHERE producer = 'rust-market-v1'",
    }
    if with_freeze_table:
        queries["trade_freezes"] = "SELECT count(*) FROM trade_freezes"
    return {name: int(sql(query)) for name, query in queries.items()}


# -- case 1 and 2 seeds ----------------------------------------------------------


def seed_rows_written_before_p89(sql):
    """Rows that exist at p5: parties, a trade, a legacy watchlist event, outbox, audit, receipt."""
    seed_trade(sql)
    seed_target(sql)
    sql(insert_event())
    sql(f"""
        INSERT INTO market_event_outbox (id,event_type,aggregate_type,aggregate_id,payload,participant_org_ids)
        VALUES ('{uuid4()}','trade.updated','trade','{TRADE}','{{"schema_version":1}}','["{BUYER}"]');
        INSERT INTO audit_logs (id,action,resource_type,resource_id)
        VALUES ('{uuid4()}','TRADE_CREATED','trade','{TRADE}');
    """)
    sql(receipt_insert("legacy-receipt", "trade.confirm", TRADE))


def receipt_insert(key: str, operation: str, resource_id: str) -> str:
    return f"""INSERT INTO market_command_results
        (id,actor_user_id,effective_organization_id,operation,idempotency_key,request_hash,
         resource_type,resource_id,response_status,response_body)
        VALUES ('{uuid4()}','{ACTOR}','{BUYER}','{operation}','{key}','{"0" * 64}',
                'trade','{resource_id}',200,'{{"stored":true}}')"""


def write_as_python_would_at_the_p89_schema(sql):
    """Writes the Python routes keep making after the schema upgrade (no p89 column named)."""
    insert_trade(sql, SECOND_TRADE)
    sql(insert_event(event=LEGACY_EVENT))
    sql(f"""
        INSERT INTO market_event_outbox (id,event_type,aggregate_type,aggregate_id,payload,participant_org_ids)
        VALUES ('{uuid4()}','trade.updated','trade','{SECOND_TRADE}','{{"schema_version":1}}','["{SELLER}"]');
        INSERT INTO audit_logs (id,action,resource_type,resource_id)
        VALUES ('{uuid4()}','TRADE_CREATED','trade','{SECOND_TRADE}');
    """)
    sql(receipt_insert("receipt-at-p89", "trade.pay", SECOND_TRADE))


# -- case 1 ----------------------------------------------------------------------


@pytest.mark.parametrize("start", [FREEZE, PRODUCER])
def test_downgrade_to_p5_is_lossless_before_any_rust_write(supplier_scratch_database, start):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", P5)
    p5_schema = schema_fingerprint(sql)
    p5_columns = table_columns(sql)
    seed_rows_written_before_p89(sql)

    migrate("upgrade", start)
    write_as_python_would_at_the_p89_schema(sql)
    # The consumer watermark is the one row that exists only at the p89 schema;
    # it lives in a table the downgrade drops and carries no user data.
    sql("INSERT INTO watchlist_producer_state (singleton) VALUES (true)")
    assert sql("SELECT version_num FROM alembic_version") == start
    assert rust_only_data(sql, with_freeze_table=start == FREEZE) == {
        name: 0 for name in rust_only_data(sql, with_freeze_table=start == FREEZE)
    }
    before = data_snapshot(sql, p5_columns)
    assert int(before["trades"].split(":")[0]) == 2
    assert int(before["watchlist_events"].split(":")[0]) == 2
    assert int(before["market_event_outbox"].split(":")[0]) == 2
    assert int(before["market_command_results"].split(":")[0]) == 2

    migrate("downgrade", P5)

    assert sql("SELECT version_num FROM alembic_version") == P5
    assert schema_fingerprint(sql) == p5_schema
    assert table_columns(sql) == p5_columns
    assert data_snapshot(sql, p5_columns) == before
    for relation in ("watchlist_producer_state", "trade_freezes"):
        assert sql(f"SELECT to_regclass('{relation}') IS NULL") == "t"

    # The lossless schema is a working p5 database: the forward path still applies.
    migrate("upgrade", FREEZE)
    assert sql("SELECT version_num FROM alembic_version") == FREEZE
    assert sql("SELECT count(*) FROM watchlist_events WHERE producer = 'legacy-python'") == "2"


# -- case 2 ----------------------------------------------------------------------


def write_producer_evidence_on_an_outbox_row(sql):
    sql(f"""INSERT INTO market_event_outbox
        (id,event_type,aggregate_type,aggregate_id,payload,participant_org_ids,producer_evidence)
        VALUES ('{SOURCE}','order.updated','order','schema-fixture','{{}}',
                '["{BUYER}"]','{{"version":1}}')""")


def write_a_trusted_watchlist_row(sql):
    sql(insert_event(identity=TRUSTED_COLUMNS, values=TRUSTED_VALUES, event=TRUSTED_EVENT))


def write_an_active_freeze(sql):
    sql(freeze_insert())


def write_a_cleared_freeze(sql):
    write_an_active_freeze(sql)
    clear_freeze(sql)


REFUSALS = {
    "producer_evidence": (write_producer_evidence_on_an_outbox_row, "outbox_producer_evidence",
                          "Retained producer evidence prevents downgrade"),
    "trusted_watchlist_row": (write_a_trusted_watchlist_row, "watchlist_events_rust_market_v1",
                              "Retained producer evidence prevents downgrade"),
    "active_freeze_row": (write_an_active_freeze, "trade_freezes",
                          "Retained trade freeze history prevents downgrade"),
    "cleared_freeze_row": (write_a_cleared_freeze, "trade_freezes",
                           "Retained trade freeze history prevents downgrade"),
}


@pytest.mark.parametrize("write", sorted(REFUSALS))
def test_downgrade_refuses_after_a_rust_write_or_a_freeze_row(supplier_scratch_database, write):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", FREEZE)
    writer, counted, message = REFUSALS[write]
    seed_rows_written_before_p89(sql)
    write_as_python_would_at_the_p89_schema(sql)
    # Ordinary Python-written rows sit beside the Rust-only data and must survive the refusal.
    writer(sql)

    precheck = rust_only_data(sql, with_freeze_table=True)
    assert precheck[counted] >= 1, precheck
    schema = schema_fingerprint(sql)
    columns = table_columns(sql)
    data = data_snapshot(sql, columns)

    refusal = migrate("downgrade", P5, success=False)

    assert message in refusal, refusal
    # One Alembic transaction: a refusal at the producer step also undoes the freeze step before it.
    assert sql("SELECT version_num FROM alembic_version") == FREEZE
    assert schema_fingerprint(sql) == schema
    assert table_columns(sql) == columns
    assert data_snapshot(sql, columns) == data
    assert rust_only_data(sql, with_freeze_table=True) == precheck


# -- case 3 ----------------------------------------------------------------------


def _decoded(node):
    """One tagged value as the Rust canonical form writes it into the hashed JSON."""
    kind = node["k"]
    if kind == "null":
        return None
    if kind in {"bool", "int", "text", "uuid", "date"}:
        return node["v"]
    if kind == "decimal":
        return format(Decimal(node["v"]).normalize(), "f")
    if kind == "timestamp":
        instant = datetime.fromisoformat(node["v"]).astimezone(UTC)
        return instant.isoformat().replace("+00:00", "Z")
    if kind == "object":
        return {name: _decoded(item) for name, item in node["v"].items()}
    if kind == "array":
        return [_decoded(item) for item in node["v"]]
    raise AssertionError(f"unknown tagged kind {kind}")


def rust_request_hash(tagged: dict) -> str:
    """SHA-256 of sorted, compact, ASCII-escaped JSON: the Rust boundary's receipt hash."""
    encoded = json.dumps(_decoded(tagged), sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def test_rust_hash_encoder_reproduces_the_shared_vectors():
    vectors = json.loads((ROOT / "tests/rust_contract/phase8/hash-vectors.json").read_text())
    assert vectors["command_request_hashes"]
    for vector in vectors["command_request_hashes"]:
        assert rust_request_hash(vector["payload"]) == vector["sha256"], vector["label"]


def tagged_command(operation: str, resource: str, **fields) -> dict:
    """A tagged request payload: text and uuid fields as given, decimals and instants tagged."""
    resource_name = "order_id" if operation.startswith("order.") else "trade_id"
    body = {"operation": {"k": "text", "v": operation}, resource_name: {"k": "uuid", "v": str(resource)}}
    body.update(fields)
    return {"k": "object", "v": body}


async def market_state(seeded) -> dict[str, str]:
    """Counts of every table a market command touches plus digests of the economic rows."""
    queries = {
        "audit_logs": "SELECT count(*) FROM audit_logs",
        "outbox": "SELECT count(*) FROM market_event_outbox",
        "notifications": "SELECT count(*) FROM notifications",
        "receipts": "SELECT count(*) FROM market_command_results",
        "freezes": "SELECT md5(coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY id::text), '')) FROM trade_freezes t",
        "trades": "SELECT md5(coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY id::text), '')) FROM trades t",
        "orders": "SELECT md5(coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY id::text), '')) FROM orderbook_orders t",
        "inventory": "SELECT md5(coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY id::text), '')) FROM inventory_items t",
    }
    state = {}
    async with seeded["factory"]() as session:
        for name, query in queries.items():
            state[name] = str((await session.execute(text(query))).scalar_one())
    return state


async def publish_listing(client, seeded, key: str) -> str:
    published = await client.post(
        f"/api/inventory/{seeded['inventory_id']}/publish",
        headers=_headers(seeded["seller_id"], key),
    )
    assert published.status_code == 200, published.text
    return published.json()["listing_id"]


async def create_trade(client, seeded, listing_id: str, key: str, quantity: str) -> str:
    response = await client.post(
        "/api/trades/",
        json={
            "order_id": listing_id,
            "quantity_mt": quantity,
            "expected_terms_digest": await _reviewed_terms_digest(seeded, listing_id),
        },
        headers=_headers(seeded["buyer_id"], key),
    )
    assert response.status_code == 200, response.text
    return response.json()["id"]


async def test_python_replays_a_rust_receipt_or_answers_409(route_market):
    client, seeded = route_market
    seller, buyer = seeded["seller_id"], seeded["buyer_id"]
    seller_org, buyer_org = seeded["seller_org_id"], seeded["buyer_org_id"]

    # Resources in the state a Rust command would have left them in.
    listing_id = await publish_listing(client, seeded, "rb-publish")
    shipped = await create_trade(client, seeded, listing_id, "rb-trade-one", "50.00")
    refused = await create_trade(client, seeded, listing_id, "rb-trade-two", "25.00")
    confirmed = await client.put(f"/api/trades/{shipped}/confirm", headers=_headers(seller, "rb-py-confirm"))
    assert confirmed.status_code == 200, confirmed.text
    assert (await client.put(
        f"/api/trades/{shipped}/deliver",
        json={"final_quantity_mt": "40.00", "final_price_per_mt": "700.00"},
        headers=_headers(seller, "rb-py-deliver"),
    )).status_code == 200
    assert (await client.post(f"/api/trades/{shipped}/pay", headers=_headers(seller, "rb-py-pay"))).status_code == 200
    assert (await client.put(f"/api/trades/{refused}/decline", headers=_headers(seller, "rb-py-decline"))).status_code == 200
    created = await client.post(
        "/api/orderbook",
        json={
            "side": "BID", "product_id": str(seeded["product_id"]),
            "delivery_point_id": str(seeded["point_id"]), "quantity_mt": "200.00",
            "price_per_mt_usd": "600.00", "availability_window": "SPOT",
        },
        headers=_headers(buyer, "rb-order-create"),
    )
    assert created.status_code == 201, created.text
    order = created.json()["id"]
    assert (await client.delete(f"/api/orderbook/{order}", headers=_headers(buyer, "rb-py-cancel"))).status_code == 204

    # One entry per receipt-backed operation: the actor, the tagged request Rust hashes,
    # the stored result, the other party's organization, and the Python call that retries it.
    amend_expiry = (datetime.now(UTC) + timedelta(hours=1)).replace(microsecond=0)
    amend_json = {
        "price_per_mt_usd": "610.00",
        "expires_at": amend_expiry.astimezone(timezone(timedelta(hours=8))).isoformat(),
    }
    amend_fields = {"update": {"k": "object", "v": {
        "price_per_mt_usd": {"k": "decimal", "v": "610.00"},
        "expires_at": {"k": "timestamp", "v": amend_expiry.isoformat()},
    }}}
    deliver_fields = {
        "final_quantity_mt": {"k": "decimal", "v": "40.00"},
        "final_price_per_mt": {"k": "decimal", "v": "700.00"},
    }
    cancel_fields = {"reason": {"k": "null"}, "if_match": {"k": "null"}}
    operations = {
        "trade.confirm": dict(actor=seller, org=seller_org, other_org=buyer_org, resource=shipped, other=refused,
                              fields={}, status=200,
                              call=lambda headers, resource: client.put(f"/api/trades/{resource}/confirm", headers=headers)),
        "trade.decline": dict(actor=seller, org=seller_org, other_org=buyer_org, resource=refused, other=shipped,
                              fields={}, status=200,
                              call=lambda headers, resource: client.put(f"/api/trades/{resource}/decline", headers=headers)),
        "trade.deliver": dict(actor=seller, org=seller_org, other_org=buyer_org, resource=shipped, other=refused,
                              fields=deliver_fields, status=200,
                              call=lambda headers, resource: client.put(
                                  f"/api/trades/{resource}/deliver", headers=headers,
                                  json={"final_quantity_mt": "40.00", "final_price_per_mt": "700.00"})),
        "trade.pay": dict(actor=seller, org=seller_org, other_org=buyer_org, resource=shipped, other=refused,
                          fields={}, status=200,
                          call=lambda headers, resource: client.post(f"/api/trades/{resource}/pay", headers=headers)),
        "order.amend": dict(actor=buyer, org=buyer_org, other_org=seller_org, resource=order, other=str(uuid4()),
                            fields=amend_fields, status=200,
                            call=lambda headers, resource: client.put(
                                f"/api/orderbook/{resource}", headers=headers, json=amend_json)),
        "order.cancel": dict(actor=buyer, org=buyer_org, other_org=seller_org, resource=order, other=str(uuid4()),
                             fields=cancel_fields, status=204,
                             call=lambda headers, resource: client.delete(f"/api/orderbook/{resource}", headers=headers)),
    }
    assert set(operations) == {
        "trade.confirm", "trade.decline", "trade.deliver", "trade.pay", "order.amend", "order.cancel",
    }

    def stored_body(operation: str, spec: dict):
        if spec["status"] == 204:
            return None
        return {"id": str(spec["resource"]), "operation": operation, "served_by": "market-write-boundary"}

    async with seeded["factory"]() as session:
        for operation, spec in operations.items():
            for suffix, organization in (("", spec["org"]), ("-other-organization", spec["other_org"])):
                session.add(MarketCommandResult(
                    actor_user_id=spec["actor"], effective_organization_id=organization,
                    support_context_id=None, operation=operation, idempotency_key=f"rust-{operation}{suffix}",
                    request_hash=rust_request_hash(tagged_command(operation, spec["resource"], **spec["fields"])),
                    resource_type="order" if operation.startswith("order.") else "trade",
                    resource_id=spec["resource"], response_status=spec["status"],
                    response_body=stored_body(operation, spec),
                ))
        await session.commit()

    before = await market_state(seeded)
    observed: dict[str, dict[str, str]] = {}
    for operation, spec in operations.items():
        headers = lambda key, actor=spec["actor"]: _headers(actor, key)
        replay = await spec["call"](headers(f"rust-{operation}"), spec["resource"])
        other_organization = await spec["call"](headers(f"rust-{operation}-other-organization"), spec["resource"])
        other_request = await spec["call"](headers(f"rust-{operation}"), spec["other"])
        if operation == "trade.deliver":
            other_request = await client.put(
                f"/api/trades/{spec['resource']}/deliver", headers=headers(f"rust-{operation}"),
                json={"final_quantity_mt": "41.00", "final_price_per_mt": "700.00"},
            )
        if operation == "order.cancel":
            other_request = await client.delete(
                f"/api/orderbook/{spec['resource']}",
                headers={**headers(f"rust-{operation}"), "If-Match": '"another-version"'},
            )
        observed[operation] = {
            "same_request": f"{replay.status_code} stored result" if replay.status_code == spec["status"]
            else f"{replay.status_code} {replay.text[:80]}",
            "other_request": str(other_request.status_code),
            "other_organization": str(other_organization.status_code),
        }

        # A keyed retry of a request Rust completed returns the stored result or 409; it never runs again.
        if replay.status_code == spec["status"]:
            if spec["status"] == 204:
                assert replay.content == b"", operation
            else:
                assert replay.json() == stored_body(operation, spec), operation
        else:
            assert replay.status_code == 409, (operation, replay.status_code, replay.text)
        assert other_request.status_code == 409, (operation, other_request.text)
        assert other_organization.status_code == 409, (operation, other_organization.text)

    print("ROLLBACK_REPLAY_OBSERVED " + json.dumps(observed, sort_keys=True))
    assert await market_state(seeded) == before
    # The Rust-hash receipts and Python's own hash agree, so the stored result is replayed.
    assert all(result["same_request"].endswith("stored result") for result in observed.values()), observed


# -- case 4 ----------------------------------------------------------------------


async def seed_admin(seeded) -> str:
    async with seeded["factory"]() as session:
        admin = User(
            email=f"freeze-admin-{uuid4()}@rollback.test", password_hash="unused",
            role=UserRole.ADMIN, status=UserStatus.APPROVED, email_verified=True,
        )
        session.add(admin)
        await session.commit()
        return str(admin.id)


async def freeze_trade(seeded, admin: str, trade_id: str):
    async with seeded["factory"]() as session:
        await session.execute(text(
            "INSERT INTO trade_freezes (id,trade_id,status,reason_code,note,frozen_by_user_id) "
            "VALUES (:id,:trade,'ACTIVE','OTHER','Review required',:admin)"
        ), {"id": str(uuid4()), "trade": trade_id, "admin": admin})
        await session.commit()


async def clear_trade_freeze(seeded, admin: str, trade_id: str):
    async with seeded["factory"]() as session:
        await session.execute(text(
            "UPDATE trade_freezes SET status='CLEARED', cleared_at=now(), cleared_by_user_id=:admin, "
            "clear_note='Review complete', version=version+1 WHERE trade_id=:trade AND status='ACTIVE'"
        ), {"admin": admin, "trade": trade_id})
        await session.commit()


async def test_python_settlement_on_a_frozen_trade_writes_nothing(route_market):
    client, seeded = route_market
    seller = seeded["seller_id"]
    admin = await seed_admin(seeded)
    listing_id = await publish_listing(client, seeded, "frozen-publish")
    settled = await create_trade(client, seeded, listing_id, "frozen-trade-one", "50.00")
    declined = await create_trade(client, seeded, listing_id, "frozen-trade-two", "25.00")
    deliver_json = {"final_quantity_mt": "40.00", "final_price_per_mt": "700.00"}
    steps = [
        # (action, key, request, status after the freeze is cleared, trade status afterwards)
        ("confirm", "frozen-confirm", lambda h: client.put(f"/api/trades/{settled}/confirm", headers=h), "CONFIRMED"),
        ("deliver", "frozen-deliver", lambda h: client.put(
            f"/api/trades/{settled}/deliver", headers=h, json=deliver_json), "DELIVERED"),
        ("pay", "frozen-pay", lambda h: client.post(f"/api/trades/{settled}/pay", headers=h), "PAID"),
    ]

    async def trade_status(trade_id: str) -> str:
        async with seeded["factory"]() as session:
            return (await session.execute(
                text("SELECT status FROM trades WHERE id=:id"), {"id": trade_id})).scalar_one()

    observed = {}
    for action, key, request, expected_status in steps:
        await freeze_trade(seeded, admin, settled)
        before = await market_state(seeded)
        keyed = await request(_headers(seller, key))
        unkeyed = await request({"Authorization": _headers(seller, key)["Authorization"]})
        observed[action] = [keyed.status_code, unkeyed.status_code]
        assert observed[action] == [500, 500], (action, keyed.text, unkeyed.text)
        # No status change, audit row, event, notification, stock change, freeze change or receipt.
        assert await market_state(seeded) == before, action
        assert await trade_status(settled) != expected_status

        # The refused attempt did not consume its idempotency key: the same key works after the clearance.
        await clear_trade_freeze(seeded, admin, settled)
        retried = await request(_headers(seller, key))
        assert retried.status_code == 200, (action, retried.text)
        assert retried.json()["status"] == expected_status
        assert await trade_status(settled) == expected_status

    # Decline is a settlement status too; an active freeze on the second trade stops it.
    await freeze_trade(seeded, admin, declined)
    before = await market_state(seeded)
    refused = await client.put(f"/api/trades/{declined}/decline", headers=_headers(seller, "frozen-decline"))
    observed["decline"] = [refused.status_code]
    assert refused.status_code == 500, refused.text
    assert await market_state(seeded) == before
    assert await trade_status(declined) == "PENDING_CONFIRMATION"

    print("ROLLBACK_FROZEN_OBSERVED " + json.dumps(observed, sort_keys=True))
