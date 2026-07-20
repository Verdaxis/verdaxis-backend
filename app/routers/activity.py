"""Short-lived stream-token-scoped tenant activity SSE."""

import asyncio
from dataclasses import dataclass
import json
import logging
import time
import uuid

import jwt
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import StreamingResponse
from sqlalchemy import select
from sqlalchemy.exc import DBAPIError

from app.core.security import decode_token
from app.config import settings
from app.database import AsyncSessionLocal
from app.models.user import Organization, User
from app.routers.auth_simple import validate_authenticated_user_state
from app.services.event_bus import event_bus

router = APIRouter(prefix="/stream", tags=["real-time"])
logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class ActivityStreamPrincipal:
    user_id: uuid.UUID
    organization_id: uuid.UUID
    token_payload: dict


def _stream_auth_error(status_code: int, code: str, message: str) -> HTTPException:
    return HTTPException(status_code=status_code, detail={"code": code, "message": message})


async def _load_activity_principal(stream_token: str) -> ActivityStreamPrincipal:
    try:
        payload = decode_token(stream_token)
        if payload.get("type") != "stream":
            raise ValueError("wrong token type")
        if payload.get("environment") != settings.ENVIRONMENT.strip().lower():
            raise ValueError("wrong token environment")
        user_id = uuid.UUID(payload["sub"])
        token_organization_id = uuid.UUID(payload["org_id"])
    except (jwt.PyJWTError, KeyError, TypeError, ValueError) as exc:
        raise _stream_auth_error(401, "STREAM_TOKEN_INVALID", "Stream token is invalid or expired") from exc

    try:
        async with AsyncSessionLocal() as db:
            try:
                user = (await db.execute(select(User).where(User.id == user_id))).scalar_one_or_none()
                if user is None or user.organization_id != token_organization_id:
                    raise _stream_auth_error(401, "STREAM_ACCESS_REVOKED", "Private stream access is unavailable")
                validate_authenticated_user_state(user, payload, request_path="/api/stream/activity")
                organization = (
                    await db.execute(select(Organization).where(Organization.id == user.organization_id))
                ).scalar_one_or_none()
                if organization is None or organization.verification_status != "APPROVED":
                    raise _stream_auth_error(401, "STREAM_ACCESS_REVOKED", "Private stream access is unavailable")
                return ActivityStreamPrincipal(
                    user_id=user.id,
                    organization_id=token_organization_id,
                    token_payload=payload,
                )
            finally:
                await db.rollback()
    except HTTPException as exc:
        if exc.status_code in (401, 403):
            raise _stream_auth_error(401, "STREAM_ACCESS_REVOKED", "Private stream access is unavailable") from exc
        raise
    except DBAPIError as exc:
        raise _stream_auth_error(
            503,
            "STREAM_AUTH_UNAVAILABLE",
            "Activity authorization is temporarily unavailable",
        ) from exc


async def _activity_principal_is_current(principal: ActivityStreamPrincipal) -> bool:
    async with AsyncSessionLocal() as db:
        try:
            user = (
                await db.execute(select(User).where(User.id == principal.user_id))
            ).scalar_one_or_none()
            if user is None or user.organization_id != principal.organization_id:
                return False
            validate_authenticated_user_state(
                user,
                principal.token_payload,
                request_path="/api/stream/activity",
            )
            organization = (
                await db.execute(select(Organization).where(Organization.id == principal.organization_id))
            ).scalar_one_or_none()
            return organization is not None and organization.verification_status == "APPROVED"
        finally:
            await db.rollback()


async def _activity_sse_generator(
    request: Request,
    channels: tuple[str, ...],
    principal: ActivityStreamPrincipal | None,
):
    """Subscribe lazily so every startup, exception, and cancellation path cleans up."""
    subscriptions: list[tuple[str, asyncio.Queue]] = []
    expires_at = float(principal.token_payload["exp"]) if principal is not None else None
    try:
        for channel in channels:
            queue = event_bus.subscribe(channel)
            if queue is None:
                yield 'event: error\ndata: {"error": "Too many connections"}\n\n'
                return
            subscriptions.append((channel, queue))

        next_revalidation = 0.0
        keepalive_at = time.monotonic() + 30.0
        while True:
            if await request.is_disconnected():
                break
            now_epoch = time.time()
            now_loop = time.monotonic()
            if expires_at is not None and now_epoch >= expires_at:
                yield 'event: auth_expired\ndata: {"error": "Stream token expired"}\n\n'
                break
            if principal is not None and now_loop >= next_revalidation:
                try:
                    if not await _activity_principal_is_current(principal):
                        yield 'event: auth_revoked\ndata: {"error": "Stream authorization changed"}\n\n'
                        break
                except HTTPException:
                    yield 'event: auth_revoked\ndata: {"error": "Stream authorization changed"}\n\n'
                    break
                except DBAPIError:
                    logger.warning("activity_sse_revalidation_failed", extra={"reason": "database_error"})
                    yield 'event: auth_revoked\ndata: {"error": "Stream authorization unavailable"}\n\n'
                    break
                next_revalidation = now_loop + 15.0

            timeout = min(
                0.5,
                max(0.1, keepalive_at - now_loop),
                max(0.1, next_revalidation - now_loop) if principal is not None else 30.0,
                max(0.1, expires_at - now_epoch) if expires_at is not None else 30.0,
            )
            message = None
            for _channel, queue in subscriptions:
                try:
                    message = queue.get_nowait()
                    break
                except asyncio.QueueEmpty:
                    continue
            if message is None:
                try:
                    message = await asyncio.wait_for(subscriptions[0][1].get(), timeout=timeout)
                except asyncio.TimeoutError:
                    if time.monotonic() >= keepalive_at:
                        yield ": keepalive\n\n"
                        keepalive_at = time.monotonic() + 30.0
                    continue
            yield f"event: {message['event']}\ndata: {json.dumps(message['data'], default=str)}\n\n"
    finally:
        for channel, queue in subscriptions:
            event_bus.unsubscribe(channel, queue)


@router.get("/activity")
async def stream_activity(request: Request, stream_token: str | None = None):
    """Tenant activity authorized only by a short-lived ``stream_token``."""
    if request.headers.get("authorization") or request.headers.get("Authorization"):
        raise _stream_auth_error(
            400,
            "STREAM_AUTH_CONFLICT",
            "Authorization headers are not accepted by SSE; use only stream_token",
        )
    if not stream_token:
        raise _stream_auth_error(
            401,
            "STREAM_TOKEN_REQUIRED",
            "A stream token is required",
        )

    principal = await _load_activity_principal(stream_token)
    channels = (f"activity:{principal.organization_id}",)

    return StreamingResponse(
        _activity_sse_generator(request, channels, principal),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
