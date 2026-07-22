"""Server-Sent Events endpoints for real-time data feeds."""
import asyncio
import json
import os
import time

import uuid

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import StreamingResponse
from sqlalchemy import select
from sqlalchemy.exc import DBAPIError
import jwt
import logging

from app.core.security import decode_token
from app.config import settings
from app.database import AsyncSessionLocal
from app.models.user import User, Organization
from app.routers.auth_simple import validate_authenticated_user_state
from app.services.event_bus import event_bus
from app.services.market_event_dispatch import (
    fetch_events_for_org,
    stream_channel_for_org,
)

router = APIRouter(prefix="/stream", tags=["real-time"])
logger = logging.getLogger(__name__)


async def _sse_generator(request: Request, channel: str):
    """SSE generator that yields events from the event bus."""
    queue = event_bus.subscribe(channel)
    if queue is None:
        yield 'event: error\ndata: {"error": "Too many connections"}\n\n'
        return
    try:
        while True:
            if await request.is_disconnected():
                break
            try:
                message = await asyncio.wait_for(queue.get(), timeout=30.0)
                yield f"event: {message['event']}\ndata: {json.dumps(message['data'], default=str)}\n\n"
            except asyncio.TimeoutError:
                # Send keepalive comment every 30s
                yield ": keepalive\n\n"
    finally:
        event_bus.unsubscribe(channel, queue)


def _format_market_event(seq, event_type: str, data) -> str:
    """One SSE frame; ``id:`` carries the durable stream sequence."""
    payload = json.dumps(data, default=str)
    if seq is None:
        return f"event: {event_type}\ndata: {payload}\n\n"
    return f"id: {seq}\nevent: {event_type}\ndata: {payload}\n\n"


async def _private_sse_generator(
    request: Request,
    channel: str,
    user_id: uuid.UUID,
    organization_id: uuid.UUID,
    token_payload: dict,
    last_event_id: int | None = None,
):
    """Private stream bounded by token expiry and mutable admission state.

    Durable delivery contract (Stage 5): messages fanned out from the market
    event outbox carry a monotonic per-database ``seq`` used as the SSE
    ``id:``. Reconnecting clients send ``Last-Event-ID`` and receive every
    committed event for their organization after that cursor straight from
    the outbox before live delivery resumes; the cursor also de-duplicates
    events that arrive on the in-process bus during replay.
    """
    # Subscribe before replay so nothing slips between the replay query and
    # live delivery; the sequence cursor drops any overlap.
    queue = event_bus.subscribe(channel)
    if queue is None:
        yield 'event: error\ndata: {"error": "Too many connections"}\n\n'
        return
    # Operational comment (invisible to EventSource): lets tests/operators
    # confirm which worker process serves this stream.
    yield f": stream-origin pid={os.getpid()}\n\n"
    cursor = last_event_id if last_event_id is not None else -1
    if last_event_id is not None:
        try:
            async with AsyncSessionLocal() as db:
                try:
                    missed = await fetch_events_for_org(
                        db, organization_id, after_seq=last_event_id
                    )
                finally:
                    await db.rollback()
        except DBAPIError:
            logger.warning(
                "private_sse_replay_failed", extra={"reason": "database_error"}
            )
            event_bus.unsubscribe(channel, queue)
            yield 'event: error\ndata: {"error": "Replay is temporarily unavailable"}\n\n'
            return
        for event in missed:
            cursor = max(cursor, event.stream_seq)
            yield _format_market_event(event.stream_seq, event.event_type, event.payload)
    expires_at = float(token_payload["exp"])
    next_revalidation = 0.0
    try:
        while True:
            now = time.time()
            if now >= expires_at:
                yield 'event: auth_expired\ndata: {"error": "Stream token expired"}\n\n'
                break
            if now >= next_revalidation:
                try:
                    # Never retain the request-scoped dependency for a stream.
                    # Each revalidation gets and releases its own short-lived
                    # session/transaction so idle tabs cannot pin pool slots.
                    async with AsyncSessionLocal() as db:
                        try:
                            result = await db.execute(select(User).where(User.id == user_id))
                            user = result.scalar_one_or_none()
                            org = None
                            if user and user.organization_id == organization_id:
                                org_result = await db.execute(
                                    select(Organization).where(Organization.id == organization_id)
                                )
                                org = org_result.scalar_one_or_none()
                            if user is None or user.organization_id != organization_id:
                                raise ValueError("stream organization changed")
                            validate_authenticated_user_state(user, token_payload, request_path="/api/stream/trades")
                            if org is None or org.verification_status != "APPROVED":
                                raise ValueError("organization is no longer approved")
                        finally:
                            await db.rollback()
                except (HTTPException, ValueError):
                    yield 'event: auth_revoked\ndata: {"error": "Stream authorization changed"}\n\n'
                    break
                except DBAPIError:
                    logger.warning("private_sse_revalidation_failed", extra={"reason": "database_error"})
                    yield 'event: auth_revoked\ndata: {"error": "Stream authorization unavailable"}\n\n'
                    break
                next_revalidation = now + 15.0
            if await request.is_disconnected():
                break
            timeout = min(30.0, max(0.1, expires_at - now), max(0.1, next_revalidation - now))
            try:
                message = await asyncio.wait_for(queue.get(), timeout=timeout)
                seq = message.get("seq")
                if seq is not None:
                    if seq <= cursor:
                        continue  # already delivered by replay
                    cursor = seq
                yield _format_market_event(seq, message["event"], message["data"])
            except asyncio.TimeoutError:
                yield ": keepalive\n\n"
    finally:
        event_bus.unsubscribe(channel, queue)


@router.get("/prices")
async def stream_prices(request: Request):
    """SSE stream for live price updates. No auth required (public data)."""
    return StreamingResponse(
        _sse_generator(request, "prices"),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )


@router.get("/orderbook")
async def stream_orderbook(request: Request):
    """SSE stream for orderbook updates (new orders, fills, cancellations)."""
    return StreamingResponse(
        _sse_generator(request, "orderbook"),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )


def _parse_last_event_id(request: Request, query_value: str | None) -> int | None:
    """Resolve the replay cursor; the standard EventSource header wins."""
    raw = request.headers.get("last-event-id") or query_value
    if raw is None:
        return None
    try:
        value = int(raw)
    except (TypeError, ValueError):
        value = -1
    if value < 0:
        raise HTTPException(
            status_code=400,
            detail={
                "code": "STREAM_CURSOR_INVALID",
                "message": "Last-Event-ID must be a non-negative integer",
            },
        )
    return value


@router.get("/trades")
async def stream_trades(
    request: Request,
    stream_token: str | None = None,
    last_event_id: str | None = None,
):
    """Tenant-private trade lifecycle stream; only 60-second stream tokens work."""
    replay_after = _parse_last_event_id(request, last_event_id)
    if request.headers.get("authorization") or request.headers.get("Authorization"):
        raise HTTPException(
            status_code=400,
            detail={
                "code": "STREAM_AUTH_CONFLICT",
                "message": "Authorization headers are not accepted by SSE; use only stream_token",
            },
        )
    if not stream_token:
        raise HTTPException(
            status_code=401,
            detail={"code": "STREAM_TOKEN_REQUIRED", "message": "A stream token is required"},
        )
    try:
        payload = decode_token(stream_token)
        if payload.get("type") != "stream":
            raise ValueError("wrong token type")
        if payload.get("environment") != settings.ENVIRONMENT.strip().lower():
            raise ValueError("wrong token environment")
        user_id = uuid.UUID(payload["sub"])
        token_organization_id = uuid.UUID(payload["org_id"])
        async with AsyncSessionLocal() as db:
            try:
                user = (await db.execute(select(User).where(User.id == user_id))).scalar_one_or_none()
                if user is None:
                    raise ValueError("unknown user")
                validate_authenticated_user_state(user, payload, request_path="/api/stream/trades")
                if user.organization_id != token_organization_id:
                    raise ValueError("stream organization changed")
                organization_id = token_organization_id
                organization = (
                    await db.execute(select(Organization).where(Organization.id == organization_id))
                ).scalar_one_or_none()
                if organization is None or organization.verification_status != "APPROVED":
                    raise ValueError("organization is not approved")
                channel = stream_channel_for_org(organization_id)
            finally:
                await db.rollback()
    except (jwt.PyJWTError, ValueError, HTTPException) as exc:
        raise HTTPException(
            status_code=401,
            detail={"code": "STREAM_TOKEN_INVALID", "message": "Stream token is invalid or expired"},
        ) from exc
    except DBAPIError as exc:
        raise HTTPException(
            status_code=503,
            detail={
                "code": "STREAM_AUTH_UNAVAILABLE",
                "message": "Stream authorization is temporarily unavailable",
            },
        ) from exc
    return StreamingResponse(
        _private_sse_generator(
            request, channel, user_id, organization_id, payload, replay_after
        ),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
