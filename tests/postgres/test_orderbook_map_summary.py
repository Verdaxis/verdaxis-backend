"""Legacy and compact map-summary parity on disposable PostgreSQL."""

from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import UUID, uuid4

import pytest

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrganizationProvenance, OrgType
from app.routers.orderbook import get_compact_map_summary, get_map_summary
from tests.postgres.test_market_routes import _seed_route_market


@pytest.mark.asyncio
async def test_compact_map_summary_matches_legacy_postgres_queries(
    market_pg,
    monkeypatch,
):
    seeded = await _seed_route_market(market_pg)
    observed_at = datetime(2026, 10, 2, 12, tzinfo=UTC)
    demo_expiry = datetime.now(UTC) + timedelta(days=1)

    async with seeded["factory"]() as db:
        demo_supplier = Organization(
            name=f"Map parity demo {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
            provenance=OrganizationProvenance.DEMO,
            verification_status="APPROVED",
        )
        unknown_supplier = Organization(
            name=f"Map parity unknown {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
            provenance=OrganizationProvenance.UNKNOWN,
            verification_status="APPROVED",
        )
        db.add_all([demo_supplier, unknown_supplier])
        await db.flush()

        def order(
            *,
            organization_id,
            owner_user_id,
            provenance: OrganizationProvenance,
            side: OrderSide,
            price: str,
            quantity: str,
            window: str,
            created_at: datetime,
            order_id: UUID | None = None,
            expires_at: datetime | None = None,
            qualified: bool = True,
            off_spec: bool = False,
        ) -> OrderBookOrder:
            return OrderBookOrder(
                id=order_id or uuid4(),
                organization_id=organization_id,
                owner_user_id=owner_user_id,
                provenance=provenance,
                side=side,
                product_id=seeded["product_id"],
                delivery_point_id=seeded["point_id"],
                quantity_mt=Decimal(quantity),
                remaining_quantity_mt=Decimal(quantity),
                price_per_mt_usd=Decimal(price),
                availability_window=window,
                status=OrderBookStatus.OPEN,
                created_at=created_at,
                expires_at=expires_at,
                certification_declared=qualified,
                certification_scheme="ISCC EU",
                specification_standard="IMPCA",
                msds_available=True,
                carbon_intensity_gco2_mj=Decimal("20.00"),
                carbon_intensity_method="ISCC EU",
                feedstock="biogenic waste",
                origin="parity fixture",
                off_spec=off_spec,
            )

        eligible = [
            order(
                organization_id=seeded["buyer_org_id"],
                owner_user_id=seeded["buyer_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.BID,
                price="900",
                quantity="500",
                window="SPOT",
                created_at=observed_at - timedelta(minutes=6),
            ),
            order(
                organization_id=seeded["buyer_org_id"],
                owner_user_id=seeded["buyer_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.BID,
                price="925",
                quantity="100",
                window="SPOT",
                created_at=observed_at - timedelta(minutes=5),
            ),
            order(
                organization_id=seeded["seller_org_id"],
                owner_user_id=seeded["seller_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.ASK,
                price="1000",
                quantity="350",
                window="SPOT",
                created_at=observed_at - timedelta(minutes=4),
            ),
            order(
                organization_id=seeded["buyer_org_id"],
                owner_user_id=seeded["buyer_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.BID,
                price="880",
                quantity="150",
                window="2027-01",
                created_at=observed_at - timedelta(minutes=3),
            ),
            order(
                organization_id=seeded["seller_org_id"],
                owner_user_id=seeded["seller_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.ASK,
                price="1200",
                quantity="200",
                window="2027-CAL",
                created_at=observed_at,
                order_id=UUID("ffffffff-ffff-ffff-ffff-ffffffffffff"),
            ),
            order(
                organization_id=demo_supplier.id,
                owner_user_id=None,
                provenance=OrganizationProvenance.DEMO,
                side=OrderSide.BID,
                price="940",
                quantity="100",
                window="2027-01",
                created_at=observed_at - timedelta(minutes=3),
                expires_at=demo_expiry,
            ),
            order(
                organization_id=demo_supplier.id,
                owner_user_id=None,
                provenance=OrganizationProvenance.DEMO,
                side=OrderSide.ASK,
                price="1040",
                quantity="90",
                window="2027-01",
                created_at=observed_at - timedelta(minutes=2),
                expires_at=demo_expiry,
            ),
            order(
                organization_id=demo_supplier.id,
                owner_user_id=None,
                provenance=OrganizationProvenance.DEMO,
                side=OrderSide.ASK,
                price="1050",
                quantity="10",
                window="2027-01",
                created_at=observed_at - timedelta(minutes=1),
                expires_at=demo_expiry,
            ),
            order(
                organization_id=demo_supplier.id,
                owner_user_id=None,
                provenance=OrganizationProvenance.DEMO,
                side=OrderSide.ASK,
                price="1060",
                quantity="80",
                window="2027-Q1",
                created_at=observed_at,
                order_id=UUID("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee"),
                expires_at=demo_expiry,
            ),
        ]
        excluded = [
            order(
                organization_id=unknown_supplier.id,
                owner_user_id=None,
                provenance=OrganizationProvenance.UNKNOWN,
                side=OrderSide.ASK,
                price="1",
                quantity="999",
                window="SPOT",
                created_at=observed_at + timedelta(minutes=1),
            ),
            order(
                organization_id=seeded["seller_org_id"],
                owner_user_id=seeded["seller_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.ASK,
                price="2",
                quantity="999",
                window="SPOT",
                created_at=observed_at + timedelta(minutes=1),
                qualified=False,
            ),
            order(
                organization_id=seeded["seller_org_id"],
                owner_user_id=seeded["seller_id"],
                provenance=OrganizationProvenance.REAL,
                side=OrderSide.ASK,
                price="3",
                quantity="999",
                window="SPOT",
                created_at=observed_at + timedelta(minutes=1),
                off_spec=True,
            ),
            order(
                organization_id=demo_supplier.id,
                owner_user_id=None,
                provenance=OrganizationProvenance.DEMO,
                side=OrderSide.ASK,
                price="4",
                quantity="999",
                window="SPOT",
                created_at=observed_at + timedelta(minutes=1),
                expires_at=datetime.now(UTC) - timedelta(days=1),
            ),
        ]
        db.add_all(eligible + excluded)
        await db.commit()

        executed = []
        execute = db.execute

        async def record_execute(statement, *args, **kwargs):
            executed.append(statement)
            return await execute(statement, *args, **kwargs)

        monkeypatch.setattr(db, "execute", record_execute)
        legacy = await get_map_summary(db=db)
        assert len(executed) == 2
        executed.clear()
        compact = await get_compact_map_summary(db=db)
        assert len(executed) == 2

    legacy_groups = {
        (group.availability_window, group.side.value, group.evidence_class): group
        for group in legacy.groups
    }
    assert set(legacy_groups) == {
        ("SPOT", "BID", "REAL"),
        ("SPOT", "ASK", "REAL"),
        ("2027-01", "BID", "REAL"),
        ("2027-CAL", "ASK", "REAL"),
        ("2027-01", "BID", "DEMO"),
        ("2027-01", "ASK", "DEMO"),
        ("2027-Q1", "ASK", "DEMO"),
    }
    real_spot_bid = legacy_groups[("SPOT", "BID", "REAL")]
    assert (
        real_spot_bid.min_price,
        real_spot_bid.max_price,
        real_spot_bid.total_quantity,
        real_spot_bid.order_count,
    ) == (Decimal("900"), Decimal("925"), Decimal("600"), 2)

    assert len(compact.markets) == 2
    real = next(row for row in compact.markets if row.evidence_class == "REAL")
    assert (
        real.bid_min_price,
        real.bid_max_price,
        real.bid_total_quantity,
        real.bid_order_count,
        real.ask_min_price,
        real.ask_max_price,
        real.ask_total_quantity,
        real.ask_order_count,
        real.spot_best_bid,
        real.spot_best_ask,
    ) == (
        Decimal("880"),
        Decimal("925"),
        Decimal("750"),
        3,
        Decimal("1000"),
        Decimal("1200"),
        Decimal("550"),
        2,
        Decimal("925"),
        Decimal("1000"),
    )
    demo = next(row for row in compact.markets if row.evidence_class == "DEMO")
    assert (
        demo.bid_total_quantity,
        demo.bid_order_count,
        demo.ask_min_price,
        demo.ask_max_price,
        demo.ask_total_quantity,
        demo.ask_order_count,
        demo.spot_best_bid,
        demo.spot_best_ask,
    ) == (
        Decimal("100"),
        1,
        Decimal("1040"),
        Decimal("1060"),
        Decimal("180"),
        3,
        None,
        None,
    )

    expected_demo_groups = [
        legacy_groups[("2027-01", "ASK", "DEMO")],
        legacy_groups[("2027-01", "BID", "DEMO")],
    ]
    assert [row.model_dump() for row in compact.demo_groups] == [
        row.model_dump() for row in expected_demo_groups
    ]
    assert compact.recent_asks == legacy.recent_asks
    assert len(compact.recent_asks) == 1
    assert compact.recent_asks[0].price_per_mt_usd == Decimal("1200")
    assert compact.recent_asks[0].created_at == observed_at
    assert compact.recent_asks[0].evidence_class == "REAL"
