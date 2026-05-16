from uuid import UUID

from app.seeds.market_seed import BUYER_ORGS, DEMO_BUYER_ORG_ID, DEMO_SELLER_ORG_ID, SUPPLIER_ORGS


DEMO_MARKET_ORG_IDS = frozenset(
    [
        *(org["id"] for org in BUYER_ORGS),
        *(org["id"] for org in SUPPLIER_ORGS),
        DEMO_BUYER_ORG_ID,
        DEMO_SELLER_ORG_ID,
    ]
)


def is_demo_market_organization(organization_id: UUID | None) -> bool:
    return organization_id is not None and organization_id in DEMO_MARKET_ORG_IDS
