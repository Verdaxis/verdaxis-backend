"""PID-aware Gemini model cache and cancellation-safe thread capacity."""

import asyncio
from collections.abc import Callable
import os
import threading
from typing import TypeVar

import google.generativeai as genai
from google.api_core import retry as google_retry
from google.generativeai.types import RequestOptions

from app.config import settings

T = TypeVar("T")

_model_lock = threading.RLock()
_model_pid: int | None = None
_models: dict[str, object] = {}


def get_gemini_model(model_name: str):
    """Return one initialized model per process and name, safe after fork."""
    global _model_pid
    if not settings.GEMINI_API_KEY:
        raise RuntimeError("Gemini provider is not configured")
    process_id = os.getpid()
    with _model_lock:
        if _model_pid != process_id:
            _models.clear()
            _model_pid = process_id
        model = _models.get(model_name)
        if model is None:
            genai.configure(api_key=settings.GEMINI_API_KEY)
            model = genai.GenerativeModel(model_name)
            _models[model_name] = model
        return model


def request_options(timeout_seconds: float) -> RequestOptions:
    """Build a provider-native RPC timeout and bounded retry deadline."""
    return RequestOptions(
        timeout=timeout_seconds,
        retry=google_retry.Retry(deadline=timeout_seconds),
    )


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


async def run_provider_call(
    capacity: ProviderCapacity,
    call: Callable[..., T],
    *args,
    busy_error: Exception,
) -> T:
    if not capacity.acquire():
        raise busy_error
    try:
        worker = asyncio.create_task(asyncio.to_thread(_call_and_release, capacity, call, args))
    except BaseException:
        capacity.release()
        raise
    return await asyncio.shield(worker)
