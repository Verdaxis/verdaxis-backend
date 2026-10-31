"""Run Rust alert storage tests on the existing disposable PostgreSQL fixture.

This is component interoperability, not the paired HTTP/state runner gate.
"""
import os
import subprocess
import sys
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy import text

ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.asyncio
async def test_rust_alert_crud_and_python_trigger(pg_session):
    _, session = pg_session
    organization, foreign, product, point = (uuid4() for _ in range(4))
    await session.execute(text(
        "INSERT INTO organizations (id,name,type) VALUES (:id,'Alert component owner','FUEL_BUYER'),"
        "(:foreign,'Alert foreign owner','FUEL_BUYER')"
    ), {"id": organization, "foreign": foreign})
    await session.execute(text(
        "INSERT INTO products (id,name,fuel_type,fuel_grade,is_active) "
        "VALUES (:id,'Alert legacy fixture','TEST','TEST',false)"
    ), {"id": product})
    await session.execute(text(
        "INSERT INTO delivery_points (id,name,region,is_active) "
        "VALUES (:id,'Alert fixture point','TEST',false)"
    ), {"id": point})
    actors = [uuid4() for _ in range(4)]
    for index, actor in enumerate(actors):
        await session.execute(text(
            "INSERT INTO users (id,email,password_hash,role,status,organization_id,email_verified,must_change_password) "
            "VALUES (:id,:email,'unused','BUYER','APPROVED',:org,true,false)"
        ), {"id": actor, "email": f"alert-{actor}@example.test", "org": [organization, organization, foreign, None][index]})
    await session.commit()
    from sqlalchemy.ext.asyncio import create_async_engine
    runtime = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with runtime.begin() as connection:
            assert await connection.scalar(text("SELECT has_function_privilege(current_user, 'pg_catalog.pg_advisory_xact_lock(bigint)', 'EXECUTE')")), "runtime role needs EXECUTE on pg_catalog.pg_advisory_xact_lock(bigint)"
            await connection.execute(text("SELECT pg_catalog.pg_advisory_xact_lock(123456789::bigint)"))
    finally:
        await runtime.dispose()
    env = {
        **os.environ,
        "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "RUST_TEST_PYTHON": sys.executable,
        "SQLX_OFFLINE": "true",
        "ALERT_TEST_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgresql://"),
        "ALERT_TEST_ORGANIZATION": str(organization),
        "ALERT_TEST_FOREIGN": str(foreign),
        "ALERT_TEST_PRODUCT": str(product),
        "ALERT_TEST_POINT": str(point),
        "ALERT_TEST_ACTORS": ",".join(map(str, actors)),
    }
    subprocess.run([
        "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
        "--test", "crud_alerts", "postgres_crud", "--", "--nocapture",
    ], cwd=ROOT, env=env, check=True, timeout=600)
    from decimal import Decimal

    from app.services.activity import check_price_alerts

    triggered = await check_price_alerts(session, product, point, Decimal(100))
    assert len(triggered) == 1
    event = triggered[0]
    assert event.event_type == "price_alert_triggered"
    assert event.aggregate_type == "price_alert"
    assert event.participant_org_ids == (organization,)
    assert event.payload == {
        "schema_version": 1,
        "alert_id": event.aggregate_id,
        "product_id": str(product),
        "direction": "above",
        "threshold_usd": "12.35",
        "triggered_price": "100",
    }
    await session.commit()
    row = (await session.execute(text(
        "SELECT id,is_active,triggered_at FROM price_alerts WHERE org_id=:org"
    ), {"org": organization})).one()
    assert row.is_active is False
    assert row.triggered_at is not None
    assert event.aggregate_id == str(row.id)
