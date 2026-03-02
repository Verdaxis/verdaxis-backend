"""Server-Sent Events endpoints for real-time data feeds."""
import asyncio
import json

from fastapi import APIRouter, Request
from fastapi.responses import StreamingResponse

from app.services.event_bus import event_bus

router = APIRouter(prefix="/stream", tags=["real-time"])


async def _sse_generator(request: Request, channel: str):
    """SSE generator that yields events from the event bus."""
    queue = event_bus.subscribe(channel)
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


@router.get("/trades")
async def stream_trades(request: Request):
    """SSE stream for trade lifecycle events."""
    return StreamingResponse(
        _sse_generator(request, "trades"),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
