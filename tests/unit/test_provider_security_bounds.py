"""Provider calls must use native deadlines and retain capacity until completion."""

import inspect
import asyncio
from pathlib import Path
import threading

import pytest

from app.config import settings
from app.services import ai_service, gemini_provider, news_feed


ROOT = Path(__file__).resolve().parents[2]


class _Response:
    text = '{"category":"markets","relevance":3,"summary":null}'


class _Model:
    def __init__(self):
        self.kwargs = None

    def generate_content(self, *args, **kwargs):
        self.kwargs = kwargs
        return _Response()


def test_copilot_uses_provider_native_timeout(monkeypatch):
    model = _Model()
    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(ai_service, "get_gemini_model", lambda *_args, **_kwargs: model)

    ai_service._chat_sync("hello")

    assert model.kwargs["request_options"].timeout == ai_service.AI_PROVIDER_TIMEOUT_SECONDS


def test_news_categorization_uses_provider_native_timeout(monkeypatch):
    model = _Model()
    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(news_feed, "get_gemini_model", lambda *_args, **_kwargs: model)

    news_feed._categorize_headline_sync("Shipping market update")

    assert model.kwargs["request_options"].timeout == news_feed.NEWS_PROVIDER_TIMEOUT_SECONDS


def test_copilot_does_not_abandon_timed_out_threads_or_release_capacity_early():
    source = inspect.getsource(ai_service.chat_with_copilot)
    assert "asyncio.wait_for" not in source
    assert "run_provider_call" in source


def test_provider_singleton_is_pid_and_thread_safe():
    source = (ROOT / "app" / "services" / "gemini_provider.py").read_text()
    assert "getpid" in source
    assert "threading" in source
    assert "request_options" in source


def test_news_refresh_defines_postgres_overlap_guard():
    assert hasattr(news_feed, "NewsRefreshInProgress")
    source = inspect.getsource(news_feed.refresh_news)
    assert "pg_try_advisory_xact_lock" in source


@pytest.mark.asyncio
async def test_cancelled_provider_waiter_does_not_release_thread_capacity_early():
    capacity = gemini_provider.ProviderCapacity(1)
    started = threading.Event()
    release = threading.Event()
    finished = threading.Event()

    def blocking_provider_call():
        started.set()
        release.wait(timeout=5)
        finished.set()
        return "done"

    first = asyncio.create_task(
        gemini_provider.run_provider_call(
            capacity,
            blocking_provider_call,
            busy_error=RuntimeError("busy"),
        )
    )
    assert await asyncio.to_thread(started.wait, 2)
    first.cancel()
    with pytest.raises(asyncio.CancelledError):
        await first

    with pytest.raises(RuntimeError, match="busy"):
        await gemini_provider.run_provider_call(
            capacity,
            lambda: "must-not-run",
            busy_error=RuntimeError("busy"),
        )

    release.set()
    assert await asyncio.to_thread(finished.wait, 2)
    for _ in range(20):
        try:
            result = await gemini_provider.run_provider_call(
                capacity,
                lambda: "available",
                busy_error=RuntimeError("busy"),
            )
            break
        except RuntimeError:
            await asyncio.sleep(0.01)
    else:
        pytest.fail("provider capacity was not released after the underlying call exited")
    assert result == "available"
