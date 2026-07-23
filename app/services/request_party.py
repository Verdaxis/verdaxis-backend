"""Resolve the authenticated actor and optional Market Support effective party."""
from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import UTC, datetime
from enum import Enum
from uuid import UUID

from fastapi import HTTPException, Request, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import settings
from app.models.market_support import (
    MarketSupportCapability,
    MarketSupportContext,
    MarketSupportContextScope,
    MarketSupportContextStatus,
    StaffCapabilityAssignment,
)
from app.models.orderbook import OrderCreationMethod
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from app.services.execution_policy import execution_party_is_eligible
from app.services.market_support import utc


MARKET_SUPPORT_CONTEXT_HEADER = "X-Verdaxis-Market-Support-Context"
MARKET_SUPPORT_CONTEXT_INVALID_HEADER = "X-Verdaxis-Market-Support-Context-Invalid"
_ORDER_ID = r"[0-9a-fA-F-]{36}"
_CANCEL_PATH = re.compile(rf"^/api/orderbook/{_ORDER_ID}/cancel/?$")
_CONTEXT_EXIT_PATH = re.compile(
    rf"^/api/admin/market-support/contexts/{_ORDER_ID}/exit/?$"
)
KNOWN_STATE_CHANGING_GET_PATHS = frozenset(
    {
        "/api/auth/verify-email",
        "/api/referrals/my-code",
        "/api/verify-email",
        "/api/watchlists/me",
    }
)


class RequestPartyMode(str, Enum):
    SELF_SERVICE = "SELF_SERVICE"
    MARKET_SUPPORT = "MARKET_SUPPORT"


@dataclass(frozen=True)
class RequestParty:
    actor: User
    effective_organization: Organization | None
    accountable_principal: User
    effective_role: UserRole
    mode: RequestPartyMode = RequestPartyMode.SELF_SERVICE
    support_context_id: UUID | None = None
    support_reference: str | None = None
    creation_method: OrderCreationMethod = OrderCreationMethod.SELF_SERVICE


def _context_error(code: str, message: str) -> HTTPException:
    return HTTPException(
        status_code=status.HTTP_409_CONFLICT,
        detail={"code": code, "message": message},
    )


def is_known_state_changing_get(method: str, path: str) -> bool:
    return method.upper() == "GET" and path.rstrip("/") in KNOWN_STATE_CHANGING_GET_PATHS


def _context_locator(request: Request | None) -> UUID | None:
    if request is None:
        return None
    raw = request.headers.get(MARKET_SUPPORT_CONTEXT_HEADER)
    if raw is None or not raw.strip():
        return None
    try:
        return UUID(raw.strip())
    except ValueError as exc:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail={"code": "MARKET_SUPPORT_CONTEXT_INVALID", "message": "Invalid support context"},
        ) from exc


async def _require_context_capabilities(db: AsyncSession, actor_id: UUID) -> None:
    assignments = (
        await db.execute(
            select(StaffCapabilityAssignment).where(
                StaffCapabilityAssignment.user_id == actor_id,
                StaffCapabilityAssignment.revoked_at.is_(None),
            )
        )
    ).scalars().all()
    assignment_by_capability = {row.capability: row for row in assignments}
    now = datetime.now(UTC)
    for capability in (
        MarketSupportCapability.MARKET_SUPPORT_LISTINGS,
        MarketSupportCapability.MARKET_SUPPORT_AUTHORIZATIONS,
    ):
        assignment = assignment_by_capability.get(capability)
        if (
            assignment is None
            or (
                assignment.expires_at is not None
                and utc(assignment.expires_at) <= now
            )
        ):
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Not found")


async def lock_request_party_capabilities(db: AsyncSession, actor_id: UUID) -> None:
    """Lock and revalidate both capabilities in one deterministic order."""
    now = datetime.now(UTC)
    for capability in sorted(
        (
            MarketSupportCapability.MARKET_SUPPORT_LISTINGS,
            MarketSupportCapability.MARKET_SUPPORT_AUTHORIZATIONS,
        ),
        key=lambda value: value.value,
    ):
        assignment = (
            await db.execute(
                select(StaffCapabilityAssignment)
                .where(
                    StaffCapabilityAssignment.user_id == actor_id,
                    StaffCapabilityAssignment.capability == capability,
                )
                .with_for_update()
                .execution_options(populate_existing=True)
            )
        ).scalar_one_or_none()
        if (
            assignment is None
            or assignment.revoked_at is not None
            or (
                assignment.expires_at is not None
                and utc(assignment.expires_at) <= now
            )
        ):
            raise _context_error(
                "MARKET_SUPPORT_CONTEXT_INVALID",
                "Market Support capability is no longer active",
            )


async def resolve_request_party(
    request: Request | None,
    db: AsyncSession,
    current_user: User,
    *,
    operation: str | None = None,
) -> RequestParty:
    """Return an immutable request party; a header never selects tenancy alone."""
    context_id = _context_locator(request)
    if context_id is None:
        organization = None
        organization_id = getattr(current_user, "organization_id", None)
        if isinstance(current_user, User) and organization_id:
            organization = (
                await db.execute(
                    select(Organization).where(Organization.id == organization_id)
                )
            ).scalar_one_or_none()
        return RequestParty(
            actor=current_user,
            effective_organization=organization,
            accountable_principal=current_user,
            effective_role=current_user.role,
            mode=RequestPartyMode.SELF_SERVICE,
        )

    # Context identifiers are non-enumerating for unauthorised/foreign users.
    if (
        not settings.MARKET_SUPPORT_ENABLED
        or current_user.role != UserRole.ADMIN
        or current_user.status != UserStatus.APPROVED
    ):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail={"code": "MARKET_SUPPORT_CONTEXT_INVALID", "message": "Not found"},
        )
    await _require_context_capabilities(db, current_user.id)
    context = (
        await db.execute(
            select(MarketSupportContext).where(MarketSupportContext.id == context_id)
        )
    ).scalar_one_or_none()
    if context is None or context.actor_user_id != current_user.id:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail={"code": "MARKET_SUPPORT_CONTEXT_INVALID", "message": "Not found"},
        )

    now = datetime.now(UTC)
    if context.status != MarketSupportContextStatus.ACTIVE:
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support context is no longer active")
    if utc(context.expires_at) <= now:
        context.status = MarketSupportContextStatus.EXPIRED
        context.ended_at = now
        context.version += 1
        await db.commit()
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support context has expired")
    if context.scope != MarketSupportContextScope.ASK_LISTINGS:
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support scope is not permitted")

    organization = (
        await db.execute(select(Organization).where(Organization.id == context.organization_id))
    ).scalar_one_or_none()
    principal = (
        await db.execute(select(User).where(User.id == context.accountable_user_id))
    ).scalar_one_or_none()
    if (
        organization is None
        or principal is None
        or organization.verification_status != "APPROVED"
        or organization.provenance != OrganizationProvenance.REAL
        or principal.organization_id != organization.id
        or principal.role != UserRole.SUPPLIER
    ):
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support context target is no longer valid")
    # Cleanup cancellation remains available after the principal loses
    # eligibility; context entry and all other operations remain strict.
    if operation != "cancel_order" and not await execution_party_is_eligible(
        db, user=principal, organization=organization
    ):
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support context target is no longer eligible")

    return RequestParty(
        actor=current_user,
        effective_organization=organization,
        accountable_principal=principal,
        effective_role=UserRole.SUPPLIER,
        mode=RequestPartyMode.MARKET_SUPPORT,
        support_context_id=context.id,
        support_reference=context.support_reference,
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )


async def lock_request_party_context(
    db: AsyncSession, party: RequestParty
) -> None:
    """Lock the context after market-slice locking and recheck its lifecycle."""
    if party.mode != RequestPartyMode.MARKET_SUPPORT or party.support_context_id is None:
        return
    # Capability revocation takes the same locks first.  Rechecking here
    # closes the gap between request-party resolution and the final mutation.
    await lock_request_party_capabilities(db, party.actor.id)
    context = (
        await db.execute(
            select(MarketSupportContext)
            .where(
                MarketSupportContext.id == party.support_context_id,
                MarketSupportContext.actor_user_id == party.actor.id,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    if context is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Not found")
    if context.status != MarketSupportContextStatus.ACTIVE:
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support context is no longer active")
    if utc(context.expires_at) <= datetime.now(UTC):
        context.status = MarketSupportContextStatus.EXPIRED
        context.ended_at = datetime.now(UTC)
        context.version += 1
        await db.commit()
        raise _context_error("MARKET_SUPPORT_CONTEXT_INVALID", "Support context has expired")


def is_market_support_mutation_allowed(
    method: str, path: str, *, context_id: UUID | None
) -> bool:
    """Deny-by-default route classification for requests carrying a context."""
    if is_known_state_changing_get(method, path):
        return False
    if context_id is None or method.upper() in {"GET", "HEAD", "OPTIONS"}:
        return True
    normalized = path if path.startswith("/api/") else f"/api/{path.lstrip('/')}"
    method = method.upper()
    if method == "POST" and normalized.rstrip("/") == "/api/orderbook":
        return True
    if method == "POST" and _CANCEL_PATH.fullmatch(normalized):
        return True
    if method == "POST" and _CONTEXT_EXIT_PATH.fullmatch(normalized):
        return True
    return False
