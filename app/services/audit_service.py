"""Audit logging service — records security and business events."""
import uuid
from typing import Any

from sqlalchemy.ext.asyncio import AsyncSession

from app.models.audit import AuditLog


async def record_audit(
    db: AsyncSession,
    *,
    user_id: uuid.UUID | None = None,
    action: str,
    resource_type: str,
    resource_id: str | None = None,
    changes: dict[str, Any] | None = None,
    ip_address: str | None = None,
    request_id: str | None = None,
) -> AuditLog:
    """Create an audit log entry. Call within the same DB transaction as the audited action."""
    entry = AuditLog(
        user_id=user_id,
        action=action,
        resource_type=resource_type,
        resource_id=str(resource_id) if resource_id else None,
        changes=changes,
        ip_address=ip_address,
        request_id=request_id,
    )
    db.add(entry)
    # Don't commit — let the caller's transaction handle it
    return entry
