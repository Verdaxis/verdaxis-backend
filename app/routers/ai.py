from typing import Annotated

from fastapi import APIRouter, Depends, HTTPException, Request, status
from slowapi.util import get_remote_address

from app.models.user import User
from app.rate_limit import limiter
from app.routing import BodySizeLimitRoute
from app.routers.auth_simple import get_current_user
from app.schemas.ai import AIChatRequest
from app.services.ai_service import AIProviderUnavailable, chat_with_copilot

MAX_AI_CHAT_BODY_BYTES = 32 * 1024


class _AIChatBodyLimitRoute(BodySizeLimitRoute):
    max_body_bytes = MAX_AI_CHAT_BODY_BYTES
    body_too_large_detail = "AI chat payload must be 32 KB or smaller"


router = APIRouter(route_class=_AIChatBodyLimitRoute)


async def _get_ai_current_user(request: Request, current_user: Annotated[User, Depends(get_current_user)]) -> User:
    request.state.ai_rate_user_id = str(current_user.id)
    request.state.ai_rate_org_id = str(current_user.organization_id or current_user.id)
    return current_user


def _ai_user_rate_key(request: Request) -> str:
    user_id = getattr(request.state, "ai_rate_user_id", None)
    return f"ai:user:{user_id}" if user_id else f"ai:ip:{get_remote_address(request)}"


def _ai_org_rate_key(request: Request) -> str:
    org_id = getattr(request.state, "ai_rate_org_id", None)
    return f"ai:org:{org_id}" if org_id else f"ai:ip:{get_remote_address(request)}"


@router.post("/ai/chat")
@limiter.limit("30/minute", key_func=_ai_org_rate_key)
@limiter.limit("10/minute", key_func=_ai_user_rate_key)
async def chat(request: Request, payload: AIChatRequest, current_user: Annotated[User, Depends(_get_ai_current_user)]):
    try:
        return {"response": await chat_with_copilot(payload.message, payload.history)}
    except AIProviderUnavailable as exc:
        raise HTTPException(status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail="AI service is temporarily unavailable") from exc
