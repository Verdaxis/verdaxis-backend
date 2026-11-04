"""Cross-stack harness for the account credential cutoff tests.

The Rust tests in ``offboarding_credentials.rs`` and
``offboarding_credentials_postgres.rs`` start this script to run the unchanged
Python application code against the same disposable database. It refuses to
run unless ``ENVIRONMENT=test``.

Usage: ``python offboarding_python_harness.py <command>``. Every command except
``serve`` reads one JSON object on stdin and prints one line
``OFFBOARDING_RESULT <json>`` on stdout.

- ``serve``: run the Python application on ``127.0.0.1`` and the port in
  ``OFFBOARDING_HARNESS_PORT``. One extra route, guarded by the token in
  ``OFFBOARDING_HARNESS_TOKEN``, publishes an event on the in-process bus: it
  stands in for an organization event that is already in flight.
- ``password``: Python's password verifier against one stored hash.
- ``auth``: Python's normal and owner-cleanup authentication dependencies for
  each presented access token.
- ``org-notice``: one organization notification fan-out.
- ``expiry-reminder``: seed one withdrawn and one open order, then run the
  order-expiry reminder scan.
"""

import asyncio
import json
import os
import sys
import uuid
from datetime import UTC, datetime, timedelta

RESULT_PREFIX = "OFFBOARDING_RESULT "
HARNESS_PATH = "/api/offboarding-harness"


def _require_disposable() -> None:
    if os.environ.get("ENVIRONMENT") != "test":
        raise SystemExit("the offboarding harness runs only with ENVIRONMENT=test")


def _emit(result: dict) -> None:
    sys.stdout.write(RESULT_PREFIX + json.dumps(result, sort_keys=True) + "\n")
    sys.stdout.flush()


def serve() -> None:
    import uvicorn
    from app.main import app
    from app.services.event_bus import event_bus

    port = int(os.environ["OFFBOARDING_HARNESS_PORT"])
    control_token = os.environ["OFFBOARDING_HARNESS_TOKEN"].encode()

    async def respond(send, status: int) -> None:
        await send(
            {
                "type": "http.response.start",
                "status": status,
                "headers": [(b"content-type", b"application/json")],
            }
        )
        await send({"type": "http.response.body", "body": b"{}"})

    async def harness(scope, receive, send):
        if scope["type"] != "http" or not scope["path"].startswith("/__harness/"):
            await app(scope, receive, send)
            return
        if scope["path"] == "/__harness/ready":
            await respond(send, 200)
            return
        headers = dict(scope.get("headers") or [])
        if scope["path"] != "/__harness/publish" or headers.get(b"x-harness-token") != control_token:
            await respond(send, 403)
            return
        body = b""
        while True:
            message = await receive()
            body += message.get("body", b"")
            if not message.get("more_body"):
                break
        event = json.loads(body)
        await event_bus.publish(event["channel"], event["event"], event["data"])
        await respond(send, 200)

    # The application lifespan loads deployed KYC keys and starts the outbox
    # dispatcher; neither is needed to serve the two private streams.
    uvicorn.run(harness, host="127.0.0.1", port=port, log_level="warning", lifespan="off")


def password(request: dict) -> dict:
    from app.core.security import verify_password

    results = {}
    for candidate in request["passwords"]:
        try:
            accepted = verify_password(candidate, request["hash"])
            results[candidate] = "accepted" if accepted else "rejected"
        except Exception as error:  # noqa: BLE001 - the outcome is the result
            results[candidate] = f"error:{type(error).__name__}"
    return {"passwords": results}


async def _authenticate(engine, dependency, token: str) -> str:
    from fastapi import HTTPException
    from sqlalchemy.ext.asyncio import AsyncSession
    from starlette.requests import Request

    request = Request(
        {
            "type": "http",
            "method": "POST",
            "path": HARNESS_PATH,
            "raw_path": HARNESS_PATH.encode(),
            "query_string": b"",
            "headers": [],
            "scheme": "http",
            "server": ("127.0.0.1", 80),
            "client": ("127.0.0.1", 1),
        }
    )
    async with AsyncSession(engine) as db:
        try:
            await dependency(request, token, db)
            return "accepted"
        except HTTPException as error:
            detail = error.detail
            code = detail.get("code") if isinstance(detail, dict) else str(detail)
            return f"refused:{error.status_code}:{code}"
        except Exception as error:  # noqa: BLE001 - an exception also refuses
            return f"error:{type(error).__name__}"
        finally:
            await db.rollback()


async def auth(request: dict) -> dict:
    from app.routers.auth_simple import get_authenticated_user, get_current_user
    from sqlalchemy.ext.asyncio import create_async_engine

    engine = create_async_engine(os.environ["DATABASE_URL"])
    results = {}
    try:
        for case in request["tokens"]:
            results[case["name"]] = {
                "normal": await _authenticate(engine, get_current_user, case["token"]),
                "cleanup": await _authenticate(engine, get_authenticated_user, case["token"]),
            }
    finally:
        await engine.dispose()
    return {"tokens": results}


async def org_notice(request: dict) -> dict:
    from app.models.notification import Notification, NotificationType
    from app.services.org_notifications import notify_org_users
    from sqlalchemy import select
    from sqlalchemy.ext.asyncio import AsyncSession, create_async_engine

    engine = create_async_engine(os.environ["DATABASE_URL"])
    title = f"Synthetic organization notice {uuid.uuid4().hex}"
    try:
        async with AsyncSession(engine) as db:
            await notify_org_users(
                db,
                uuid.UUID(request["organization_id"]),
                NotificationType.SYSTEM,
                title,
                "Synthetic notice for a disposable organization.",
                {"event": "synthetic_notice"},
            )
            await db.commit()
            recipients = (
                await db.execute(select(Notification.recipient_id).where(Notification.title == title))
            ).scalars().all()
    finally:
        await engine.dispose()
    return {"recipients": sorted(str(recipient) for recipient in recipients)}


async def expiry_reminder(request: dict) -> dict:
    from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
    from app.models.catalog import DeliveryPoint, Product
    from app.models.notification import Notification
    from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
    from app.models.user import OrganizationProvenance
    from app.services.order_expiry_reminders import remind_expiring_orders
    from sqlalchemy import select
    from sqlalchemy.ext.asyncio import AsyncSession, create_async_engine

    organization_id = uuid.UUID(request["organization_id"])
    recipients = [uuid.UUID(value) for value in request["recipients"]]
    now = datetime.now(UTC)
    expiry = now + timedelta(hours=24)
    product_spec = PRODUCTS_BY_CODE["BIO_METHANOL"]
    point_spec = DELIVERY_POINTS_BY_NAME["Singapore"]
    engine = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with AsyncSession(engine, expire_on_commit=False) as db:
            if await db.get(Product, product_spec.id) is None:
                db.add(
                    Product(
                        id=product_spec.id,
                        name=product_spec.name,
                        fuel_type=product_spec.fuel_type,
                        fuel_grade=product_spec.fuel_grade,
                        unit=product_spec.unit,
                        min_lot_size=product_spec.min_lot_size,
                        is_active=True,
                    )
                )
            if await db.get(DeliveryPoint, point_spec.id) is None:
                db.add(
                    DeliveryPoint(
                        id=point_spec.id,
                        name=point_spec.name,
                        region=point_spec.region,
                        timezone=point_spec.timezone,
                        is_active=True,
                    )
                )
            await db.flush()
            orders = {
                "withdrawn": OrderBookOrder(
                    id=uuid.uuid4(),
                    organization_id=organization_id,
                    owner_user_id=uuid.UUID(request["withdrawn_owner_id"]),
                    side=OrderSide.BID,
                    product_id=product_spec.id,
                    delivery_point_id=point_spec.id,
                    quantity_mt=100,
                    remaining_quantity_mt=100,
                    price_per_mt_usd=500,
                    availability_window="SPOT",
                    status=OrderBookStatus.CANCELLED,
                    provenance=OrganizationProvenance.REAL,
                    expires_at=expiry,
                ),
                "open": OrderBookOrder(
                    id=uuid.uuid4(),
                    organization_id=organization_id,
                    owner_user_id=None,
                    side=OrderSide.BID,
                    product_id=product_spec.id,
                    delivery_point_id=point_spec.id,
                    quantity_mt=100,
                    remaining_quantity_mt=100,
                    price_per_mt_usd=500,
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                    provenance=OrganizationProvenance.REAL,
                    expires_at=expiry,
                ),
            }
            db.add_all(orders.values())
            await db.commit()
            await remind_expiring_orders(db, now=now)
            await db.commit()
            names = {str(order.id): name for name, order in orders.items()}
            notices = (
                await db.execute(select(Notification).where(Notification.recipient_id.in_(recipients)))
            ).scalars().all()
    finally:
        await engine.dispose()
    reminders = sorted(
        [names[notice.data["order_id"]], str(notice.recipient_id)]
        for notice in notices
        if (notice.data or {}).get("event") == "order_expiring" and notice.data.get("order_id") in names
    )
    return {"reminders": reminders}


def main() -> None:
    _require_disposable()
    command = sys.argv[1]
    if command == "serve":
        serve()
        return
    request = json.loads(sys.stdin.read() or "{}")
    if command == "password":
        _emit(password(request))
    elif command == "auth":
        _emit(asyncio.run(auth(request)))
    elif command == "org-notice":
        _emit(asyncio.run(org_notice(request)))
    elif command == "expiry-reminder":
        _emit(asyncio.run(expiry_reminder(request)))
    else:
        raise SystemExit(f"unknown command {command!r}")


if __name__ == "__main__":
    main()
