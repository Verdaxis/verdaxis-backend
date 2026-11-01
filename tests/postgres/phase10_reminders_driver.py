#!/usr/bin/env python3
"""Run the retained Python order-expiry reminder functions once.

The Rust target ``rs/read/tests/phase10_reminders_postgres.rs`` runs this
helper as a child process to prove that Python and Rust reminders share one
duplicate identity, one scan key and one delivery contract while both exist.

Argument 1 is one JSON command:

- ``{"command": "scan", "now": iso}``: ``remind_expiring_orders`` and commit;
- ``{"command": "deliver", "now": iso, "accept": bool}``:
  ``deliver_expiry_reminder_emails`` with the provider call replaced by a
  recorder that returns ``accept``;
- ``{"command": "payload", "email": str, "message": str}``:
  ``build_order_expiry_email_payload``.

The last stdout line is ``RESULT <json>``. The service functions run
unmodified; only the email sender is replaced. The database URL comes from
``DATABASE_URL`` and must be a loopback ``*_test`` database.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# Run as a script, Python puts only this folder on the path; the application
# package lives at the repository root.
sys.path.insert(0, str(ROOT))


def checked_url(raw: str):
    from sqlalchemy.engine import make_url

    url = make_url(raw)
    if url.host != "127.0.0.1" or not (url.database or "").endswith("_test"):
        raise SystemExit("refusing a database that is not a loopback *_test database")
    if url.drivername != "postgresql+asyncpg":
        raise SystemExit("the driver needs a postgresql+asyncpg URL")
    return url


async def run(command: dict) -> object:
    from app.services import order_expiry_reminders as reminders
    from app.services.email import build_order_expiry_email_payload

    kind = command["command"]
    if kind == "payload":
        return build_order_expiry_email_payload(command["email"], command["message"])

    from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

    calls: list[dict] = []

    async def record(payload, notification_id):
        # The production sender derives the provider key the same way
        # (app/services/email.py, send_order_expiry_email).
        calls.append({"payload": payload, "key": f"order-expiry/{notification_id}"})
        return bool(command.get("accept", True))

    reminders.send_order_expiry_email = record
    now = datetime.fromisoformat(command["now"]).astimezone(UTC)
    engine = create_async_engine(checked_url(os.environ["DATABASE_URL"]))
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as db:
            if kind == "scan":
                created = await reminders.remind_expiring_orders(db, now=now)
                await db.commit()
                return {"created": created}
            if kind == "deliver":
                sent = await reminders.deliver_expiry_reminder_emails(db, now=now)
                return {"sent": sent, "calls": calls}
    finally:
        await engine.dispose()
    raise SystemExit(f"unknown command {kind!r}")


def main() -> None:
    result = asyncio.run(run(json.loads(sys.argv[1])))
    print("RESULT " + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
