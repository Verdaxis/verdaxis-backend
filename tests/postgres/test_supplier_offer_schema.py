"""PostgreSQL constraints and runtime authority for declared supplier offers."""
from __future__ import annotations

from datetime import UTC, datetime, timedelta
from decimal import Decimal
import json
import os
from uuid import UUID, uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine


_UCOME = UUID("e561e43f-d9b2-598e-981c-f1d28d515ddc")
_SINGAPORE = UUID("73835e92-820e-584b-8280-bb61c63aa28e")
_JSON_COLUMNS = {"listing_terms", "contract_terms", "source_offer_snapshot"}
_MUTABLE_OFFER_COLUMNS = {
    "quantity_mt", "min_fill_mt", "price_per_mt_usd", "availability_window",
    "listing_terms", "status", "revision", "expires_at", "updated_at",
}
_SOURCE_COLUMNS = {"source_offer_id", "target_supplier_org_id", "source_offer_snapshot"}


@pytest.fixture
async def offer_database(market_pg):
    app_url = os.environ.get("DATABASE_URL", "")
    app_role = os.environ.get("RUNTIME_TEST_APP_ROLE", "")
    if not app_url or not app_role:
        pytest.skip("the PostgreSQL runtime app role is not configured")
    runtime_url = make_url(app_url).set(database=market_pg.url.database)
    assert runtime_url.username == app_role
    runtime_engine = create_async_engine(runtime_url, hide_parameters=True)
    rows = {
        "owner": market_pg,
        "runtime": runtime_engine,
        "supplier_org": uuid4(),
        "supplier_user": uuid4(),
        "other_org": uuid4(),
        "other_user": uuid4(),
        "buyer_org": uuid4(),
        "buyer_user": uuid4(),
        "other_product": uuid4(),
        "other_point": uuid4(),
    }
    try:
        async with runtime_engine.connect() as connection:
            role = (await connection.execute(text(
                "SELECT current_user, rolsuper FROM pg_roles WHERE rolname = current_user"
            ))).one()
            assert role == (app_role, False)
        async with market_pg.begin() as connection:
            for actor, role, organization_type in (
                ("supplier", "SUPPLIER", "FUEL_SUPPLIER"),
                ("other", "SUPPLIER", "FUEL_SUPPLIER"),
                ("buyer", "BUYER", "FUEL_BUYER"),
            ):
                await connection.execute(text(
                    "INSERT INTO organizations (id, name, type) VALUES (:id, :name, :type)"
                ), {"id": rows[f"{actor}_org"], "name": f"Offer {actor}", "type": organization_type})
                await connection.execute(text(
                    "INSERT INTO users (id, organization_id, email, password_hash, role) "
                    "VALUES (:id, :organization, :email, 'unused', :role)"
                ), {
                    "id": rows[f"{actor}_user"], "organization": rows[f"{actor}_org"],
                    "email": f"{uuid4()}@supplier-offer.test", "role": role,
                })
            await connection.execute(text(
                "INSERT INTO products (id, name, fuel_type, fuel_grade, unit, is_active) VALUES "
                "(:ucome, 'UCOME B100', 'FAME', 'UCOME', 'MT', true), "
                "(:other, 'Other product', 'Methanol', 'Bio', 'MT', true)"
            ), {"ucome": _UCOME, "other": rows["other_product"]})
            await connection.execute(text(
                "INSERT INTO delivery_points (id, name, region, timezone, is_active) VALUES "
                "(:singapore, 'Singapore', 'Asia', 'Asia/Singapore', true), "
                "(:other, 'Other point', 'Asia', 'Asia/Shanghai', true)"
            ), {"singapore": _SINGAPORE, "other": rows["other_point"]})
        yield rows
    finally:
        await runtime_engine.dispose()


def _offer_values(rows, **overrides):
    now = datetime.now(UTC)
    return {
        "id": uuid4(), "supplier_org_id": rows["supplier_org"],
        "supplier_user_id": rows["supplier_user"], "product_id": _UCOME,
        "delivery_point_id": _SINGAPORE, "quantity_mt": Decimal("100.00"),
        "min_fill_mt": Decimal("10.00"), "price_per_mt_usd": Decimal("1100.00"),
        "availability_window": "SPOT", "listing_terms": {"schema_version": 1},
        "expires_at": now + timedelta(days=1), "created_at": now, "updated_at": now,
        "idempotency_key": None, "idempotency_request_hash": None, **overrides,
    }


def _rfq_values(rows, **overrides):
    return {
        "id": uuid4(), "buyer_org_id": rows["buyer_org"],
        "buyer_user_id": rows["buyer_user"], "product_id": _UCOME,
        "delivery_point_id": _SINGAPORE, "quantity_mt": Decimal("100.00"),
        "availability_window": "SPOT", "contract_terms": {"schema_version": 1},
        "status": "OPEN", "is_anonymous": False,
        "expires_at": datetime.now(UTC) + timedelta(days=1), "created_at": datetime.now(UTC),
        "source_offer_id": None, "target_supplier_org_id": None,
        "source_offer_snapshot": None, **overrides,
    }


async def _insert(engine, table, values):
    # All table and column identifiers come from test constants. JSON None must
    # bind as SQL NULL so completeness constraints are exercised correctly.
    parameters = {
        column: json.dumps(value) if column in _JSON_COLUMNS and value is not None else value
        for column, value in values.items()
    }
    placeholders = [
        f"CAST(:{column} AS json)" if column in _JSON_COLUMNS else f":{column}"
        for column in values
    ]
    async with engine.begin() as connection:
        return (await connection.execute(text(
            f"INSERT INTO {table} ({', '.join(values)}) "
            f"VALUES ({', '.join(placeholders)}) RETURNING *"
        ), parameters)).mappings().one()


async def _permission_denied(engine, statement, parameters):
    with pytest.raises(DBAPIError) as error:
        async with engine.begin() as connection:
            await connection.execute(text(statement), parameters)
    assert error.value.orig.sqlstate == "42501"


@pytest.mark.asyncio
async def test_runtime_can_create_revise_and_withdraw_offer(offer_database):
    rows = offer_database
    offer = await _insert(rows["runtime"], "supplier_offers", _offer_values(rows))
    assert (offer["status"], offer["revision"]) == ("OPEN", 1)
    revised_terms = {"schema_version": 1, "batch_reference": "REVISED-BATCH"}
    async with rows["runtime"].begin() as connection:
        updated = (await connection.execute(text(
            "UPDATE supplier_offers SET quantity_mt = 120, min_fill_mt = 20, "
            "price_per_mt_usd = 1125, availability_window = '2026-10', "
            "listing_terms = CAST(:terms AS json), revision = revision + 1, "
            "expires_at = expires_at + interval '1 day', updated_at = :updated "
            "WHERE id = :id RETURNING *"
        ), {"id": offer["id"], "terms": json.dumps(revised_terms), "updated": datetime.now(UTC)})).mappings().one()
        assert (updated["quantity_mt"], updated["min_fill_mt"], updated["price_per_mt_usd"]) == (
            Decimal("120"), Decimal("20"), Decimal("1125"),
        )
        assert updated["listing_terms"] == revised_terms
        assert updated["revision"] == 2
        assert updated["expires_at"] == offer["expires_at"] + timedelta(days=1)
        await connection.execute(text(
            "UPDATE supplier_offers SET status = 'WITHDRAWN', revision = revision + 1, "
            "updated_at = now() WHERE id = :id"
        ), {"id": offer["id"]})
        stored = (await connection.execute(text(
            "SELECT status, revision, listing_terms FROM supplier_offers WHERE id = :id"
        ), {"id": offer["id"]})).one()
        assert stored == ("WITHDRAWN", 3, revised_terms)


@pytest.mark.asyncio
@pytest.mark.parametrize(("quantity", "price"), [
    (Decimal("1.00"), Decimal("0.01")),
    (Decimal("100000.00"), Decimal("1000000.00")),
])
async def test_runtime_can_insert_exact_numeric_limits(offer_database, quantity, price):
    rows = offer_database
    offer = await _insert(rows["runtime"], "supplier_offers", _offer_values(
        rows, quantity_mt=quantity, min_fill_mt=quantity, price_per_mt_usd=price,
        status="OPEN", revision=1,
    ))
    assert (offer["quantity_mt"], offer["min_fill_mt"], offer["price_per_mt_usd"]) == (
        quantity, quantity, price,
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(("column", "value", "constraint"), [
    ("quantity_mt", Decimal("0"), "numeric_values"),
    ("quantity_mt", Decimal("100000.01"), "numeric_values"),
    ("quantity_mt", Decimal("100.001"), "numeric_values"),
    ("quantity_mt", Decimal("NaN"), "numeric_values"),
    ("min_fill_mt", Decimal("0"), "numeric_values"),
    ("min_fill_mt", Decimal("100.01"), "numeric_values"),
    ("min_fill_mt", Decimal("10.001"), "numeric_values"),
    ("price_per_mt_usd", Decimal("0"), "numeric_values"),
    ("price_per_mt_usd", Decimal("1000000.01"), "numeric_values"),
    ("price_per_mt_usd", Decimal("1.001"), "numeric_values"),
    ("price_per_mt_usd", Decimal("Infinity"), "numeric_values"),
    ("status", "ACCEPTED", "status"),
    ("revision", 0, "revision"),
])
async def test_direct_sql_rejects_invalid_offer_values(offer_database, column, value, constraint):
    rows = offer_database
    with pytest.raises(DBAPIError) as error:
        await _insert(rows["runtime"], "supplier_offers", _offer_values(rows, **{column: value}))
    assert error.value.orig.sqlstate == "23514"
    assert f"ck_supplier_offers_{constraint}" in str(error.value.orig)


@pytest.mark.asyncio
@pytest.mark.parametrize(("column", "other"), [
    ("product_id", "other_product"), ("delivery_point_id", "other_point"),
])
async def test_direct_sql_rejects_other_product_or_delivery_point(offer_database, column, other):
    rows = offer_database
    with pytest.raises(DBAPIError) as error:
        await _insert(rows["runtime"], "supplier_offers", _offer_values(rows, **{column: rows[other]}))
    assert error.value.orig.sqlstate == "23514"
    assert "ck_supplier_offers_catalog" in str(error.value.orig)


@pytest.mark.asyncio
async def test_offer_idempotency_requires_pair_and_is_scoped_to_supplier(offer_database):
    rows = offer_database
    paired = {"idempotency_key": "same-request", "idempotency_request_hash": "a" * 64}
    await _insert(rows["runtime"], "supplier_offers", _offer_values(rows, **paired))
    with pytest.raises(DBAPIError) as duplicate:
        await _insert(rows["runtime"], "supplier_offers", _offer_values(rows, **paired))
    assert duplicate.value.orig.sqlstate == "23505"
    assert "uq_supplier_offers_org_idempotency" in str(duplicate.value.orig)
    await _insert(rows["runtime"], "supplier_offers", _offer_values(
        rows, **paired, supplier_org_id=rows["other_org"], supplier_user_id=rows["other_user"],
    ))
    for missing in paired:
        with pytest.raises(DBAPIError) as incomplete:
            await _insert(rows["runtime"], "supplier_offers", _offer_values(
                rows, **{**paired, missing: None},
            ))
        assert incomplete.value.orig.sqlstate == "23514"
        assert "ck_supplier_offers_idempotency" in str(incomplete.value.orig)
    # A caller without a key can make more than one independent offer.
    for _ in range(2):
        await _insert(rows["runtime"], "supplier_offers", _offer_values(rows))


@pytest.mark.asyncio
async def test_runtime_cannot_rewrite_offer_identity_or_delete_history(offer_database):
    rows = offer_database
    offer = await _insert(rows["runtime"], "supplier_offers", _offer_values(
        rows, idempotency_key="original", idempotency_request_hash="b" * 64,
    ))
    for column in (
        "id", "supplier_org_id", "supplier_user_id", "product_id", "delivery_point_id",
        "idempotency_key", "idempotency_request_hash", "created_at",
    ):
        await _permission_denied(
            rows["runtime"], f"UPDATE supplier_offers SET {column} = {column} WHERE id = :id",
            {"id": offer["id"]},
        )
    await _permission_denied(
        rows["runtime"], "DELETE FROM supplier_offers WHERE id = :id", {"id": offer["id"]},
    )
    async with rows["runtime"].connect() as connection:
        stored = (await connection.execute(text(
            "SELECT * FROM supplier_offers WHERE id = :id"
        ), {"id": offer["id"]})).mappings().one()
        assert stored == offer


@pytest.mark.asyncio
async def test_runtime_supplier_offer_grants_are_explicit_and_future_columns_stay_closed(offer_database):
    rows = offer_database
    probe = "unreviewed_offer_probe"
    async with rows["owner"].begin() as connection:
        await connection.execute(text(f"ALTER TABLE supplier_offers ADD COLUMN {probe} text"))
    try:
        async with rows["runtime"].connect() as connection:
            table_grants = (await connection.execute(text(
                "SELECT has_table_privilege(current_user, 'supplier_offers', 'SELECT'), "
                "has_table_privilege(current_user, 'supplier_offers', 'INSERT'), "
                "has_table_privilege(current_user, 'supplier_offers', 'UPDATE'), "
                "has_table_privilege(current_user, 'supplier_offers', 'DELETE')"
            ))).one()
            assert table_grants == (True, False, False, False)
            grants = (await connection.execute(text(
                "SELECT attname, has_column_privilege(current_user, attrelid, attname, 'INSERT'), "
                "has_column_privilege(current_user, attrelid, attname, 'UPDATE') "
                "FROM pg_attribute WHERE attrelid = 'supplier_offers'::regclass "
                "AND attnum > 0 AND NOT attisdropped"
            ))).all()
            assert {name for name, _, _ in grants} == set(_offer_values(rows)) | {"status", "revision", probe}
            for name, can_insert, can_update in grants:
                assert can_insert == (name != probe), name
                assert can_update == (name in _MUTABLE_OFFER_COLUMNS), name
        for statement in (
            f"INSERT INTO supplier_offers ({probe}) VALUES ('unreviewed')",
            f"UPDATE supplier_offers SET {probe} = 'unreviewed' WHERE false",
        ):
            await _permission_denied(rows["runtime"], statement, {})
    finally:
        async with rows["owner"].begin() as connection:
            await connection.execute(text(f"ALTER TABLE supplier_offers DROP COLUMN {probe}"))


@pytest.mark.asyncio
async def test_rfq_source_is_complete_or_absent_and_cannot_be_rewritten(offer_database):
    rows = offer_database
    offer = await _insert(rows["runtime"], "supplier_offers", _offer_values(rows))
    source = {
        "source_offer_id": offer["id"], "target_supplier_org_id": rows["supplier_org"],
        "source_offer_snapshot": {"id": str(offer["id"]), "revision": 1},
    }
    untargeted = await _insert(rows["runtime"], "rfqs", _rfq_values(rows))
    assert all(untargeted[column] is None for column in _SOURCE_COLUMNS)
    targeted = await _insert(rows["runtime"], "rfqs", _rfq_values(rows, **source))
    assert {column: targeted[column] for column in _SOURCE_COLUMNS} == source
    for column in _SOURCE_COLUMNS:
        # Exercise both one missing field and one field present on its own.
        for partial in ({**source, column: None}, {column: source[column]}):
            with pytest.raises(DBAPIError) as error:
                await _insert(rows["runtime"], "rfqs", _rfq_values(rows, **partial))
            assert error.value.orig.sqlstate == "23514"
            assert "ck_rfqs_source_offer" in str(error.value.orig)
        await _permission_denied(
            rows["runtime"], f"UPDATE rfqs SET {column} = {column} WHERE id = :id",
            {"id": targeted["id"]},
        )
    for column in ("source_offer_id", "target_supplier_org_id"):
        with pytest.raises(DBAPIError) as missing_source:
            await _insert(rows["runtime"], "rfqs", _rfq_values(
                rows, **{**source, column: uuid4()},
            ))
        assert missing_source.value.orig.sqlstate == "23503"
        assert f"fk_rfqs_{column}" in str(missing_source.value.orig)
    # Cancelling the RFQ remains permitted and retains its source evidence.
    async with rows["runtime"].begin() as connection:
        stored = (await connection.execute(text(
            "UPDATE rfqs SET status = 'CANCELLED' WHERE id = :id "
            "RETURNING status, source_offer_id, target_supplier_org_id, source_offer_snapshot"
        ), {"id": targeted["id"]})).one()
        assert stored == ("CANCELLED", source["source_offer_id"], source["target_supplier_org_id"], source["source_offer_snapshot"])
