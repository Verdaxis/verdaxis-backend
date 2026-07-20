"""Private SSE credential, scope, expiry, and subscription cleanup contracts."""

import asyncio
import inspect
import time
from contextlib import asynccontextmanager
from types import SimpleNamespace
from unittest.mock import MagicMock, patch
from uuid import uuid4

import pytest
from fastapi import HTTPException

from app.core.security import create_access_token, create_stream_token, decode_token
from app.models.user import UserRole, UserStatus
from app.routers import activity, stream


class _Request:
    def __init__(self, *, headers=None, disconnected=True):
        self.headers = headers or {}
        self._disconnected = disconnected

    async def is_disconnected(self):
        return self._disconnected


def _user(*, user_id=None, organization_id=None):
    return SimpleNamespace(
        id=user_id or uuid4(),
        organization_id=organization_id or uuid4(),
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
        password_changed_at=None,
    )


class _RecordingBus:
    def __init__(self):
        self.subscribed = []
        self.unsubscribed = []

    def subscribe(self, channel):
        queue = asyncio.Queue()
        self.subscribed.append((channel, queue))
        return queue

    def unsubscribe(self, channel, queue):
        self.unsubscribed.append((channel, queue))


class _RejectSecondSubscriptionBus(_RecordingBus):
    def subscribe(self, channel):
        if self.subscribed:
            return None
        return super().subscribe(channel)


def test_activity_route_has_no_bearer_auth_dependency():
    assert "current_user" not in inspect.signature(activity.stream_activity).parameters


@pytest.mark.asyncio
async def test_activity_requires_a_short_lived_stream_token():
    with pytest.raises(HTTPException) as exc_info:
        await activity.stream_activity(request=_Request(), stream_token=None)

    assert exc_info.value.status_code == 401
    assert exc_info.value.detail["code"] == "STREAM_TOKEN_REQUIRED"


@pytest.mark.asyncio
async def test_activity_invalid_stream_token_is_rejected_not_downgraded_to_public():
    kwargs = {"request": _Request(), "stream_token": create_access_token(str(uuid4()))}
    if "current_user" in inspect.signature(activity.stream_activity).parameters:
        kwargs["current_user"] = None
    with pytest.raises(HTTPException) as exc_info:
        await activity.stream_activity(**kwargs)
    assert exc_info.value.status_code == 401


@pytest.mark.asyncio
async def test_activity_rejects_conflicting_authorization_and_stream_token():
    candidate = _user()
    token = create_stream_token(candidate.id, candidate.organization_id)
    kwargs = {
        "request": _Request(headers={"Authorization": "Bearer ordinary-access-token"}),
        "stream_token": token,
    }
    if "current_user" in inspect.signature(activity.stream_activity).parameters:
        kwargs["current_user"] = None
    result = MagicMock()
    result.scalar_one_or_none.return_value = candidate

    @asynccontextmanager
    async def session_context():
        async def execute(*_args, **_kwargs):
            return result
        async def rollback():
            return None
        yield SimpleNamespace(execute=execute, rollback=rollback)

    with patch.object(activity, "AsyncSessionLocal", session_context):
        with pytest.raises(HTTPException) as exc_info:
            await activity.stream_activity(**kwargs)
    assert exc_info.value.status_code == 400


@pytest.mark.asyncio
async def test_trade_stream_rejects_conflicting_authorization_and_stream_token():
    token = create_stream_token(uuid4(), uuid4())

    with pytest.raises(HTTPException) as exc_info:
        await stream.stream_trades(
            request=_Request(headers={"Authorization": "Bearer ordinary-access-token"}),
            stream_token=token,
        )

    assert exc_info.value.status_code == 400
    assert exc_info.value.detail["code"] == "STREAM_AUTH_CONFLICT"


@pytest.mark.asyncio
async def test_activity_exact_org_revalidation_rejects_user_move():
    user_id = uuid4()
    original_org_id = uuid4()
    moved_user = _user(user_id=user_id, organization_id=uuid4())
    result = MagicMock()
    result.scalar_one_or_none.return_value = moved_user

    @asynccontextmanager
    async def session_context():
        async def execute(*_args, **_kwargs):
            return result

        async def rollback():
            return None

        yield SimpleNamespace(execute=execute, rollback=rollback)

    principal = activity.ActivityStreamPrincipal(
        user_id=user_id,
        organization_id=original_org_id,
        token_payload=decode_token(create_stream_token(user_id, original_org_id)),
    )
    with patch.object(activity, "AsyncSessionLocal", session_context):
        assert await activity._activity_principal_is_current(principal) is False


@pytest.mark.asyncio
async def test_trade_stream_org_move_revokes_and_unsubscribes_original_channel():
    user_id = uuid4()
    original_org_id = uuid4()
    moved_user = _user(user_id=user_id, organization_id=uuid4())
    token_payload = decode_token(create_stream_token(user_id, original_org_id))
    result = MagicMock()
    result.scalar_one_or_none.return_value = moved_user
    db = MagicMock()
    db.execute = MagicMock(return_value=result)
    db.rollback = MagicMock()

    @asynccontextmanager
    async def session_context():
        async_db = SimpleNamespace(
            execute=MagicMock(return_value=result),
            rollback=MagicMock(return_value=None),
        )
        async def execute(*args, **kwargs):
            return result
        async def rollback():
            return None
        async_db.execute = execute
        async_db.rollback = rollback
        yield async_db

    bus = _RecordingBus()
    with patch.object(stream, "AsyncSessionLocal", session_context), patch.object(stream, "event_bus", bus):
        generator = stream._private_sse_generator(
            request=_Request(disconnected=False),
            channel=f"trades:{original_org_id}",
            user_id=user_id,
            organization_id=original_org_id,
            token_payload=token_payload,
        )
        event = await generator.__anext__()
        await generator.aclose()

    assert "auth_revoked" in event
    assert [channel for channel, _queue in bus.unsubscribed] == [f"trades:{original_org_id}"]


@pytest.mark.asyncio
async def test_activity_generator_unsubscribes_all_queues_on_cancellation():
    generator_factory = getattr(activity, "_activity_sse_generator", None)
    assert generator_factory is not None
    bus = _RecordingBus()
    request = _Request(disconnected=False)
    with patch.object(activity, "event_bus", bus):
        generator = generator_factory(
            request=request,
            channels=("activity", f"activity:{uuid4()}"),
            principal=None,
        )
        task = asyncio.create_task(generator.__anext__())
        await asyncio.sleep(0)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        await generator.aclose()

    assert len(bus.subscribed) == 2
    assert {channel for channel, _queue in bus.unsubscribed} == {
        channel for channel, _queue in bus.subscribed
    }


@pytest.mark.asyncio
async def test_activity_generator_unsubscribes_first_queue_when_second_subscription_is_rejected():
    bus = _RejectSecondSubscriptionBus()
    channels = ("activity", f"activity:{uuid4()}")
    with patch.object(activity, "event_bus", bus):
        generator = activity._activity_sse_generator(
            request=_Request(),
            channels=channels,
            principal=None,
        )
        assert "Too many connections" in await generator.__anext__()
        await generator.aclose()

    assert [channel for channel, _queue in bus.unsubscribed] == ["activity"]
