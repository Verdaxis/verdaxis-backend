"""Synthetic retired activity stream router (fixture data only)."""
from fastapi import APIRouter

router = APIRouter()

@router.get("/stream/activity")
async def activity_stream():
    return {"events": []}
