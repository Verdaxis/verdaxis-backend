"""
Unit tests for orderbook Pydantic schemas.

Tests validation rules, default values, and serialization without
requiring a running database.
"""
import pytest
from decimal import Decimal
from uuid import uuid4
from datetime import datetime

from app.schemas.orderbook import (
    OrderCreate,
    OrderUpdate,
    OrderResponse,
    OrderMyResponse,
    TradeCreate,
    TradeResponse,
    TradeDeliverPayload,
    AggregatedOrderbookResponse,
    OrderSide,
    OrderBookStatus,
    TradeStatus,
    Initiator,
    FuelGrade,
    TierLabel,
)


class TestOrderCreate:
    def test_valid_bid_order(self):
        product_id = uuid4()
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=product_id,
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            certification_scheme="ISCC EU",
        )
        assert order.side == OrderSide.BID
        assert order.product_id == product_id
        assert order.delivery_point_id is not None
        assert order.availability_window == "SPOT"  # default
        assert order.certifications == []  # default
        assert order.certification_declared is False
        assert order.msds_available is False
        assert order.off_spec is False
        assert order.port_id is None
        assert order.vessel_id is None

    def test_valid_ask_order_with_all_fields(self):
        product_id = uuid4()
        dp_id = uuid4()
        order = OrderCreate(
            side=OrderSide.ASK,
            product_id=product_id,
            delivery_point_id=dp_id,
            port_id="NLRTM",
            quantity_mt=Decimal("5000"),
            price_per_mt_usd=Decimal("780"),
            availability_window="2026-Q1",
            certifications=["ISCC", "Nanolumi"],
            certification_declared=True,
            certification_scheme="ISCC EU",
            specification_standard="IMPCA",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("18.40"),
            carbon_intensity_method="ISCC EU",
            feedstock="Municipal solid waste",
            origin="Netherlands",
            off_spec=True,
            off_spec_notes="Water content slightly above target",
            expires_at=datetime(2026, 6, 1),
        )
        assert order.side == OrderSide.ASK
        assert order.product_id == product_id
        assert order.delivery_point_id == dp_id
        assert order.certifications == ["ISCC", "Nanolumi"]
        assert order.certification_declared is True
        assert order.certification_scheme == "ISCC EU"
        assert order.specification_standard == "IMPCA"
        assert order.msds_available is True
        assert order.carbon_intensity_gco2_mj == Decimal("18.40")
        assert order.carbon_intensity_method == "ISCC EU"
        assert order.feedstock == "Municipal solid waste"
        assert order.origin == "Netherlands"
        assert order.off_spec is True
        assert order.off_spec_notes == "Water content slightly above target"
        assert order.availability_window == "2026-Q1"

    def test_quantity_must_be_positive(self):
        with pytest.raises(Exception):
            OrderCreate(
                side=OrderSide.BID,
                product_id=uuid4(),
                delivery_point_id=uuid4(),
                quantity_mt=Decimal("0"),
                price_per_mt_usd=Decimal("100"),
                certification_scheme="ISCC EU",
            )

    def test_negative_quantity_rejected(self):
        with pytest.raises(Exception):
            OrderCreate(
                side=OrderSide.ASK,
                product_id=uuid4(),
                delivery_point_id=uuid4(),
                quantity_mt=Decimal("-500"),
                price_per_mt_usd=Decimal("100"),
                certification_scheme="ISCC EU",
            )

    def test_price_must_be_positive(self):
        with pytest.raises(Exception):
            OrderCreate(
                side=OrderSide.BID,
                product_id=uuid4(),
                delivery_point_id=uuid4(),
                quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("0"),
                certification_scheme="ISCC EU",
            )

    def test_product_id_required(self):
        """product_id is mandatory."""
        with pytest.raises(Exception):
            OrderCreate(
                side=OrderSide.BID,
                delivery_point_id=uuid4(),
                quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("100"),
                certification_scheme="ISCC EU",
            )

    def test_vessel_id_accepts_uuid(self):
        vid = uuid4()
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            vessel_id=vid,
            quantity_mt=Decimal("500"),
            price_per_mt_usd=Decimal("600"),
            certification_scheme="ISCC EU",
        )
        assert order.vessel_id == vid

    def test_delivery_point_id_required(self):
        with pytest.raises(Exception):
            OrderCreate(
                side=OrderSide.ASK,
                product_id=uuid4(),
                quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("500"),
                certification_scheme="ISCC EU",
            )

    def test_bid_can_omit_certification_scheme(self):
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("500"),
        )
        assert order.certification_scheme is None

    def test_ask_can_omit_declaration_in_schema_but_is_checked_in_router(self):
        order = OrderCreate(
            side=OrderSide.ASK,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("500"),
            certification_scheme="ISCC EU",
        )
        assert order.certification_declared is False

    def test_delivery_point_id_accepts_uuid(self):
        dp_id = uuid4()
        order = OrderCreate(
            side=OrderSide.ASK,
            product_id=uuid4(),
            delivery_point_id=dp_id,
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("500"),
            certification_declared=True,
            certification_scheme="ISCC EU",
        )
        assert order.delivery_point_id == dp_id


class TestOrderUpdate:
    def test_all_fields_optional(self):
        update = OrderUpdate()
        dumped = update.model_dump(exclude_unset=True)
        assert dumped == {}

    def test_partial_update(self):
        update = OrderUpdate(
            quantity_mt=Decimal("2000"),
            price_per_mt_usd=Decimal("600"),
            certification_scheme="ISCC EU",
        )
        dumped = update.model_dump(exclude_unset=True)
        assert "quantity_mt" in dumped
        assert "price_per_mt_usd" in dumped
        assert "availability_window" not in dumped

    def test_certifications_update(self):
        update = OrderUpdate(certifications=["ISCC"])
        dumped = update.model_dump(exclude_unset=True)
        assert dumped["certifications"] == ["ISCC"]

    def test_metadata_update(self):
        update = OrderUpdate(
            certification_declared=True,
            certification_scheme="ISCC PLUS",
            specification_standard="ASTM D5798",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("12.30"),
            carbon_intensity_method="Supplier attestation",
            feedstock="Residue ethanol",
            origin="Brazil",
            off_spec=True,
            off_spec_notes="Chloride above nominal target",
        )
        dumped = update.model_dump(exclude_unset=True)
        assert dumped["certification_declared"] is True
        assert dumped["certification_scheme"] == "ISCC PLUS"
        assert dumped["specification_standard"] == "ASTM D5798"
        assert dumped["msds_available"] is True
        assert dumped["carbon_intensity_gco2_mj"] == Decimal("12.30")
        assert dumped["carbon_intensity_method"] == "Supplier attestation"
        assert dumped["feedstock"] == "Residue ethanol"
        assert dumped["origin"] == "Brazil"
        assert dumped["off_spec"] is True
        assert dumped["off_spec_notes"] == "Chloride above nominal target"


class TestOrderResponse:
    def test_from_dict(self):
        product_id = uuid4()
        resp = OrderResponse(
            id=uuid4(),
            side=OrderSide.ASK,
            product_id=product_id,
            product_name="Biofuel Bio",
            market_product="BIO_METHANOL",
            fuel_type="Biofuel",
            fuel_grade="Bio",
            region="Singapore",
            quantity_mt=Decimal("5000"),
            remaining_quantity_mt=Decimal("3000"),
            price_per_mt_usd=Decimal("780"),
            availability_window="SPOT",
            certifications=["ISCC"],
            certification_declared=True,
            certification_scheme="ISCC EU",
            specification_standard="IMPCA",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("22.10"),
            carbon_intensity_method="ISCC EU",
            feedstock="Anaerobic digestion CO2 + green hydrogen",
            origin="Iceland",
            off_spec=True,
            off_spec_notes="Cloud point under review",
            is_verdaxis_verified=True,
            status=OrderBookStatus.PARTIALLY_FILLED,
            created_at=datetime.utcnow(),
        )
        assert resp.remaining_quantity_mt == Decimal("3000")
        assert resp.tier_label == TierLabel.INDEPENDENT  # default
        assert resp.product_id == product_id
        assert resp.product_name == "Biofuel Bio"
        assert resp.market_product == "BIO_METHANOL"
        assert resp.certification_declared is True
        assert resp.certification_scheme == "ISCC EU"
        assert resp.specification_standard == "IMPCA"
        assert resp.msds_available is True
        assert resp.carbon_intensity_gco2_mj == Decimal("22.10")
        assert resp.carbon_intensity_method == "ISCC EU"
        assert resp.feedstock == "Anaerobic digestion CO2 + green hydrogen"
        assert resp.origin == "Iceland"
        assert resp.off_spec is True
        assert resp.off_spec_notes == "Cloud point under review"

    def test_my_response_extends_base(self):
        now = datetime.utcnow()
        resp = OrderMyResponse(
            id=uuid4(),
            side=OrderSide.BID,
            product_id=uuid4(),
            product_name="LNG Conventional",
            market_product="BIO_ETHANOL",
            fuel_type="LNG",
            fuel_grade="Conventional",
            region="Houston",
            quantity_mt=Decimal("1000"),
            remaining_quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("1200"),
            availability_window="SPOT",
            is_verdaxis_verified=False,
            status=OrderBookStatus.OPEN,
            created_at=now,
            organization_id=uuid4(),
            updated_at=now,
            trade_count=3,
        )
        assert resp.trade_count == 3
        assert resp.organization_id is not None

    def test_market_product_defaults_to_none(self):
        resp = OrderResponse(
            id=uuid4(),
            side=OrderSide.ASK,
            product_id=uuid4(),
            product_name="Legacy Product",
            fuel_type="Legacy",
            fuel_grade="Conventional",
            region="Singapore",
            quantity_mt=Decimal("5000"),
            remaining_quantity_mt=Decimal("3000"),
            price_per_mt_usd=Decimal("780"),
            availability_window="SPOT",
            is_verdaxis_verified=True,
            status=OrderBookStatus.OPEN,
            created_at=datetime.utcnow(),
        )

        assert resp.market_product is None
        assert resp.certification_declared is False
        assert resp.certification_scheme is None
        assert resp.specification_standard is None
        assert resp.msds_available is False
        assert resp.carbon_intensity_gco2_mj is None
        assert resp.carbon_intensity_method is None
        assert resp.feedstock is None
        assert resp.origin is None
        assert resp.off_spec is False
        assert resp.off_spec_notes is None


class TestTradeCreate:
    def test_valid_trade(self):
        tc = TradeCreate(
            order_id=uuid4(),
            quantity_mt=Decimal("500"),
        )
        assert tc.quantity_mt == Decimal("500")

    def test_quantity_must_be_positive(self):
        with pytest.raises(Exception):
            TradeCreate(order_id=uuid4(), quantity_mt=Decimal("0"))

    def test_negative_quantity_rejected(self):
        with pytest.raises(Exception):
            TradeCreate(order_id=uuid4(), quantity_mt=Decimal("-100"))


class TestTradeResponse:
    def test_full_response(self):
        resp = TradeResponse(
            id=uuid4(),
            ask_order_id=uuid4(),
            buyer_id=uuid4(),
            seller_id=uuid4(),
            buyer_name="Pacific Ocean Lines",
            seller_name="Global Energy Supply",
            initiated_by=Initiator.BUYER,
            quantity_mt=Decimal("500"),
            price_per_mt_usd=Decimal("780"),
            status=TradeStatus.CONFIRMED,
            confirmed_at=datetime.utcnow(),
            created_at=datetime.utcnow(),
            product_id=uuid4(),
            product_name="Biofuel Bio",
            fuel_type="Biofuel",
            fuel_grade="Bio",
            region="Singapore",
        )
        assert resp.buyer_name == "Pacific Ocean Lines"
        assert resp.status == TradeStatus.CONFIRMED
        assert resp.bid_order_id is None  # not set
        assert resp.product_name == "Biofuel Bio"

    def test_defaults(self):
        resp = TradeResponse(
            id=uuid4(),
            buyer_id=uuid4(),
            seller_id=uuid4(),
            initiated_by=Initiator.SELLER,
            quantity_mt=Decimal("100"),
            price_per_mt_usd=Decimal("500"),
            status=TradeStatus.PENDING_CONFIRMATION,
            created_at=datetime.utcnow(),
        )
        assert resp.buyer_name == ""
        assert resp.seller_name == ""
        assert resp.fuel_type == ""
        assert resp.region == ""
        assert resp.product_name == ""
        assert resp.commission_rate_pct == Decimal("0.5")
        assert resp.final_quantity_mt is None
        assert resp.commission_amount_usd is None


class TestTradeDeliverPayload:
    def test_valid_payload(self):
        p = TradeDeliverPayload(
            final_quantity_mt=Decimal("495"),
            final_price_per_mt=Decimal("785"),
        )
        assert p.final_quantity_mt == Decimal("495")

    def test_quantity_must_be_positive(self):
        with pytest.raises(Exception):
            TradeDeliverPayload(
                final_quantity_mt=Decimal("0"),
                final_price_per_mt=Decimal("100"),
            )

    def test_price_must_be_positive(self):
        with pytest.raises(Exception):
            TradeDeliverPayload(
                final_quantity_mt=Decimal("100"),
                final_price_per_mt=Decimal("-5"),
            )


class TestAggregatedOrderbookResponse:
    def test_aggregated_data(self):
        agg = AggregatedOrderbookResponse(
            product_id=uuid4(),
            product_name="Biofuel Bio",
            fuel_type="Biofuel",
            region="Singapore",
            side=OrderSide.ASK,
            min_price=Decimal("750"),
            max_price=Decimal("800"),
            total_quantity=Decimal("15000"),
            order_count=5,
        )
        assert agg.order_count == 5
        assert agg.min_price < agg.max_price
        assert agg.product_name == "Biofuel Bio"


class TestEnumValues:
    def test_order_side_values(self):
        assert OrderSide.BID.value == "BID"
        assert OrderSide.ASK.value == "ASK"

    def test_order_book_status_values(self):
        assert OrderBookStatus.OPEN.value == "OPEN"
        assert OrderBookStatus.PARTIALLY_FILLED.value == "PARTIALLY_FILLED"
        assert OrderBookStatus.FILLED.value == "FILLED"
        assert OrderBookStatus.CANCELLED.value == "CANCELLED"
        assert OrderBookStatus.EXPIRED.value == "EXPIRED"

    def test_trade_status_values(self):
        assert TradeStatus.PENDING_CONFIRMATION.value == "PENDING_CONFIRMATION"
        assert TradeStatus.CONFIRMED.value == "CONFIRMED"
        assert TradeStatus.DELIVERED.value == "DELIVERED"
        assert TradeStatus.PAID.value == "PAID"
        assert TradeStatus.CANCELLED.value == "CANCELLED"
        assert TradeStatus.DECLINED.value == "DECLINED"

    def test_initiator_values(self):
        assert Initiator.BUYER.value == "BUYER"
        assert Initiator.SELLER.value == "SELLER"

    def test_fuel_grade_values(self):
        assert FuelGrade.CONVENTIONAL.value == "Conventional"
        assert FuelGrade.GREEN.value == "Green"
        assert FuelGrade.BIO.value == "Bio"
        assert FuelGrade.E.value == "E"
        assert FuelGrade.SYNTHETIC.value == "Synthetic"
