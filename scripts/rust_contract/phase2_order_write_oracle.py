"""Disposable boundary oracle: real Python lock/receipt helpers, no market route."""
from __future__ import annotations

import asyncio
import json
import os
from datetime import datetime
from uuid import UUID

import asyncpg
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from sqlalchemy import text
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.models.orderbook import OrderSide
from app.services.command_results import prepare_command_attempt
from app.services.market_transactions import retry_market_transaction
from app.services.market_locks import acquire_market_slice_lock, market_slice_lock_key

app = FastAPI()
ACTOR = UUID("20000000-0000-4000-8000-000000000001")
ORG = UUID("10000000-0000-4000-8000-000000000001")
NOW = datetime.fromisoformat("2026-10-19T00:00:00+00:00")
engine = create_async_engine(os.environ["DATABASE_URL"])
Session = async_sessionmaker(engine)


@app.get("/.well-known/verdaxis-disposable-test")
async def identity():
    return {"disposable": True, "token": os.environ["DISPOSABLE_TEST_TOKEN"]}


@app.post("/__contract__/order-write")
async def command(request: Request):
    if request.headers.get("x-disposable-token") != os.environ["DISPOSABLE_TEST_TOKEN"]:
        return JSONResponse({}, status_code=403)
    v = await request.json()
    identifier = UUID(v["id"])
    mode = v["mode"]
    key = market_slice_lock_key(side=OrderSide.BID, product_id=identifier,
                                delivery_point_id=None, availability_window="SPOT")
    blocker = None
    observed = False
    if mode in {"timeout", "ordered"}:
        blocker = await asyncpg.connect(os.environ["PHASE2_DATABASE_URL"])
        await blocker.execute("BEGIN")
        await blocker.execute("SELECT pg_advisory_xact_lock($1)", key)
    release = None
    if mode == "ordered":
        holder, blocker = blocker, None
        async def release_holder():
            nonlocal observed
            try:
                async with asyncio.timeout(5):
                    while not await holder.fetchval(
                        "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND classid::bigint=$1 AND objid::bigint=$2 AND NOT granted)",
                        (key & ((1 << 64)-1)) >> 32, key & 0xffffffff,
                    ):
                        await asyncio.sleep(0)
                observed = True
            finally:
                await holder.execute("ROLLBACK")
                await holder.close()
        release = asyncio.create_task(release_holder())
    status, replayed = 200, False
    attempts = 0
    async with Session() as db:
        @retry_market_transaction()
        async def execute(db):
            nonlocal attempts, replayed, status
            attempts += 1
            try:
                scope = dict(request.scope)
                scope["headers"] = [(b"idempotency-key", str(identifier).encode())]
                attempt = await prepare_command_attempt(db, Request(scope), actor_user_id=ACTOR,
                    effective_organization_id=ORG, support_context_id=None, operation="order.amend",
                    resource_type="order", resource_id=identifier, payload=str(identifier))
                if attempt.replay is not None:
                    replayed = True
                    await db.rollback()
                else:
                    await acquire_market_slice_lock(db, side=OrderSide.BID, product_id=identifier,
                        delivery_point_id=None, availability_window="SPOT")
                    bounds = (await db.execute(text("SELECT current_setting('lock_timeout'),current_setting('statement_timeout')"))).one()
                    assert tuple(bounds) == ("500ms", "5s")
                    await db.execute(text("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at) VALUES(:id,'phase2_boundary','boundary',:aggregate,CAST(:participants AS json),CAST(:payload AS json),CAST(:now AS timestamptz))"),
                        {"id": identifier, "aggregate":str(identifier), "participants":json.dumps([str(ORG)]), "payload":json.dumps({"id":str(identifier)}), "now":NOW})
                    if mode == "rollback":
                        await db.execute(text("SELECT 1/0"))
                    if (mode == "retry-40001" and attempts == 1) or mode == "retry-40P01-bound":
                        code = "40001" if mode == "retry-40001" else "40P01"
                        await db.execute(text("DO $$ BEGIN RAISE EXCEPTION 'scheduled retry' USING ERRCODE='" + code + "'; END $$"))
                    await db.execute(text("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body,created_at) VALUES(:id,:actor,:org,'order.amend',:key,:hash,'order',:id,200,CAST(:body AS jsonb),CAST(:now AS timestamptz))"),
                        {"id":identifier,"actor":ACTOR,"org":ORG,"key":str(identifier),"hash":attempt.request_hash,"body":json.dumps({"id":str(identifier)}),"now":NOW})
                    await db.commit()
                    if mode == "lost-response":
                        status = 503
            except Exception as error:
                from app.services.market_transactions import is_retryable_market_transaction_error
                if is_retryable_market_transaction_error(error):
                    raise
                from fastapi import HTTPException
                await db.rollback()
                status = error.status_code if isinstance(error, HTTPException) else 500
        try:
            await execute(db=db)
        except Exception as error:
            from fastapi import HTTPException
            status = error.status_code if isinstance(error, HTTPException) else 500
        finally:
            if blocker:
                await blocker.execute("ROLLBACK")
                await blocker.close()
    if release:
        await release
    return {"id":str(identifier),"status":status,"replayed":replayed,"attempts":attempts,"lock_observed":observed}


if __name__ == "__main__":
    import uvicorn
    assert os.environ["ENVIRONMENT"] == "test"
    bind = os.environ["PHASE2_BIND"]
    assert bind.startswith("127.0.0.1:")
    uvicorn.run(app, host="127.0.0.1", port=int(bind.rsplit(":", 1)[1]), log_level="warning")
