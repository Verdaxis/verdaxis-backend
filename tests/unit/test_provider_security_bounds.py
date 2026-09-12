"""Provider calls must use native deadlines and retain capacity until completion."""

import inspect
import asyncio
import json
import os
import threading
from types import SimpleNamespace

import httpx
import pytest
from google import genai
from google.genai import errors, types

from app.config import settings
from app.services import ai_service, gemini_provider, kyc, news_feed


class _Response:
    text = '{"category":"markets","relevance":3,"summary":null}'


class _Models:
    def __init__(self):
        self.calls = []

    def generate_content(self, **kwargs):
        self.calls.append(kwargs)
        return _Response()


class _Client:
    def __init__(self, **kwargs):
        self.init_kwargs = kwargs
        self.models = _Models()
        self.closed = False

    def close(self):
        self.closed = True


def test_shared_client_uses_bounded_native_retry_within_total_budget(monkeypatch):
    clients = []

    def client_factory(**kwargs):
        client = _Client(**kwargs)
        clients.append(client)
        return client

    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(gemini_provider.genai, "Client", client_factory)
    monkeypatch.setattr(gemini_provider, "_client", None)
    monkeypatch.setattr(gemini_provider, "_client_pid", None)

    first_response = gemini_provider.generate_content("gemini-test", "hello", 20)
    second_response = gemini_provider.generate_content("gemini-test", "hello", 15)

    assert first_response.text and second_response.text
    assert len(clients) == 1
    assert clients[0].init_kwargs == {"api_key": "configured-not-logged"}
    for request, total_budget_ms in zip(clients[0].models.calls, (20_000, 15_000)):
        http_options = request["config"].http_options
        assert (
            http_options.timeout * http_options.retry_options.attempts
            + int(http_options.retry_options.initial_delay * 1_000)
            <= total_budget_ms
        )
        assert http_options.retry_options.attempts == 2
        assert request["model"] == "gemini-test"
        assert request["contents"] == "hello"
        assert request["config"].automatic_function_calling.disable is True
    assert clients[0].models.calls[0]["config"].http_options.timeout != (
        clients[0].models.calls[1]["config"].http_options.timeout
    )


def test_supported_sdk_serializes_content_and_retries_with_mock_transport(monkeypatch):
    requests = []

    def handler(request):
        requests.append(request)
        if len(requests) == 1:
            return httpx.Response(
                503,
                json={"error": {"code": 503, "message": "busy", "status": "UNAVAILABLE"}},
            )
        return httpx.Response(
            200,
            json={
                "candidates": [
                    {
                        "content": {"parts": [{"text": "ok"}], "role": "model"},
                        "finishReason": "STOP",
                        "index": 0,
                    }
                ]
            },
        )

    http_options = types.HttpOptions(
        timeout=9_950,
        retry_options=types.HttpRetryOptions(
            attempts=2,
            initial_delay=0,
            max_delay=0,
            exp_base=1,
            jitter=0,
        ),
        httpx_client=httpx.Client(transport=httpx.MockTransport(handler)),
    )
    client = genai.Client(api_key="not-a-real-key", http_options=http_options)
    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(gemini_provider, "_client", client)
    monkeypatch.setattr(gemini_provider, "_client_pid", os.getpid())
    try:
        response = gemini_provider.generate_content("gemini-test", "hello", 20)
    finally:
        client.close()

    assert response.text == "ok"
    assert len(requests) == 2
    assert requests[0].url.path.endswith("/models/gemini-test:generateContent")
    assert json.loads(requests[0].content)["contents"][0]["parts"] == [{"text": "hello"}]
    assert requests[0].extensions["timeout"]["read"] == 9.95


def test_copilot_and_news_keep_their_provider_budgets(monkeypatch):
    calls = []
    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(
        ai_service,
        "generate_content",
        lambda model, contents, timeout: calls.append((model, contents, timeout)) or _Response(),
    )
    monkeypatch.setattr(
        news_feed,
        "generate_content",
        lambda model, contents, timeout: calls.append((model, contents, timeout)) or _Response(),
    )

    ai_service._chat_sync("hello")
    news_feed._categorize_headline_sync("Shipping market update")

    assert calls[0] == (
        gemini_provider.DEFAULT_GEMINI_MODEL,
        "hello",
        ai_service.AI_PROVIDER_TIMEOUT_SECONDS,
    )
    assert calls[1][0] == news_feed.GEMINI_MODEL
    assert isinstance(calls[1][1], str) and "Shipping market update" in calls[1][1]
    assert calls[1][2] == news_feed.NEWS_PROVIDER_TIMEOUT_SECONDS


def test_kyc_sends_document_as_typed_inline_bytes(monkeypatch):
    calls = []
    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(
        kyc,
        "generate_content",
        lambda model, contents, timeout: calls.append((model, contents, timeout))
        or SimpleNamespace(
            text='{"valid":true,"readable":true,"tampered":false,"confidence":0.9,"issues":[]}'
        ),
    )

    result = kyc._verify_document_with_gemini_sync(b"document-bytes", "application/pdf", "identity")

    assert result["passed"] is True
    assert calls[0][0] == gemini_provider.DEFAULT_GEMINI_MODEL
    assert 0 < calls[0][2] <= kyc.KYC_PROVIDER_TIMEOUT_SECONDS
    document_part = calls[0][1][1]
    assert isinstance(document_part, types.Part)
    assert document_part.inline_data.mime_type == "application/pdf"
    assert document_part.inline_data.data == b"document-bytes"


def test_kyc_fallback_cannot_restart_the_provider_budget(monkeypatch):
    calls = []
    clock = iter((100.0, 110.0, 130.0))

    def missing_model(model, _contents, timeout):
        calls.append((model, timeout))
        raise RuntimeError("model 404 not found")

    monkeypatch.setattr(kyc, "monotonic", lambda: next(clock))
    monkeypatch.setattr(kyc, "generate_content", missing_model)

    with pytest.raises(kyc.KYCProviderUnavailable, match="No configured KYC model"):
        kyc._verify_document_with_gemini_sync(b"document", "application/pdf", "identity")

    assert calls == [(gemini_provider.DEFAULT_GEMINI_MODEL, 20)]


def test_kyc_uses_sdk_error_code_for_model_fallback(monkeypatch):
    calls = []
    clock = iter((100.0, 100.0, 101.0))

    def provider_call(model, _contents, timeout):
        calls.append((model, timeout))
        if len(calls) == 1:
            raise errors.ClientError(
                404,
                {"error": {"message": "Requested entity was not found."}},
            )
        return SimpleNamespace(
            text='{"valid":true,"readable":true,"tampered":false,"confidence":0.9,"issues":[]}'
        )

    monkeypatch.setattr(kyc, "monotonic", lambda: next(clock))
    monkeypatch.setattr(kyc, "generate_content", provider_call)

    result = kyc._verify_document_with_gemini_sync(b"document", "application/pdf", "identity")

    assert result["passed"] is True
    assert calls == [
        (gemini_provider.DEFAULT_GEMINI_MODEL, 30),
        ("gemini-3.1-flash-lite", 29),
    ]


def test_copilot_does_not_abandon_timed_out_threads_or_release_capacity_early():
    source = inspect.getsource(ai_service.chat_with_copilot)
    assert "asyncio.wait_for" not in source
    assert "run_provider_call" in source


def test_provider_client_cache_is_pid_safe(monkeypatch):
    process_id = 10
    clients = []

    def client_factory(**kwargs):
        client = _Client(**kwargs)
        clients.append(client)
        return client

    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(gemini_provider.genai, "Client", client_factory)
    monkeypatch.setattr(gemini_provider.os, "getpid", lambda: process_id)
    monkeypatch.setattr(gemini_provider, "_client", None)
    monkeypatch.setattr(gemini_provider, "_client_pid", None)

    gemini_provider.generate_content("first", "hello", 20)
    gemini_provider.generate_content("second", "hello", 20)
    process_id = 11
    gemini_provider.generate_content("third", "hello", 20)

    assert len(clients) == 2
    assert clients[0].closed is True


@pytest.mark.asyncio
async def test_copilot_does_not_start_provider_after_queue_deadline(monkeypatch):
    clock = [100.0]
    provider_called = False

    async def delayed_to_thread(call, *args):
        clock[0] = 121.0
        return call(*args)

    def provider_call(*_args):
        nonlocal provider_called
        provider_called = True
        return _Response()

    monkeypatch.setattr(settings, "GEMINI_API_KEY", "configured-not-logged")
    monkeypatch.setattr(ai_service, "monotonic", lambda: clock[0])
    monkeypatch.setattr(gemini_provider, "monotonic", lambda: clock[0])
    monkeypatch.setattr(asyncio, "to_thread", delayed_to_thread)
    monkeypatch.setattr(ai_service, "generate_content", provider_call)

    with pytest.raises(ai_service.AIProviderUnavailable):
        await ai_service.chat_with_copilot("hello")

    assert provider_called is False


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


@pytest.mark.asyncio
async def test_provider_deadline_does_not_release_thread_capacity_early():
    capacity = gemini_provider.ProviderCapacity(1)
    started = threading.Event()
    release = threading.Event()
    finished = threading.Event()

    def blocking_provider_call():
        started.set()
        release.wait(timeout=5)
        finished.set()

    call = asyncio.create_task(
        gemini_provider.run_provider_call(
            capacity,
            blocking_provider_call,
            busy_error=RuntimeError("busy"),
            deadline=gemini_provider.monotonic() + 0.01,
        )
    )
    assert await asyncio.to_thread(started.wait, 2)
    with pytest.raises(TimeoutError):
        await call
    with pytest.raises(RuntimeError, match="busy"):
        await gemini_provider.run_provider_call(
            capacity,
            lambda: None,
            busy_error=RuntimeError("busy"),
        )

    release.set()
    assert await asyncio.to_thread(finished.wait, 2)
