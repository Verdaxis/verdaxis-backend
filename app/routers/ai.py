from fastapi import APIRouter, Depends, Body
from app.services.ai_service import chat_with_copilot
from app.models.user import User
from app.core.auth import get_current_user
from typing import Annotated, List, Dict

router = APIRouter()

@router.post("/ai/chat")
async def chat(
    message: str = Body(..., embed=True),
    history: List[Dict[str, str]] = Body([], embed=True),
    current_user: Annotated[User, Depends(get_current_user)] = None
):
    response = await chat_with_copilot(message, history)
    return {"response": response}
