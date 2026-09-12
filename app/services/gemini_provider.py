"""PID-aware Gemini client cache and cancellation-safe thread capacity."""

import asyncio
from collections.abc import Callable
import os
import threading
from time import monotonic
from typing import TypeVar

from google import genai
from google.genai import types

from app.config import settings

T = TypeVar("T")

DEFAULT_GEMINI_MODEL = "gemini-2.5-flash-lite"
PROVIDER_ATTEMPTS = 2
PROVIDER_RETRY_DELAY_SECONDS = 0.1

_client_lock = threading.RLock()
_client_pid: int | None = None
_client: genai.Client | None = None


def _close_cached_client() -> None:
    global _client
    if _client is not None:
        try:
            _client.close()
        except Exception:
            pass
    _client = None


def _client_for_process() -> genai.Client:
    """Return one shared client per process."""
    global _client, _client_pid
    if not settings.GEMINI_API_KEY:
        raise RuntimeError("Gemini provider is not configured")

    process_id = os.getpid()
    with _client_lock:
        if _client_pid != process_id:
            _close_cached_client()
            _client_pid = process_id
        if _client is None:
            _client = genai.Client(api_key=settings.GEMINI_API_KEY)
        return _client


def _request_options(timeout_seconds: float) -> types.HttpOptions:
    total_timeout_ms = int(timeout_seconds * 1_000)
    retry_delay_ms = int(PROVIDER_RETRY_DELAY_SECONDS * 1_000)
    attempt_timeout_ms = (total_timeout_ms - retry_delay_ms) // PROVIDER_ATTEMPTS
    if attempt_timeout_ms < 1:
        raise TimeoutError("Gemini provider deadline expired")
    return types.HttpOptions(
        timeout=attempt_timeout_ms,
        retry_options=types.HttpRetryOptions(
            attempts=PROVIDER_ATTEMPTS,
            initial_delay=PROVIDER_RETRY_DELAY_SECONDS,
            max_delay=PROVIDER_RETRY_DELAY_SECONDS,
            exp_base=1,
            jitter=0,
        ),
    )


def generate_content(model_name: str, contents: object, timeout_seconds: float):
    """Generate content within the caller's total timeout budget."""
    config = types.GenerateContentConfig(
        automatic_function_calling=types.AutomaticFunctionCallingConfig(disable=True),
        http_options=_request_options(timeout_seconds),
    )
    return _client_for_process().models.generate_content(
        model=model_name,
        contents=contents,
        config=config,
    )


def inline_data_part(data: bytes, mime_type: str) -> types.Part:
    """Build an SDK-native inline binary part."""
    return types.Part.from_bytes(data=data, mime_type=mime_type)


class ProviderCapacity:
    """Non-blocking per-process capacity retained until a thread really exits."""

    def __init__(self, limit: int):
        self._semaphore = threading.BoundedSemaphore(limit)

    def acquire(self) -> bool:
        return self._semaphore.acquire(blocking=False)

    def release(self) -> None:
        self._semaphore.release()


def _call_and_release(capacity: ProviderCapacity, call: Callable[..., T], args: tuple) -> T:
    try:
        return call(*args)
    finally:
        # This runs in the worker thread only after the provider call really
        # returns, even if the awaiting request was cancelled meanwhile.
        capacity.release()


def _consume_worker_exception(worker: asyncio.Task) -> None:
    if not worker.cancelled():
        worker.exception()


async def run_provider_call(
    capacity: ProviderCapacity,
    call: Callable[..., T],
    *args,
    busy_error: Exception,
    deadline: float | None = None,
) -> T:
    if not capacity.acquire():
        raise busy_error
    try:
        worker = asyncio.create_task(asyncio.to_thread(_call_and_release, capacity, call, args))
    except BaseException:
        capacity.release()
        raise
    worker.add_done_callback(_consume_worker_exception)
    protected_worker = asyncio.shield(worker)
    if deadline is None:
        return await protected_worker
    return await asyncio.wait_for(protected_worker, timeout=max(0, deadline - monotonic()))
