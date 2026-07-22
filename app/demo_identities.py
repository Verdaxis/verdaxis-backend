"""Exact deterministic identities for synthetic market organizations.

This module is application policy.  Alembic revisions repeat their own
immutable literals and must never import it.
"""

from uuid import UUID


DEMO_SEED_BUYERS: tuple[tuple[UUID, str], ...] = (
    (UUID("4da7b285-34ee-5443-9406-f96b4ed1a251"), "Verdaxis Demo Buyer 01"),
    (UUID("0dbce576-2026-5925-ab66-674d505e98ad"), "Verdaxis Demo Buyer 02"),
    (UUID("3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a"), "Verdaxis Demo Buyer 03"),
    (UUID("277491df-cb0d-5f2d-a2cf-5746829c6da6"), "Verdaxis Demo Buyer 04"),
    (UUID("3b302066-d65c-5c3e-8fcc-70b3da3bcafd"), "Verdaxis Demo Buyer 05"),
)

DEMO_SEED_SUPPLIERS: tuple[tuple[UUID, str, str], ...] = (
    (UUID("79609f48-0a3e-560e-a1e1-63d90601d84a"), "Verdaxis Demo Supplier 01", "MAJOR_TRADER"),
    (UUID("2c4e387e-de22-5adb-ad88-9274ba84ebe1"), "Verdaxis Demo Supplier 02", "MAJOR_TRADER"),
    (UUID("612953c7-567a-58b3-bc42-ee817d2bbe74"), "Verdaxis Demo Supplier 03", "TIER_1_PRODUCER"),
    (UUID("93ccda09-54b3-53ee-afc0-759d3048161f"), "Verdaxis Demo Supplier 04", "REGIONAL_SUPPLIER"),
    (UUID("82426590-0963-5486-9b05-f81e97afe6ef"), "Verdaxis Demo Supplier 05", "MAJOR_TRADER"),
)

DEMO_ACCOUNT_BUYER_ORG_ID = UUID("acc3f20a-fe94-4463-9029-a55e35634eb7")
DEMO_ACCOUNT_SELLER_ORG_ID = UUID("c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4")
DEMO_ACTIVITY_BUYER_ORG_ID = UUID("7cc77115-0a9f-4ec4-8c74-05aa10050111")
DEMO_ACTIVITY_SELLER_ORG_ID = UUID("d1e43e55-3fb0-4b5e-9f0b-93aa10050222")

CANONICAL_DEMO_ORG_NAMES: dict[UUID, str] = {
    **{organization_id: name for organization_id, name in DEMO_SEED_BUYERS},
    **{
        organization_id: name
        for organization_id, name, _tier in DEMO_SEED_SUPPLIERS
    },
    DEMO_ACCOUNT_BUYER_ORG_ID: "Verdaxis Demo Buyer 06",
    DEMO_ACCOUNT_SELLER_ORG_ID: "Verdaxis Demo Supplier 06",
    DEMO_ACTIVITY_BUYER_ORG_ID: "Verdaxis Demo Buyer Activity",
    DEMO_ACTIVITY_SELLER_ORG_ID: "Verdaxis Demo Supplier Activity",
}

DEMO_MARKET_ORG_IDS = frozenset(CANONICAL_DEMO_ORG_NAMES)
DEMO_ACTIVITY_ORG_IDS = frozenset(
    (DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID)
)

KNOWN_TEST_ORG_IDS = frozenset(
    UUID(f"9e63f7a1-0000-4000-8000-{number:012d}")
    for number in (*range(1, 5), *range(11, 15))
)
