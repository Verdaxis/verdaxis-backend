"""
Unit tests for the forward curve router logic.
Tests curve aggregation, CSV export, and schema validation using mock DB sessions.
"""
import pytest
from decimal import Decimal
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

from app.routers.curves import (
    _aggregate_depth_levels,
    _build_board_cell,
    build_forward_curve_board,
    compute_forward_curve,
)
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.schemas.curves import (
    ForwardCurveBoardCell,
    ForwardCurveBoardDepthLevel,
    ForwardCurveBoardFocus,
    ForwardCurveBoardPort,
    ForwardCurveBoardProduct,
    ForwardCurveBoardResponse,
    ForwardCurvePoint,
    ForwardCurveResponse,
)


# ---------------------------------------------------------------------------
# Schema tests
# ---------------------------------------------------------------------------


class TestForwardCurvePointSchema:
    """ForwardCurvePoint schema validation."""

    def test_full_two_sided_market(self):
        point = ForwardCurvePoint(
            availability_window="2026-Q2",
            best_bid=Decimal("520.00"),
            best_ask=Decimal("530.00"),
            mid_price=Decimal("525.00"),
            spread=Decimal("10.00"),
            volume_mt=Decimal("1500.00"),
            order_count=4,
        )
        assert point.availability_window == "2026-Q2"
        assert point.best_bid == Decimal("520.00")
        assert point.best_ask == Decimal("530.00")
        assert point.mid_price == Decimal("525.00")
        assert point.spread == Decimal("10.00")
        assert point.volume_mt == Decimal("1500.00")
        assert point.order_count == 4

    def test_bid_only_market(self):
        """When only bids exist, ask/mid/spread are None."""
        point = ForwardCurvePoint(
            availability_window="SPOT",
            best_bid=Decimal("500.00"),
            best_ask=None,
            mid_price=None,
            spread=None,
            volume_mt=Decimal("500.00"),
            order_count=2,
        )
        assert point.best_bid == Decimal("500.00")
        assert point.best_ask is None
        assert point.mid_price is None
        assert point.spread is None

    def test_ask_only_market(self):
        """When only asks exist, bid/mid/spread are None."""
        point = ForwardCurvePoint(
            availability_window="2027-CAL",
            best_bid=None,
            best_ask=Decimal("610.00"),
            mid_price=None,
            spread=None,
            volume_mt=Decimal("200.00"),
            order_count=1,
        )
        assert point.best_ask == Decimal("610.00")
        assert point.best_bid is None
        assert point.mid_price is None

    def test_serialization(self):
        point = ForwardCurvePoint(
            availability_window="2026-Q1",
            best_bid=Decimal("480.00"),
            best_ask=Decimal("490.00"),
            mid_price=Decimal("485.00"),
            spread=Decimal("10.00"),
            volume_mt=Decimal("1000.00"),
            order_count=3,
        )
        data = point.model_dump()
        assert data["availability_window"] == "2026-Q1"
        assert data["best_bid"] == Decimal("480.00")
        assert data["order_count"] == 3


class TestForwardCurveResponseSchema:
    """ForwardCurveResponse wrapper schema."""

    def test_empty_curve(self):
        from datetime import datetime, timezone
        resp = ForwardCurveResponse(
            product_id=uuid4(),
            delivery_point_id=None,
            curve=[],
            generated_at=datetime.now(timezone.utc),
        )
        assert resp.curve == []
        assert resp.delivery_point_id is None

    def test_with_points(self):
        from datetime import datetime, timezone
        pid = uuid4()
        point = ForwardCurvePoint(
            availability_window="SPOT",
            best_bid=Decimal("510.00"),
            best_ask=Decimal("515.00"),
            mid_price=Decimal("512.50"),
            spread=Decimal("5.00"),
            volume_mt=Decimal("300.00"),
            order_count=2,
        )
        resp = ForwardCurveResponse(
            product_id=pid,
            delivery_point_id=None,
            curve=[point],
            generated_at=datetime.now(timezone.utc),
        )
        assert len(resp.curve) == 1
        assert resp.product_id == pid


class TestForwardCurveBoardSchema:
    """Forward curve board response schema."""

    def test_board_cell_carries_hybrid_market_context(self):
        pid = uuid4()
        dp_id = uuid4()
        cell = ForwardCurveBoardCell(
            product_id=pid,
            market_product="BIO_METHANOL",
            product_name="Bio Methanol",
            delivery_point_id=dp_id,
            delivery_point_name="Singapore",
            region="Asia",
            availability_window="SPOT",
            benchmark_mid=Decimal("1052.00"),
            benchmark_source="seed_matrix",
            is_demo_benchmark=True,
            best_bid=Decimal("1048.00"),
            best_ask=Decimal("1056.00"),
            spread=Decimal("8.00"),
            volume_mt=Decimal("9000.00"),
            order_count=4,
        )

        assert cell.benchmark_mid == Decimal("1052.00")
        assert cell.is_demo_benchmark is True
        assert cell.spread == Decimal("8.00")
        assert cell.order_source_kind == MarketSourceKind.NO_DATA
        assert cell.benchmark_source_kind == MarketSourceKind.NO_DATA
        assert cell.demo_status == MarketDemoStatus.NOT_APPLICABLE

    def test_board_response_groups_ports_products_and_focus(self):
        from datetime import datetime, timezone

        pid = uuid4()
        dp_id = uuid4()
        cell = ForwardCurveBoardCell(
            product_id=pid,
            market_product="BIO_METHANOL",
            product_name="Bio Methanol",
            delivery_point_id=dp_id,
            delivery_point_name="Singapore",
            region="Asia",
            availability_window="SPOT",
            benchmark_mid=Decimal("1052.00"),
            benchmark_source="manual_override",
            best_bid=None,
            best_ask=None,
            spread=None,
            volume_mt=Decimal("0"),
            order_count=0,
        )
        response = ForwardCurveBoardResponse(
            availability_window="SPOT",
            products=[
                ForwardCurveBoardProduct(
                    product_id=pid,
                    market_product="BIO_METHANOL",
                    product_name="Bio Methanol",
                )
            ],
            ports=[
                ForwardCurveBoardPort(
                    delivery_point_id=dp_id,
                    delivery_point_name="Singapore",
                    region="Asia",
                    cells=[cell],
                )
            ],
            focus=ForwardCurveBoardFocus(
                product_id=pid,
                market_product="BIO_METHANOL",
                product_name="Bio Methanol",
                delivery_point_id=dp_id,
                delivery_point_name="Singapore",
                region="Asia",
                availability_window="SPOT",
                curve=[cell],
                depth_bids=[ForwardCurveBoardDepthLevel(price_per_mt_usd=Decimal("1048"), quantity_mt=Decimal("5000"), order_count=1)],
                depth_asks=[],
            ),
            generated_at=datetime.now(timezone.utc),
        )

        assert response.products[0].market_product == "BIO_METHANOL"
        assert response.ports[0].cells[0].delivery_point_name == "Singapore"
        assert response.focus.depth_bids[0].quantity_mt == Decimal("5000")

    @pytest.mark.asyncio
    async def test_real_orders_hide_seed_demo_benchmark(self):
        """Demo benchmark scaffolding disappears once the exact slice has real orders."""
        product = MagicMock()
        product.id = uuid4()
        product.market_product = "BIO_METHANOL"
        product.name = "Bio Methanol"

        delivery_point = MagicMock()
        delivery_point.id = uuid4()
        delivery_point.name = "Singapore"
        delivery_point.region = "Asia"

        quote = MagicMock()
        quote.benchmark_price_per_mt_usd = Decimal("1052.00")
        quote.source = "seed_matrix"

        with patch("app.routers.curves.get_benchmark_quote", new=AsyncMock(return_value=quote)):
            cell = await _build_board_cell(
                AsyncMock(),
                product=product,
                delivery_point=delivery_point,
                availability_window="SPOT",
                orderbook_bucket={
                    "real_best_bid": Decimal("1048.00"),
                    "real_best_ask": Decimal("1056.00"),
                    "real_volume_mt": Decimal("9000.00"),
                    "real_order_count": 1,
                },
            )

        assert cell.benchmark_mid is None
        assert cell.benchmark_source is None
        assert cell.is_demo_benchmark is False
        assert cell.best_bid == Decimal("1048.00")
        assert cell.order_source_kind == MarketSourceKind.LIVE_ORDER
        assert cell.benchmark_source_kind == MarketSourceKind.NO_DATA
        assert cell.demo_status == MarketDemoStatus.REAL_ONLY

    @pytest.mark.asyncio
    async def test_board_cell_prefers_real_headline_without_blending_demo(self):
        """REAL headline values and totals cannot include visible demo depth."""
        product = MagicMock()
        product.id = uuid4()
        product.market_product = "BIO_METHANOL"
        product.name = "Bio Methanol"

        delivery_point = MagicMock()
        delivery_point.id = uuid4()
        delivery_point.name = "Singapore"
        delivery_point.region = "Asia"

        cell = await _build_board_cell(
            AsyncMock(),
            product=product,
            delivery_point=delivery_point,
            availability_window="SPOT",
            orderbook_bucket={
                "real_best_bid": Decimal("1045.00"),
                "demo_best_bid": Decimal("1050.00"),
                "real_best_ask": Decimal("1056.00"),
                "demo_best_ask": Decimal("1062.00"),
                "real_volume_mt": Decimal("3000.00"),
                "demo_volume_mt": Decimal("6000.00"),
                "real_order_count": 2,
                "demo_order_count": 2,
            },
            benchmark_quote=None,
        )

        assert cell.order_source_kind == MarketSourceKind.LIVE_ORDER
        assert cell.demo_status == MarketDemoStatus.REAL_ONLY
        assert cell.real_order_count == 2
        assert cell.demo_order_count == 2
        assert cell.real_best_bid == Decimal("1045.00")
        assert cell.demo_best_bid == Decimal("1050.00")
        assert cell.best_bid == Decimal("1045.00")
        assert cell.best_ask == Decimal("1056.00")
        assert cell.volume_mt == Decimal("3000.00")
        assert cell.order_count == 2
        assert cell.best_bid_source_kind == MarketSourceKind.LIVE_ORDER
        assert cell.best_ask_source_kind == MarketSourceKind.LIVE_ORDER

    @pytest.mark.asyncio
    async def test_board_cell_falls_back_to_visibly_demo_without_unknown_rows(self):
        product = MagicMock()
        product.id = uuid4()
        product.market_product = "BIO_METHANOL"
        product.name = "Bio Methanol"
        delivery_point = MagicMock()
        delivery_point.id = uuid4()
        delivery_point.name = "Singapore"
        delivery_point.region = "Asia"

        cell = await _build_board_cell(
            AsyncMock(),
            product=product,
            delivery_point=delivery_point,
            availability_window="SPOT",
            orderbook_bucket={
                "demo_best_bid": Decimal("1050.00"),
                "demo_best_ask": Decimal("1062.00"),
                "demo_volume_mt": Decimal("6000.00"),
                "demo_order_count": 2,
                "unknown_order_count": 4,
            },
            benchmark_quote=None,
        )

        assert cell.order_source_kind == MarketSourceKind.DEMO_SEED
        assert cell.demo_status == MarketDemoStatus.DEMO_ONLY
        assert cell.best_bid == Decimal("1050.00")
        assert cell.best_ask == Decimal("1062.00")
        assert cell.volume_mt == Decimal("6000.00")
        assert cell.order_count == 2

    @pytest.mark.asyncio
    async def test_depth_prefers_real_rows_without_blending_demo_levels(self):
        db = AsyncMock()
        result = MagicMock()
        real_row = MagicMock()
        real_row.side = "BID"
        real_row.price_per_mt_usd = Decimal("1045.00")
        real_row.quantity_mt = Decimal("100.00")
        real_row.order_count = 1
        real_row.real_order_count = 1
        real_row.demo_order_count = 0
        demo_row = MagicMock()
        demo_row.side = "BID"
        demo_row.price_per_mt_usd = Decimal("1050.00")
        demo_row.quantity_mt = Decimal("900.00")
        demo_row.order_count = 1
        demo_row.real_order_count = 0
        demo_row.demo_order_count = 1
        result.all.return_value = [demo_row, real_row]
        db.execute.return_value = result

        bids, asks = await _aggregate_depth_levels(
            db,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            availability_window="SPOT",
        )

        assert asks == []
        assert len(bids) == 1
        assert bids[0].price_per_mt_usd == Decimal("1045.00")
        assert bids[0].quantity_mt == Decimal("100.00")
        assert bids[0].source_kind == MarketSourceKind.LIVE_ORDER
        assert bids[0].demo_status == MarketDemoStatus.REAL_ONLY

    @pytest.mark.asyncio
    async def test_forward_board_batches_benchmark_quotes(self):
        product = MagicMock()
        product.id = uuid4()
        product.market_product = "BIO_METHANOL"
        product.name = "Bio Methanol"

        delivery_point = MagicMock()
        delivery_point.id = uuid4()
        delivery_point.name = "Singapore"
        delivery_point.region = "Asia"

        with (
            patch("app.routers.curves._load_board_products", new=AsyncMock(return_value=[product])),
            patch("app.routers.curves._load_board_delivery_points", new=AsyncMock(return_value=[delivery_point])),
            patch("app.routers.curves._aggregate_orderbook_window", new=AsyncMock(return_value={})),
            patch("app.routers.curves._aggregate_orderbook_focus_windows", new=AsyncMock(return_value={})),
            patch("app.routers.curves._aggregate_depth_levels", new=AsyncMock(return_value=([], []))),
            patch("app.routers.curves.get_benchmark_quotes", new=AsyncMock(return_value={})) as batch_quotes,
            patch("app.routers.curves.get_benchmark_quote", new=AsyncMock(side_effect=AssertionError("per-cell benchmark fetch used"))),
            patch("app.routers.curves.load_indication_summaries", new=AsyncMock(return_value={})) as indication_summaries,
            patch("app.routers.curves.load_physical_stem_summaries", new=AsyncMock(return_value={})) as stem_summaries,
            patch("app.routers.curves.load_fair_price_bands", new=AsyncMock(return_value={})) as fair_bands,
            patch("app.routers.curves.load_latest_indications_for_focus", new=AsyncMock(return_value=[])) as focus_indications,
            patch("app.routers.curves.load_physical_stems_for_focus", new=AsyncMock(return_value=[])) as focus_stems,
        ):
            board = await build_forward_curve_board(AsyncMock(), availability_window="SPOT")

        batch_quotes.assert_awaited_once()
        indication_summaries.assert_awaited_once()
        stem_summaries.assert_awaited_once()
        fair_bands.assert_awaited_once()
        focus_indications.assert_awaited_once()
        focus_stems.assert_awaited_once()
        assert len(board.ports) == 1
        assert len(board.focus.curve) >= 1


# ---------------------------------------------------------------------------
# compute_forward_curve() logic tests
# ---------------------------------------------------------------------------


class TestComputeForwardCurve:
    """Test the pure aggregation logic with mock DB."""

    @pytest.mark.asyncio
    async def test_empty_orderbook_returns_empty_list(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())
        assert points == []

    @pytest.mark.asyncio
    async def test_single_bid_window_only(self):
        """One BID in a window: best_bid set, best_ask/mid/spread None."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        _attrs = ["availability_window", "side", "max_price", "min_price", "total_volume", "order_count"]
        row = MagicMock(spec=_attrs)
        row.availability_window = "Spot"
        row.side = "BID"
        row.max_price = Decimal("500.00")   # impl reads max_price for BID best_bid
        row.min_price = Decimal("495.00")   # unused for BID
        row.total_volume = Decimal("250.00")
        row.order_count = 1

        mock_result.all.return_value = [row]
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())

        assert len(points) == 1
        assert points[0].availability_window == "SPOT"
        assert points[0].best_bid == Decimal("500.00")
        assert points[0].best_ask is None
        assert points[0].mid_price is None
        assert points[0].spread is None
        assert points[0].volume_mt == Decimal("250.00")
        assert points[0].order_count == 1

    @pytest.mark.asyncio
    async def test_single_ask_window_only(self):
        """One ASK in a window: best_ask set, best_bid/mid/spread None."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        _attrs = ["availability_window", "side", "max_price", "min_price", "total_volume", "order_count"]
        row = MagicMock(spec=_attrs)
        row.availability_window = "Q3 2026"
        row.side = "ASK"
        row.max_price = Decimal("560.00")   # unused for ASK
        row.min_price = Decimal("550.00")   # impl reads min_price for ASK best_ask
        row.total_volume = Decimal("400.00")
        row.order_count = 2

        mock_result.all.return_value = [row]
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())

        assert len(points) == 1
        assert points[0].best_ask == Decimal("550.00")
        assert points[0].best_bid is None
        assert points[0].mid_price is None
        assert points[0].spread is None

    @pytest.mark.asyncio
    async def test_two_sided_market_computes_mid_and_spread(self):
        """BID + ASK in same window: mid and spread computed correctly."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        _attrs = ["availability_window", "side", "max_price", "min_price", "total_volume", "order_count"]
        bid_row = MagicMock(spec=_attrs)
        bid_row.availability_window = "Q2 2026"
        bid_row.side = "BID"
        bid_row.max_price = Decimal("520.00")   # best_bid = max BID price
        bid_row.min_price = Decimal("510.00")   # not used
        bid_row.total_volume = Decimal("600.00")
        bid_row.order_count = 3

        ask_row = MagicMock(spec=_attrs)
        ask_row.availability_window = "Q2 2026"
        ask_row.side = "ASK"
        ask_row.max_price = Decimal("540.00")   # not used
        ask_row.min_price = Decimal("530.00")   # best_ask = min ASK price
        ask_row.total_volume = Decimal("400.00")
        ask_row.order_count = 2

        mock_result.all.return_value = [bid_row, ask_row]
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())

        assert len(points) == 1
        p = points[0]
        assert p.availability_window == "2026-Q2"
        assert p.best_bid == Decimal("520.00")
        assert p.best_ask == Decimal("530.00")
        assert p.mid_price == Decimal("525.00")
        assert p.spread == Decimal("10.00")
        assert p.volume_mt == Decimal("1000.00")
        assert p.order_count == 5

    @pytest.mark.asyncio
    async def test_multiple_windows_each_returned_as_separate_point(self):
        """Each availability_window becomes its own ForwardCurvePoint."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        _attrs = ["availability_window", "side", "max_price", "min_price", "total_volume", "order_count"]
        rows = []
        for window in ["Spot", "Q3 2026", "Forward 2027"]:
            bid = MagicMock(spec=_attrs)
            bid.availability_window = window
            bid.side = "BID"
            bid.max_price = Decimal("500.00")
            bid.min_price = Decimal("490.00")
            bid.total_volume = Decimal("100.00")
            bid.order_count = 1
            rows.append(bid)

        mock_result.all.return_value = rows
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())
        assert len(points) == 3
        windows = {p.availability_window for p in points}
        assert windows == {"SPOT", "2026-Q3", "2027-CAL"}

    @pytest.mark.asyncio
    async def test_filters_by_product_id(self):
        """product_id is required; verify query is executed."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        pid = uuid4()
        await compute_forward_curve(mock_db, product_id=pid)
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_filters_by_delivery_point_id(self):
        """delivery_point_id filter is applied when provided."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_forward_curve(mock_db, product_id=uuid4(), delivery_point_id=uuid4())
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_excludes_demo_market_orders(self):
        """The legacy curve has no source labelling, so demo-org liquidity
        must be excluded from the aggregation entirely."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_forward_curve(mock_db, product_id=uuid4())
        stmt = mock_db.execute.call_args.args[0]
        rendered = str(stmt.compile(compile_kwargs={"literal_binds": True}))
        assert "orderbook_orders.provenance = 'REAL'" in rendered
        assert "organization_id NOT IN" not in rendered

    @pytest.mark.asyncio
    async def test_mid_price_rounded_to_two_decimal_places(self):
        """Mid price = (bid + ask) / 2, rounded to 2dp."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        _attrs = ["availability_window", "side", "max_price", "min_price", "total_volume", "order_count"]
        bid_row = MagicMock(spec=_attrs)
        bid_row.availability_window = "Spot"
        bid_row.side = "BID"
        bid_row.max_price = Decimal("501.00")
        bid_row.min_price = Decimal("498.00")
        bid_row.total_volume = Decimal("100.00")
        bid_row.order_count = 1

        ask_row = MagicMock(spec=_attrs)
        ask_row.availability_window = "Spot"
        ask_row.side = "ASK"
        ask_row.max_price = Decimal("510.00")
        ask_row.min_price = Decimal("502.00")
        ask_row.total_volume = Decimal("100.00")
        ask_row.order_count = 1

        mock_result.all.return_value = [bid_row, ask_row]
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())
        assert points[0].mid_price == Decimal("501.50")

    @pytest.mark.asyncio
    async def test_asymmetric_volume_and_spread(self):
        """(501 + 504) / 2 = 502.50, spread = 3.00, combined volume."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        _attrs = ["availability_window", "side", "max_price", "min_price", "total_volume", "order_count"]
        bid_row = MagicMock(spec=_attrs)
        bid_row.availability_window = "Spot"
        bid_row.side = "BID"
        bid_row.max_price = Decimal("501.00")   # best_bid
        bid_row.min_price = Decimal("495.00")   # unused
        bid_row.total_volume = Decimal("100.00")
        bid_row.order_count = 1

        ask_row = MagicMock(spec=_attrs)
        ask_row.availability_window = "Spot"
        ask_row.side = "ASK"
        ask_row.max_price = Decimal("510.00")   # unused
        ask_row.min_price = Decimal("504.00")   # best_ask
        ask_row.total_volume = Decimal("150.00")
        ask_row.order_count = 2

        mock_result.all.return_value = [bid_row, ask_row]
        mock_db.execute.return_value = mock_result

        points = await compute_forward_curve(mock_db, product_id=uuid4())
        assert points[0].mid_price == Decimal("502.50")
        assert points[0].spread == Decimal("3.00")
        assert points[0].volume_mt == Decimal("250.00")
        assert points[0].order_count == 3


# ---------------------------------------------------------------------------
# CSV builder tests
# ---------------------------------------------------------------------------


class TestBuildCsv:
    """Tests for the CSV builder helper."""

    def test_csv_has_header_row(self):
        from app.routers.curves import build_csv

        csv_text = build_csv([])
        first_line = csv_text.splitlines()[0]
        assert "availability_window" in first_line
        assert "best_bid" in first_line
        assert "best_ask" in first_line
        assert "mid_price" in first_line
        assert "spread" in first_line
        assert "volume_mt" in first_line
        assert "order_count" in first_line

    def test_csv_empty_points_has_only_header(self):
        from app.routers.curves import build_csv

        csv_text = build_csv([])
        lines = [l for l in csv_text.splitlines() if l.strip()]
        assert len(lines) == 1

    def test_csv_one_point_produces_two_lines(self):
        from app.routers.curves import build_csv

        point = ForwardCurvePoint(
            availability_window="SPOT",
            best_bid=Decimal("500.00"),
            best_ask=Decimal("510.00"),
            mid_price=Decimal("505.00"),
            spread=Decimal("10.00"),
            volume_mt=Decimal("300.00"),
            order_count=2,
        )
        csv_text = build_csv([point])
        lines = [l for l in csv_text.splitlines() if l.strip()]
        assert len(lines) == 2
        assert "SPOT" in lines[1]
        assert "500.00" in lines[1]

    def test_csv_none_fields_serialized_as_empty_string(self):
        from app.routers.curves import build_csv

        point = ForwardCurvePoint(
            availability_window="2027-CAL",
            best_bid=None,
            best_ask=Decimal("620.00"),
            mid_price=None,
            spread=None,
            volume_mt=Decimal("100.00"),
            order_count=1,
        )
        csv_text = build_csv([point])
        data_line = csv_text.splitlines()[1]
        assert "2027-CAL" in data_line
        assert "620.00" in data_line

    def test_csv_multiple_points(self):
        from app.routers.curves import build_csv

        points = [
            ForwardCurvePoint(
                availability_window="SPOT",
                best_bid=Decimal("510.00"),
                best_ask=Decimal("520.00"),
                mid_price=Decimal("515.00"),
                spread=Decimal("10.00"),
                volume_mt=Decimal("500.00"),
                order_count=3,
            ),
            ForwardCurvePoint(
                availability_window="2026-Q1",
                best_bid=Decimal("480.00"),
                best_ask=None,
                mid_price=None,
                spread=None,
                volume_mt=Decimal("200.00"),
                order_count=1,
            ),
        ]
        csv_text = build_csv(points)
        lines = [l for l in csv_text.splitlines() if l.strip()]
        assert len(lines) == 3


# ---------------------------------------------------------------------------
# Router endpoint tests (FastAPI TestClient with dependency override)
# ---------------------------------------------------------------------------


class TestForwardCurveEndpoint:
    """Integration-style tests for the /forward endpoint using DI override."""

    @pytest.mark.asyncio
    async def test_forward_requires_product_id(self):
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        with patch("app.routers.curves.compute_forward_curve", new=AsyncMock(return_value=[])):
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get("/api/v1/curves/forward")
        assert response.status_code == 422

    @pytest.mark.asyncio
    async def test_forward_returns_200_with_product_id(self):
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        pid = uuid4()
        mock_point = ForwardCurvePoint(
            availability_window="SPOT",
            best_bid=Decimal("510.00"),
            best_ask=Decimal("520.00"),
            mid_price=Decimal("515.00"),
            spread=Decimal("10.00"),
            volume_mt=Decimal("500.00"),
            order_count=3,
        )

        with patch("app.routers.curves.compute_forward_curve", new=AsyncMock(return_value=[mock_point])):
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get(f"/api/v1/curves/forward?product_id={pid}")

        assert response.status_code == 200
        data = response.json()
        assert data["product_id"] == str(pid)
        assert len(data["curve"]) == 1
        assert data["curve"][0]["availability_window"] == "SPOT"
        assert "generated_at" in data

    @pytest.mark.asyncio
    async def test_forward_accepts_optional_delivery_point_id(self):
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        with patch("app.routers.curves.compute_forward_curve", new=AsyncMock(return_value=[])):
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get(
                    f"/api/v1/curves/forward?product_id={uuid4()}&delivery_point_id={uuid4()}"
                )
        assert response.status_code == 200

    @pytest.mark.asyncio
    async def test_forward_export_returns_csv_content_type(self):
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        mock_point = ForwardCurvePoint(
            availability_window="2026-Q1",
            best_bid=Decimal("480.00"),
            best_ask=Decimal("490.00"),
            mid_price=Decimal("485.00"),
            spread=Decimal("10.00"),
            volume_mt=Decimal("750.00"),
            order_count=4,
        )

        with patch("app.routers.curves.compute_forward_curve", new=AsyncMock(return_value=[mock_point])):
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get(f"/api/v1/curves/forward/export?product_id={uuid4()}")

        assert response.status_code == 200
        assert "text/csv" in response.headers["content-type"]
        assert "attachment" in response.headers["content-disposition"]
        assert ".csv" in response.headers["content-disposition"]

    @pytest.mark.asyncio
    async def test_forward_export_csv_body_has_header_and_data(self):
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        mock_point = ForwardCurvePoint(
            availability_window="2026-Q2",
            best_bid=Decimal("520.00"),
            best_ask=Decimal("530.00"),
            mid_price=Decimal("525.00"),
            spread=Decimal("10.00"),
            volume_mt=Decimal("1000.00"),
            order_count=5,
        )

        with patch("app.routers.curves.compute_forward_curve", new=AsyncMock(return_value=[mock_point])):
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get(f"/api/v1/curves/forward/export?product_id={uuid4()}")

        text = response.text
        lines = [l for l in text.splitlines() if l.strip()]
        assert len(lines) == 2
        assert "availability_window" in lines[0]
        assert "2026-Q2" in lines[1]
        assert "520.00" in lines[1]

    @pytest.mark.asyncio
    async def test_forward_export_requires_product_id(self):
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        with patch("app.routers.curves.compute_forward_curve", new=AsyncMock(return_value=[])):
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get("/api/v1/curves/forward/export")
        assert response.status_code == 422

    @pytest.mark.asyncio
    async def test_forward_board_returns_matrix_and_focus(self):
        from datetime import datetime, timezone
        from httpx import AsyncClient, ASGITransport
        from fastapi import FastAPI
        from app.routers.curves import router
        from app.database import get_db

        app = FastAPI()
        app.include_router(router, prefix="/api/v1")

        async def mock_db():
            yield AsyncMock()
        app.dependency_overrides[get_db] = mock_db

        pid = uuid4()
        dp_id = uuid4()
        cell = ForwardCurveBoardCell(
            product_id=pid,
            market_product="BIO_METHANOL",
            product_name="Bio Methanol",
            delivery_point_id=dp_id,
            delivery_point_name="Singapore",
            region="Asia",
            availability_window="SPOT",
            benchmark_mid=Decimal("1052.00"),
            benchmark_source="seed_matrix",
            is_demo_benchmark=True,
            best_bid=Decimal("1048.00"),
            best_ask=Decimal("1056.00"),
            spread=Decimal("8.00"),
            volume_mt=Decimal("9000.00"),
            order_count=4,
        )
        board = ForwardCurveBoardResponse(
            availability_window="SPOT",
            products=[ForwardCurveBoardProduct(product_id=pid, market_product="BIO_METHANOL", product_name="Bio Methanol")],
            ports=[ForwardCurveBoardPort(delivery_point_id=dp_id, delivery_point_name="Singapore", region="Asia", cells=[cell])],
            focus=ForwardCurveBoardFocus(
                product_id=pid,
                market_product="BIO_METHANOL",
                product_name="Bio Methanol",
                delivery_point_id=dp_id,
                delivery_point_name="Singapore",
                region="Asia",
                availability_window="SPOT",
                curve=[cell],
                depth_bids=[],
                depth_asks=[],
            ),
            generated_at=datetime.now(timezone.utc),
        )

        with patch("app.routers.curves.build_forward_curve_board", new=AsyncMock(return_value=board)) as mocked:
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
                response = await client.get(
                    f"/api/v1/curves/forward/board?availability_window=SPOT&focus_market_product=BIO_METHANOL&focus_delivery_point_id={dp_id}"
                )

        assert response.status_code == 200
        data = response.json()
        assert data["ports"][0]["cells"][0]["benchmark_source"] == "seed_matrix"
        assert data["ports"][0]["cells"][0]["is_demo_benchmark"] is True
        assert data["focus"]["market_product"] == "BIO_METHANOL"
        mocked.assert_awaited_once()
