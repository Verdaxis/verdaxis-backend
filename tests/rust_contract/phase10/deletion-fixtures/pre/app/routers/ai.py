"""Synthetic retired AI chat router (fixture data only)."""
from fastapi import APIRouter

router = APIRouter()

@router.post("/ai/chat")
async def ai_chat():
    return {"reply": ""}
