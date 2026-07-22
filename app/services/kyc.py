"""Fail-closed, bounded Google Gemini KYC verification."""

import json
from typing import Any

import structlog

from app.config import settings
from app.services.gemini_provider import (
    ProviderCapacity,
    get_gemini_model,
    request_options,
    run_provider_call,
)

logger = structlog.get_logger()
KYC_REVIEW_REQUIRED = "SUBMITTED"
KYC_PROVIDER_TIMEOUT_SECONDS = 30
MAX_CONCURRENT_KYC_PROVIDER_CALLS = 2
_provider_capacity = ProviderCapacity(MAX_CONCURRENT_KYC_PROVIDER_CALLS)


class KYCProviderUnavailable(RuntimeError):
    """The external verifier cannot produce a trustworthy result."""


def _parse_provider_result(raw_text: str) -> dict[str, Any]:
    if raw_text.startswith("```"):
        raw_text = "\n".join(line for line in raw_text.splitlines() if not line.startswith("```"))
    result = json.loads(raw_text)
    if not isinstance(result, dict):
        raise ValueError("KYC provider response must be a JSON object")
    valid, readable, tampered = (result.get(key) for key in ("valid", "readable", "tampered"))
    confidence = result.get("confidence")
    issues = result.get("issues")
    if not all(isinstance(value, bool) for value in (valid, readable, tampered)):
        raise ValueError("KYC provider returned invalid boolean fields")
    if isinstance(confidence, bool) or not isinstance(confidence, (int, float)) or not 0 <= float(confidence) <= 1:
        raise ValueError("KYC provider returned invalid confidence")
    if not isinstance(issues, list) or not all(isinstance(issue, str) for issue in issues):
        raise ValueError("KYC provider returned invalid issues")
    normalized_issues = [issue[:500] for issue in issues[:20]]
    return {
        "valid": valid, "readable": readable, "tampered": tampered,
        "confidence": float(confidence), "issues": normalized_issues,
        "passed": valid and readable and not tampered and float(confidence) > 0.7,
    }


def _verify_document_with_gemini_sync(image_bytes: bytes, mime_type: str, doc_type: str) -> dict[str, Any]:
    prompt = f"""You are a KYC document verification system for a maritime fuel marketplace. Carefully examine this {doc_type} document image.

Respond with ONLY valid JSON (no markdown, no explanation):
{{"valid": true, "readable": true, "tampered": false, "confidence": 0.85, "issues": []}}

Fields: valid recognizable official document; readable clearly legible; tampered signs of manipulation; confidence 0.0-1.0; issues specific problems."""
    image_part = {"mime_type": mime_type, "data": image_bytes}
    for model_name in ("gemini-2.0-flash-lite", "gemini-1.5-flash-8b"):
        try:
            response = get_gemini_model(model_name).generate_content(
                [prompt, image_part],
                request_options=request_options(KYC_PROVIDER_TIMEOUT_SECONDS),
            )
            return _parse_provider_result(response.text.strip())
        except Exception as exc:
            message = str(exc).lower()
            if "model" in message and ("not found" in message or "404" in message):
                logger.warning("kyc_model_fallback", attempted=model_name, error_type=type(exc).__name__)
                continue
            raise
    raise KYCProviderUnavailable("No configured KYC model is available")


async def verify_document_with_gemini(image_bytes: bytes, mime_type: str, doc_type: str) -> dict:
    if not settings.GEMINI_API_KEY:
        raise KYCProviderUnavailable("KYC provider is not configured")
    try:
        result = await run_provider_call(
            _provider_capacity,
            _verify_document_with_gemini_sync,
            image_bytes,
            mime_type,
            doc_type,
            busy_error=KYCProviderUnavailable("KYC provider is busy"),
        )
        logger.info("kyc_gemini_result", doc_type=doc_type, passed=result["passed"], confidence=result["confidence"])
        return result
    except KYCProviderUnavailable:
        raise
    except Exception as exc:
        logger.error("kyc_provider_unavailable", doc_type=doc_type, error_type=type(exc).__name__)
        raise KYCProviderUnavailable("KYC provider could not verify the document") from exc
