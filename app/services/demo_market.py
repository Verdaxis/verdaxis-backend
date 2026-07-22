from uuid import UUID

from app.demo_identities import (
    DEMO_ACTIVITY_BUYER_ORG_ID,
    DEMO_ACTIVITY_ORG_IDS,
    DEMO_ACTIVITY_SELLER_ORG_ID,
    DEMO_MARKET_ORG_IDS,
)


def is_demo_market_organization(organization_id: UUID | None) -> bool:
    return organization_id is not None and organization_id in DEMO_MARKET_ORG_IDS
