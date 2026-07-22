"""Locked user ownership and organization admission revalidation.

Callers acquire idempotency and sorted market-slice advisory locks, then lock
all affected market rows before entering this seam.  This module deliberately
uses row locks only; a second organization advisory namespace would create a
competing lock graph with security-owned approval changes.
"""
from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.user import Organization, User, UserStatus
from app.services.demo_market import DEMO_ACTIVITY_ORG_IDS
from app.services.provenance import execution_provenance_compatible


APPROVED_ORGANIZATION_STATUS = "APPROVED"
APPROVED_KYC_STATUS = "APPROVED"


@dataclass(frozen=True)
class MarketActorOwnership:
    """One concrete authenticated user-to-organization ownership assertion."""

    user_id: UUID
    organization_id: UUID


async def lock_and_load_market_organizations(
    db: AsyncSession,
    organization_ids: Iterable[UUID],
    *,
    actor_ownerships: Iterable[MarketActorOwnership] = (),
    require_approved: bool = True,
    require_approved_ids: Iterable[UUID] = (),
) -> dict[UUID, Organization]:
    """Lock users, then organizations, after every affected market row.

    Security owns the eligibility decision exposed at the route dependency;
    market owns this transaction-local revalidation and exact user ownership
    check.  Future security ``owner_user_id`` columns can be passed here
    without introducing another lock namespace.
    """
    ids = tuple(sorted(set(organization_ids), key=str))
    if not ids:
        return {}

    ownership_by_user = {
        ownership.user_id: ownership.organization_id
        for ownership in actor_ownerships
    }
    if ownership_by_user:
        user_ids = tuple(sorted(ownership_by_user, key=str))
        users = {
            user.id: user
            for user in (
                await db.execute(
                    select(User)
                    .where(User.id.in_(user_ids))
                    .order_by(User.id)
                    .with_for_update()
                    # The actor row is always already in the session identity
                    # map (auth dependency); refresh it under the lock so a
                    # mid-request rejection is observed.
                    .execution_options(populate_existing=True)
                )
            ).scalars()
        }
        if set(users) != set(user_ids):
            raise HTTPException(status_code=409, detail="Market user no longer exists")
        for user_id, organization_id in ownership_by_user.items():
            user = users[user_id]
            if user.organization_id != organization_id:
                raise HTTPException(
                    status_code=409,
                    detail="Market user organization ownership changed",
                )
            if (
                user.status != UserStatus.APPROVED
                or str(user.kyc_status or "").upper() != APPROVED_KYC_STATUS
            ):
                raise HTTPException(
                    status_code=409,
                    detail="User is not approved for market activity",
                )

    organizations = {
        organization.id: organization
        for organization in (
            await db.execute(
                select(Organization)
                .where(Organization.id.in_(ids))
                .order_by(Organization.id)
                .with_for_update()
                .execution_options(populate_existing=True)
            )
        ).scalars()
    }
    missing = set(ids).difference(organizations)
    if missing:
        raise HTTPException(status_code=409, detail="Market organization no longer exists")
    approved_ids = set(ids) if require_approved else set(require_approved_ids)
    if any(
        organization_id in approved_ids
        and str(organization.verification_status or "").upper()
        != APPROVED_ORGANIZATION_STATUS
        for organization_id, organization in organizations.items()
    ):
        raise HTTPException(
            status_code=409,
            detail="Organization is not approved for market activity",
        )
    return organizations


def assert_market_participant_provenance(organization: Organization) -> None:
    """Admit REAL or an explicitly allowlisted demo-activity participant."""
    provenance = organization.provenance
    if str(getattr(provenance, "value", provenance)) == "REAL":
        return
    if (
        str(getattr(provenance, "value", provenance)) == "DEMO"
        and organization.id in DEMO_ACTIVITY_ORG_IDS
    ):
        return
    raise HTTPException(
        status_code=409,
        detail="Organization provenance is not eligible for market activity",
    )


def assert_market_pair_provenance(
    left: Organization,
    right: Organization,
) -> None:
    """Admit only REAL/REAL or the one canonical demo activity pair."""
    if execution_provenance_compatible(
        left.provenance,
        right.provenance,
        left_org_id=left.id,
        right_org_id=right.id,
        allowed_demo_org_pair=DEMO_ACTIVITY_ORG_IDS,
    ):
        return
    raise HTTPException(
        status_code=409,
        detail="Market parties do not have compatible provenance",
    )
