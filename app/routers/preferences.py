from datetime import UTC, datetime
import json
from typing import Any

from fastapi import APIRouter, Body, Depends, HTTPException, Request, status
from pydantic import ValidationError
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.user import User
from app.models.user_preference import UserPreference
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.schemas.preferences import NAMESPACE_SCHEMAS


MAX_PREFERENCE_BYTES = 8 * 1024

router = APIRouter(prefix="/users/me/preferences", tags=["Preferences"])


def _validate_preference_value(namespace: str, value: Any) -> Any:
    schema = NAMESPACE_SCHEMAS.get(namespace)
    if schema is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Unknown preference namespace")

    serialized = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(serialized) > MAX_PREFERENCE_BYTES:
        raise HTTPException(status_code=413, detail="Preference payload too large")

    try:
        schema.model_validate(value)
        return value
    except ValidationError as exc:
        raise HTTPException(status_code=422, detail=exc.errors()) from exc


@router.get("")
async def get_preferences(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    result = await db.execute(
        select(UserPreference).where(UserPreference.user_id == current_user.id)
    )
    return {
        preference.namespace: preference.value
        for preference in result.scalars().all()
    }


@router.put("/{namespace}")
@limiter.limit("30/minute")
async def put_preference(
    request: Request,
    namespace: str,
    value: Any = Body(...),
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    validated_value = _validate_preference_value(namespace, value)
    now = datetime.now(UTC)

    result = await db.execute(
        select(UserPreference).where(
            UserPreference.user_id == current_user.id,
            UserPreference.namespace == namespace,
        )
    )
    preference = result.scalar_one_or_none()

    if preference is None:
        preference = UserPreference(
            user_id=current_user.id,
            namespace=namespace,
            value=validated_value,
            updated_at=now,
        )
        db.add(preference)
    else:
        preference.value = validated_value
        preference.updated_at = now

    await db.commit()

    return {
        "value": preference.value,
        "updated_at": preference.updated_at.isoformat(),
    }
