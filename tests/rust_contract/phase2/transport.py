"""Disposable-only write probe, never mounted by the application.

Exercises HTTP -> runtime-role transaction -> committed evidence. This is not a
ported CRUD route. Both language probes use the same controlled action ledger.
"""
import os
from uuid import UUID

import asyncpg

from scripts.rust_contract.phase2_state import validate_action

ACTOR = UUID("20000000-0000-4000-8000-000000000001")


async def execute(action):
    if os.environ.get("ENVIRONMENT") != "test" or not os.environ.get("DISPOSABLE_TEST_TOKEN"):
        raise RuntimeError("disposable action transport only")
    validate_action(action)
    connection = await asyncpg.connect(os.environ["PHASE2_DATABASE_URL"])
    transaction = connection.transaction()
    try:
        await transaction.start()
        actor = await connection.fetchrow("SELECT current_user, rolsuper, pg_get_userbyid(datdba)=current_user AS owner FROM pg_roles CROSS JOIN pg_database WHERE rolname=current_user AND datname=current_database()")
        if actor["rolsuper"] or actor["owner"]:
            raise RuntimeError("probe requires non-owner runtime role")
        # Transaction-local application clock. SQL now() is not overridden:
        # capture its interval separately, and use the explicit action clock
        # only for generated business timestamps in this test-only probe.
        await connection.execute("SELECT set_config('verdaxis.test_action_clock',$1,true)", action["now"])
        database_now = await connection.fetchval("SELECT now()")
        for index, raw_id in enumerate(action["ids"]):
            await connection.execute("INSERT INTO feedback_entries(id,user_id,message,page,created_at) VALUES($1,$2,$3,NULL,current_setting('verdaxis.test_action_clock')::timestamptz)", UUID(raw_id), ACTOR, action["referral_code"])
            if index == 0 and action["fault"] == "between-writes":
                await transaction.rollback()
                return {"outcome": "rolled-back", "ids": action["ids"], "now": action["now"], "referral_code": action["referral_code"]}, database_now
        if os.environ.get("PHASE2_EMIT_EVENTS") == "true":
            await connection.execute("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at) VALUES($1,'phase2_probe','feedback',$2,$3::json,$4::json,current_setting('verdaxis.test_action_clock')::timestamptz)", UUID(action["ids"][0]), action["ids"][0], '["10000000-0000-4000-8000-000000000001"]', '{"version":1}')
        if action["fault"] == "before-commit":
            await transaction.rollback()
            outcome = "rolled-back"
        else:
            await transaction.commit()
            outcome = "ambiguous" if action["fault"] == "after-commit" else "committed"
        return {"outcome": outcome, "ids": action["ids"], "now": action["now"], "referral_code": action["referral_code"]}, database_now
    finally:
        await connection.close()
