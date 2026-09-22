"""B100 uses the same price-time matching and quantity ledger as other fuels."""
# Explicit fixture re-exports make pytest registration visible to static tools.
# ruff: noqa: PLC0414

from datetime import UTC, datetime, timedelta
from decimal import Decimal

import pytest

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.models.orderbook import OrderBookStatus, OrderSide
from app.services.matching_engine import match_order
from tests.unit.test_matching_engine import (
    _make_order,
)
from tests.unit.test_matching_engine import (
    async_engine as async_engine,
)
from tests.unit.test_matching_engine import (
    buyer_org as buyer_org,
)
from tests.unit.test_matching_engine import (
    db as db,
)
from tests.unit.test_matching_engine import (
    org_buyer_id as org_buyer_id,
)
from tests.unit.test_matching_engine import (
    org_seller_id as org_seller_id,
)
from tests.unit.test_matching_engine import (
    seller_org as seller_org,
)
from tests.unit.test_matching_engine import (
    setup_tables as setup_tables,
)
from tests.unit.test_matching_engine import (
    test_dp as test_dp,
)
from tests.unit.test_matching_engine import (
    test_product as test_product,
)


@pytest.mark.asyncio
@pytest.mark.parametrize("compatible", [True, False])
async def test_b100_matching_preserves_quantity_and_snapshots(
    db,
    buyer_org,
    seller_org,
    org_buyer_id,
    org_seller_id,
    test_product,
    test_dp,
    compatible,
):
    test_product.id = PRODUCT_IDS["UCOME_B100"]
    test_product.name = "UCOME B100"
    test_product.fuel_type = "FAME"
    test_product.fuel_grade = "UCOME"
    test_dp.id = DELIVERY_POINT_IDS["Singapore"]
    await db.flush()
    ask = _make_order(
        org_seller_id,
        OrderSide.ASK,
        product_id=test_product.id,
        delivery_point_id=test_dp.id,
        price=Decimal(900),
        quantity=Decimal(30),
        certification_scheme="ISCC_EU",
    )
    ask.msds_available = True
    ask.fame_terms = {
        "side": "ASK",
        "neat_fame": True,
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "certificate",
        "certificate_holder": "supplier",
        "evidence_status": "PENDING",
        "certificate_valid_until": str(datetime.now(UTC).date() + timedelta(days=365)),
        "evidence_due": "BEFORE_LOADING",
        "cfpp_c": -5,
    }
    bid = _make_order(
        org_buyer_id,
        OrderSide.BID,
        product_id=test_product.id,
        delivery_point_id=test_dp.id,
        price=Decimal(1000),
        quantity=Decimal(20),
        certification_scheme="ISCC_EU",
    )
    bid.fame_terms = {
        "side": "BID",
        "neat_fame": True,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
        "max_cfpp_c": -5 if compatible else -10,
    }
    db.add_all([ask, bid])
    await db.flush()
    trades = await match_order(db, bid)
    if not compatible:
        assert trades == []
        assert ask.remaining_quantity_mt == 30
        assert bid.remaining_quantity_mt == 20
        return
    assert len(trades) == 1
    trade = trades[0]
    assert trade.quantity_mt == 20
    assert trade.price_per_mt_usd == 900
    assert trade.market_product == "UCOME_B100"
    assert trade.fame_terms_snapshot["ask"]["cfpp_c"] == "-5"
    assert trade.fame_terms_snapshot["bid"]["max_cfpp_c"] == "-5"
    assert ask.remaining_quantity_mt == 10
    assert ask.status == OrderBookStatus.PARTIALLY_FILLED
    assert bid.status == OrderBookStatus.FILLED
