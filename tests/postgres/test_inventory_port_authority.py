"""Inventory reference reads and FK races under the unchanged runtime ACL."""
import asyncio
import os
from uuid import uuid4

import httpx
import pytest
from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import create_access_token
from app.database import get_db
from app.main import app
from tests.postgres.retention_test_support import reset_outbox
from tests.postgres.test_market_routes import _seed_route_market


@pytest.mark.asyncio
@pytest.mark.parametrize("case", ["existing", "missing", "deleted_after_read"])
async def test_inventory_port_http_as_runtime_role(market_pg, market_pg_url, case):
    seeded = await _seed_route_market(market_pg)
    port_id = "authority-" + uuid4().hex[:12]
    async with market_pg.begin() as connection:
        await reset_outbox(connection)
        await connection.execute(text(
            "INSERT INTO ports (id,name,country,location,is_active) VALUES "
            "(:id,'Singapore','Singapore',ST_GeogFromText('SRID=4326;POINT(103.8 1.3)'),true)"
        ), {"id": port_id})
    url = make_url(os.environ["DATABASE_URL"]).set(database=make_url(market_pg_url).database)
    engine = create_async_engine(url)
    read_finished = asyncio.Event()
    deletion_finished = asyncio.Event()

    class ReferenceRaceSession(AsyncSession):
        async def scalar(self, statement, *args, **kwargs):
            value = await super().scalar(statement, *args, **kwargs)
            if case == "deleted_after_read" and "SELECT ports.id" in str(statement):
                read_finished.set()
                await asyncio.wait_for(deletion_finished.wait(), 10)
            return value

    factory = async_sessionmaker(engine, class_=ReferenceRaceSession, expire_on_commit=False)
    async def override_db():
        async with factory() as session:
            yield session

    async def delete_reference():
        await asyncio.wait_for(read_finished.wait(), 10)
        async with market_pg.begin() as connection:
            await connection.execute(text("DELETE FROM ports WHERE id=:id"), {"id": port_id})
        deletion_finished.set()

    deletion = None
    try:
        async with engine.connect() as connection:
            rights = (await connection.execute(text(
                "SELECT current_user, has_table_privilege(current_user,'ports','SELECT'), "
                "has_table_privilege(current_user,'ports','UPDATE')"
            ))).one()
            assert rights == (os.environ["RUNTIME_TEST_APP_ROLE"], True, False)
        if case == "deleted_after_read":
            deletion = asyncio.create_task(delete_reference())
        app.dependency_overrides[get_db] = override_db
        token = create_access_token(str(seeded["seller_id"]))
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app, raise_app_exceptions=False), base_url="http://test") as client:
            response = await client.post("/api/inventory", headers={"Authorization": f"Bearer {token}"}, json={
                "port_id": port_id if case != "missing" else "absent-port",
                "fuel_type": "Methanol", "current_stock_mt": "1000",
            })
        assert response.status_code == (200 if case == "existing" else 404), response.text
        if case != "existing":
            assert response.json() == {"detail": "Port not found"}
        if deletion:
            await deletion
        async with market_pg.connect() as connection:
            count = await connection.scalar(text("SELECT count(*) FROM inventory_items WHERE port_id=:id"), {"id": port_id})
            events = await connection.scalar(text("SELECT count(*) FROM market_event_outbox WHERE event_type='inventory_created'"))
            assert count == (1 if case == "existing" else 0)
            assert events == (1 if case == "existing" else 0)
    finally:
        app.dependency_overrides.pop(get_db, None)
        if deletion and not deletion.done():
            deletion.cancel()
        if deletion:
            await asyncio.gather(deletion, return_exceptions=True)
        await engine.dispose()
