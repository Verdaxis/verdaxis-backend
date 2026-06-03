from uuid import UUID

from app.seeds.market_seed import BUYER_ORGS, DEMO_BUYER_ORG_ID, DEMO_SELLER_ORG_ID, SUPPLIER_ORGS

DEMO_ACTIVITY_BUYER_ORG_ID = UUID("7cc77115-0a9f-4ec4-8c74-05aa10050111")
DEMO_ACTIVITY_SELLER_ORG_ID = UUID("d1e43e55-3fb0-4b5e-9f0b-93aa10050222")

DEMO_MARKET_ORG_IDS = frozenset(
    [
        *(org["id"] for org in BUYER_ORGS),
        *(org["id"] for org in SUPPLIER_ORGS),
        DEMO_BUYER_ORG_ID,
        DEMO_SELLER_ORG_ID,
        DEMO_ACTIVITY_BUYER_ORG_ID,
        DEMO_ACTIVITY_SELLER_ORG_ID,
    ]
)

DEMO_ACTIVITY_ORG_IDS = frozenset([DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID])


def is_demo_market_organization(organization_id: UUID | None) -> bool:
    return organization_id is not None and organization_id in DEMO_MARKET_ORG_IDS
