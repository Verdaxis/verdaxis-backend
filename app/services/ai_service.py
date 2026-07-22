"""Bounded, failure-isolated Gemini copilot calls."""

from app.config import settings
from app.services.gemini_provider import (
    ProviderCapacity,
    get_gemini_model,
    request_options,
    run_provider_call,
)

AI_PROVIDER_TIMEOUT_SECONDS = 20
MAX_CONCURRENT_AI_PROVIDER_CALLS = 4
_provider_capacity = ProviderCapacity(MAX_CONCURRENT_AI_PROVIDER_CALLS)


class AIProviderUnavailable(RuntimeError):
    pass


def _chat_sync(message: str) -> str:
    if not settings.GEMINI_API_KEY:
        raise AIProviderUnavailable("AI provider is not configured")
    model = get_gemini_model("gemini-2.0-flash-lite")
    response = model.generate_content(
        message,
        request_options=request_options(AI_PROVIDER_TIMEOUT_SECONDS),
    )
    text = getattr(response, "text", None)
    if not isinstance(text, str) or not text.strip():
        raise AIProviderUnavailable("AI provider returned an empty response")
    return text[:16_000]


async def chat_with_copilot(message: str, history: list | None = None):
    del history
    if not settings.GEMINI_API_KEY:
        raise AIProviderUnavailable("AI provider is not configured")
    try:
        return await run_provider_call(
            _provider_capacity,
            _chat_sync,
            message,
            busy_error=AIProviderUnavailable("AI provider is busy"),
        )
    except AIProviderUnavailable:
        raise
    except Exception as exc:
        raise AIProviderUnavailable("AI provider is temporarily unavailable") from exc
