from uuid import uuid4

from app.seeds.market_seed import BUYER_ORGS, DEMO_BUYER_ORG_ID, DEMO_SELLER_ORG_ID, SUPPLIER_ORGS
from app.services.demo_market import is_demo_market_organization


def test_seeded_buyer_orgs_are_marked_demo_market():
    assert is_demo_market_organization(BUYER_ORGS[0]["id"]) is True


def test_seeded_supplier_orgs_are_marked_demo_market():
    assert is_demo_market_organization(SUPPLIER_ORGS[0]["id"]) is True


def test_demo_accounts_are_marked_demo_market():
    assert is_demo_market_organization(DEMO_BUYER_ORG_ID) is True
    assert is_demo_market_organization(DEMO_SELLER_ORG_ID) is True


def test_unknown_org_is_not_marked_demo_market():
    assert is_demo_market_organization(uuid4()) is False
