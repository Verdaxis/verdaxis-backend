"""Immutable organization provenance and execution policy helpers.

The registries in this module are deliberately ID-only. Organization names and
domains are mutable metadata and must never make a market-data claim.
"""
from __future__ import annotations

from collections.abc import Set
from uuid import UUID

from app.models.user import Organization, OrganizationProvenance
from app.demo_identities import KNOWN_TEST_ORG_IDS
from app.services.demo_market import DEMO_MARKET_ORG_IDS

# Dedicated staging integration identities documented in the pilot runbook.
# Keep this list explicit; adding an identity is an operational change.
INTEGRATION_TEST_ORG_IDS = KNOWN_TEST_ORG_IDS
KNOWN_CANARY_ORG_IDS: frozenset[UUID] = frozenset()


def coerce_provenance(value: object) -> OrganizationProvenance:
    """Normalize ORM/test values without treating arbitrary values as REAL."""
    if isinstance(value, OrganizationProvenance):
        return value
    if isinstance(value, str):
        try:
            return OrganizationProvenance(value)
        except ValueError:
            pass
    return OrganizationProvenance.UNKNOWN


def classify_organization_provenance(organization_id: UUID | None) -> OrganizationProvenance:
    if organization_id in DEMO_MARKET_ORG_IDS:
        return OrganizationProvenance.DEMO
    if organization_id in INTEGRATION_TEST_ORG_IDS:
        return OrganizationProvenance.TEST
    if organization_id in KNOWN_CANARY_ORG_IDS:
        return OrganizationProvenance.CANARY
    return OrganizationProvenance.UNKNOWN


def snapshot_organization_provenance(organization: Organization | None) -> OrganizationProvenance:
    """Return only the stored classification used by executable snapshots.

    ID registries are migration/seed inputs, never runtime authority. A known
    demo ID whose stored value is still UNKNOWN remains quarantined.
    """
    if organization is None:
        return OrganizationProvenance.UNKNOWN
    return coerce_provenance(getattr(organization, "provenance", None))


def execution_provenance_compatible(
    left: OrganizationProvenance,
    right: OrganizationProvenance,
    *,
    left_org_id: UUID | None = None,
    right_org_id: UUID | None = None,
    allowed_demo_org_pair: Set[UUID] | None = None,
) -> bool:
    """Permit REAL pairs, or one explicitly supplied deterministic DEMO pair."""
    normalized = (coerce_provenance(left), coerce_provenance(right))
    if normalized == (OrganizationProvenance.REAL, OrganizationProvenance.REAL):
        return True
    if normalized != (OrganizationProvenance.DEMO, OrganizationProvenance.DEMO):
        return False
    if (
        left_org_id is None
        or right_org_id is None
        or allowed_demo_org_pair is None
    ):
        return False
    return frozenset((left_org_id, right_org_id)) == frozenset(allowed_demo_org_pair)
