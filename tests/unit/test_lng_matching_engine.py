"""Bio-LNG incompatible requirements never reserve matched quantity."""
from decimal import Decimal

import pytest

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.models.orderbook import OrderSide
from app.services.matching_engine import match_order
from tests.unit.test_matching_engine import (
    _make_order, async_engine, buyer_org, db, org_buyer_id, org_seller_id,
    seller_org, setup_tables, test_dp, test_product,
)
from tests.unit.test_lng_order import ASK, BID


@pytest.mark.asyncio
@pytest.mark.parametrize("compatible", [True, False])
async def test_lng_matching_respects_requirements(db, buyer_org, seller_org, org_buyer_id,
                                                org_seller_id, test_product, test_dp, compatible):
    test_product.id = PRODUCT_IDS["BIO_LNG"]
    test_product.name, test_product.fuel_type, test_product.fuel_grade = "Bio-LNG", "LNG", "Bio"
    test_dp.id = DELIVERY_POINT_IDS["Rotterdam"]
    test_dp.name, test_dp.region = "Rotterdam", "Europe"
    await db.flush()
    ask = _make_order(org_seller_id, OrderSide.ASK, product_id=test_product.id,
                      delivery_point_id=test_dp.id, price=Decimal(900), quantity=Decimal(30),
                      certification_scheme="ISCC EU")
    ask.msds_available = True
    ask.lng_terms = dict(ASK)
    bid = _make_order(org_buyer_id, OrderSide.BID, product_id=test_product.id,
                      delivery_point_id=test_dp.id, price=Decimal(1000), quantity=Decimal(20))
    bid.lng_terms = {**BID, "accept_resellers": compatible}
    db.add_all([ask, bid])
    await db.flush()
    trades = await match_order(db, bid)
    if not compatible:
        assert trades == []
        assert ask.remaining_quantity_mt == 30 and bid.remaining_quantity_mt == 20
    else:
        assert len(trades) == 1
        assert trades[0].market_product == "BIO_LNG"
        assert trades[0].lng_terms_snapshot["ask"]["scheme_certificate_number"] == "SELLER-123"
        assert ask.remaining_quantity_mt == 10
