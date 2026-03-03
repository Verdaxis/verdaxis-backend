"""KYC document verification using Google Gemini Vision."""
import json
import structlog
from app.config import settings

logger = structlog.get_logger()

_AUTO_APPROVED_RESULT = {
    "valid": True,
    "readable": True,
    "tampered": False,
    "confidence": 1.0,
    "issues": ["GEMINI_API_KEY not configured — auto-approved"],
    "passed": True,
}


async def verify_document_with_gemini(
    image_bytes: bytes,
    mime_type: str,
    doc_type: str,
) -> dict:
    """
    Submit a document image to Gemini for KYC verification.

    Returns a dict with keys:
        valid: bool        — recognisable official document
        readable: bool     — text/details clearly legible
        tampered: bool     — signs of digital manipulation
        confidence: float  — 0.0-1.0
        issues: list[str]  — specific problems found
        passed: bool       — valid AND not tampered AND confidence > 0.7
    """
    if not settings.GEMINI_API_KEY:
        logger.warning("kyc_gemini_skipped", reason="GEMINI_API_KEY not configured", doc_type=doc_type)
        return _AUTO_APPROVED_RESULT

    try:
        import google.generativeai as genai
        import google.generativeai.types as gtypes

        genai.configure(api_key=settings.GEMINI_API_KEY)

        prompt = f"""You are a KYC document verification system for a maritime fuel marketplace. \
Carefully examine this {doc_type} document image.

Respond with ONLY valid JSON (no markdown, no explanation):
{{
  "valid": true,
  "readable": true,
  "tampered": false,
  "confidence": 0.85,
  "issues": []
}}

Fields:
- valid: true if this is a recognizable official document, false otherwise
- readable: true if text and details can be clearly read
- tampered: true if there are signs of digital editing or manipulation
- confidence: your confidence in this assessment (0.0-1.0)
- issues: list any specific problems found"""

        image_part = {"mime_type": mime_type, "data": image_bytes}

        # Try gemini-2.0-flash-lite first, fall back to gemini-1.5-flash-8b
        for model_name in ("gemini-2.0-flash-lite", "gemini-1.5-flash-8b"):
            try:
                model = genai.GenerativeModel(model_name)
                response = model.generate_content([prompt, image_part])
                break
            except Exception as model_exc:
                err_str = str(model_exc)
                if "not found" in err_str.lower() or "invalid" in err_str.lower() or "404" in err_str:
                    logger.warning("kyc_model_fallback", attempted=model_name, error=err_str)
                    continue
                raise
        else:
            raise RuntimeError("All Gemini model names exhausted")

        raw_text = response.text.strip()

        # Strip markdown code fences if present
        if raw_text.startswith("```"):
            lines = raw_text.splitlines()
            raw_text = "\n".join(
                line for line in lines
                if not line.startswith("```")
            ).strip()

        result = json.loads(raw_text)

        # Ensure all expected keys exist with safe defaults
        result.setdefault("valid", False)
        result.setdefault("readable", False)
        result.setdefault("tampered", True)
        result.setdefault("confidence", 0.0)
        result.setdefault("issues", [])

        result["passed"] = (
            bool(result["valid"])
            and not bool(result["tampered"])
            and float(result["confidence"]) > 0.7
        )

        logger.info(
            "kyc_gemini_result",
            doc_type=doc_type,
            passed=result["passed"],
            confidence=result["confidence"],
        )
        return result

    except json.JSONDecodeError as exc:
        logger.error("kyc_gemini_parse_error", doc_type=doc_type, error=str(exc))
        return {
            "valid": False,
            "readable": False,
            "tampered": False,
            "confidence": 0.0,
            "issues": [f"Could not parse Gemini response: {exc}"],
            "passed": False,
        }
    except Exception as exc:
        logger.error("kyc_gemini_error", doc_type=doc_type, error=str(exc))
        return {
            "valid": False,
            "readable": False,
            "tampered": False,
            "confidence": 0.0,
            "issues": [f"Gemini verification error: {exc}"],
            "passed": False,
        }
