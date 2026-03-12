"""SSE activity stream — public + participant-specific events."""
import asyncio
import json

from fastapi import APIRouter, Request, Depends
from fastapi.responses import StreamingResponse

from app.routers.auth_simple import get_current_user_optional
from app.services.event_bus import event_bus

router = APIRouter(prefix="/stream", tags=["real-time"])


@router.get("/activity")
async def stream_activity(
    request: Request,
    token: str | None = None,
    current_user=Depends(get_current_user_optional),
):
    """SSE stream for market activity.

    Authenticated users receive both public activity events and events
    specific to their organisation (outbids, triggered alerts).
    Unauthenticated users receive public activity events only.
    """
    # EventSource doesn't support custom headers, so accept token as query param
    # and resolve the user manually if header-based auth returned None.
    if current_user is None and token:
        from app.routers.auth_simple import get_current_user as _get_user
        from app.database import get_db as _get_db
        try:
            from jose import jwt
            from app.config import settings
            payload = jwt.decode(token, settings.JWT_SECRET_KEY, algorithms=[settings.JWT_ALGORITHM])
            # If decode succeeds, create a minimal user-like object
            class _SSEUser:
                def __init__(self, org_id):
                    self.organization_id = org_id
            current_user = _SSEUser(payload.get("organization_id"))
        except Exception:
            pass  # Invalid token — proceed as unauthenticated

    subscriptions: list[tuple[str, asyncio.Queue]] = []

    public_queue = event_bus.subscribe("activity")
    if public_queue is None:
        from fastapi.responses import JSONResponse
        return JSONResponse(
            status_code=503,
            content={"detail": "Too many connections to activity channel"},
        )
    subscriptions.append(("activity", public_queue))

    if current_user is not None and current_user.organization_id is not None:
        org_channel = f"activity:{current_user.organization_id}"
        org_queue = event_bus.subscribe(org_channel)
        if org_queue is not None:
            subscriptions.append((org_channel, org_queue))

    async def _generator():
        silence_budget = 30.0
        elapsed_silence = 0.0
        poll_interval = 0.5

        try:
            while True:
                if await request.is_disconnected():
                    break

                got_message = False
                for _channel, q in subscriptions:
                    try:
                        message = await asyncio.wait_for(q.get(), timeout=poll_interval)
                        yield (
                            f"event: {message['event']}\n"
                            f"data: {json.dumps(message['data'], default=str)}\n\n"
                        )
                        got_message = True
                        elapsed_silence = 0.0
                    except asyncio.TimeoutError:
                        pass

                if not got_message:
                    elapsed_silence += poll_interval * len(subscriptions)
                    if elapsed_silence >= silence_budget:
                        yield ": keepalive\n\n"
                        elapsed_silence = 0.0
        finally:
            for channel, q in subscriptions:
                event_bus.unsubscribe(channel, q)

    return StreamingResponse(
        _generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
