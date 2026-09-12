"""Bounded, failure-isolated Gemini copilot calls."""

from time import monotonic

from app.config import settings
from app.services.gemini_provider import (
    DEFAULT_GEMINI_MODEL,
    ProviderCapacity,
    generate_content,
    run_provider_call,
)

AI_PROVIDER_TIMEOUT_SECONDS = 20
MAX_CONCURRENT_AI_PROVIDER_CALLS = 4
_provider_capacity = ProviderCapacity(MAX_CONCURRENT_AI_PROVIDER_CALLS)


class AIProviderUnavailable(RuntimeError):
    pass


def _chat_sync(message: str, deadline: float | None = None) -> str:
    if not settings.GEMINI_API_KEY:
        raise AIProviderUnavailable("AI provider is not configured")
    timeout_seconds = AI_PROVIDER_TIMEOUT_SECONDS if deadline is None else int(deadline - monotonic())
    if timeout_seconds < 1:
        raise AIProviderUnavailable("AI provider deadline expired")
    response = generate_content(
        DEFAULT_GEMINI_MODEL,
        message,
        timeout_seconds,
    )
    text = getattr(response, "text", None)
    if not isinstance(text, str) or not text.strip():
        raise AIProviderUnavailable("AI provider returned an empty response")
    return text[:16_000]


async def chat_with_copilot(message: str, history: list | None = None):
    del history
    if not settings.GEMINI_API_KEY:
        raise AIProviderUnavailable("AI provider is not configured")
    deadline = monotonic() + AI_PROVIDER_TIMEOUT_SECONDS
    try:
        return await run_provider_call(
            _provider_capacity,
            _chat_sync,
            message,
            deadline,
            busy_error=AIProviderUnavailable("AI provider is busy"),
            deadline=deadline,
        )
    except AIProviderUnavailable:
        raise
    except Exception as exc:
        raise AIProviderUnavailable("AI provider is temporarily unavailable") from exc
