"""Audit logging service — records security and business events."""
import uuid
from typing import Any

from fastapi import Request
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.audit import AuditLog


def request_audit_context(request: Request) -> dict[str, str | None]:
    """IP + correlation id kwargs for record_audit, derived from the request."""
    from app.main import request_id_ctx  # lazy: main imports the routers that import us
    from app.middleware.preauth_rate_limit import client_ip

    return {
        "ip_address": client_ip(request),
        "request_id": request_id_ctx.get() or request.headers.get("X-Request-ID"),
    }


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
    """Create an audit log entry in the caller's transaction.

    Delegated market-support routes invoke the ordinary order application
    functions with the customer's accountable principal. The request-local
    support context preserves the actual authenticated administrator as the
    audit actor and adds structured, non-sensitive attribution metadata.

    An ordinary edit/cancel by the exact customer owner atomically hands a
    support-managed listing back to direct customer management. That prevents
    a later mandate revocation or support operator from overriding customer
    changes.
    """
    from app.services.market_support_context import current_market_support_action

    support_context = current_market_support_action()
    if support_context is not None:
        user_id = support_context.actor_user_id
        changes = {
            **(changes or {}),
            "market_support": {
                "target_organization_id": str(
                    support_context.target_organization_id
                ),
                "accountable_user_id": str(support_context.accountable_user_id),
                "support_authorization_id": str(
                    support_context.support_authorization_id
                ),
                "submission_method": "VERDAXIS_ASSISTED",
                "operation": support_context.operation,
                "reason_code": support_context.reason_code,
                "support_case_reference": support_context.support_case_reference,
                "support_version": support_context.support_version,
            },
        }
    elif hasattr(db, "execute"):
        from app.services.market_support_handoff import (
            adopt_assisted_order_for_customer_action,
        )

        handoff = await adopt_assisted_order_for_customer_action(
            db,
            action=action,
            resource_type=resource_type,
            resource_id=resource_id,
            actor_user_id=user_id,
        )
        if handoff is not None:
            changes = {
                **(changes or {}),
                "market_support_handoff": {
                    "management_authority": "CUSTOMER_DIRECT",
                    "reason_code": handoff.reason_code,
                    "support_version": {
                        "from": handoff.previous_support_version,
                        "to": handoff.current_support_version,
                    },
                },
            }

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
