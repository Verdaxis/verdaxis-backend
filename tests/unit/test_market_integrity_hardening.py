"""Sprint 2 market-integrity policy tests.

These tests intentionally cover policy at the model/schema/service boundaries so
the SQLite unit suite can exercise the safety contract without pretending to be
PostgreSQL. PostgreSQL-only locking and constraint tests live in tests/postgres.
"""
from __future__ import annotations

from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4
from inspect import signature, unwrap
from types import SimpleNamespace
from pathlib import Path

import pytest
from fastapi import HTTPException
from pydantic import ValidationError
from unittest.mock import AsyncMock

from app.config import Settings
from app.models.orderbook import OrderBookOrder, OrderSide, Trade
from app.models.user import Organization, OrganizationProvenance
from app.schemas.orderbook import OrderCreate, TradeCreate, TradeDeliverPayload
from app.schemas.marketplace import InventoryCreate
from app.services.availability_windows import tradable_availability_windows
from app.services.demo_market import (
    DEMO_ACTIVITY_BUYER_ORG_ID,
    DEMO_ACTIVITY_SELLER_ORG_ID,
)
from app.services.market_data_eligibility import (
    market_data_eligible_organization_clause,
)
from app.services.matching_engine import match_order
from app.services.provenance import (
    INTEGRATION_TEST_ORG_IDS,
    classify_organization_provenance,
    execution_provenance_compatible,
)


def test_market_seed_uses_a_run_marker_not_an_economic_sentinel():
    from app.models.seed import SeedRun
    from app.seeds.market_seed import MARKET_SEED_NAME, _SENTINEL_ID

    assert _SENTINEL_ID is None
    assert SeedRun.__tablename__ == "seed_runs"
    assert MARKET_SEED_NAME == "market"


def test_integrity_migration_reports_sentinel_instead_of_deleting_it():
    migration = Path("alembic/versions/mi_20260720_market_integrity.py").read_text()
    assert "market-integrity preflight" in migration
    assert "DELETE FROM orderbook_orders" not in migration
    assert "current_setting('verdaxis.trusted_provenance_promotion'" not in migration
    assert "verdaxis_admin_promote_org_provenance" not in migration
    assert "SECURITY DEFINER" not in migration


def test_demo_reset_requires_explicit_nonproduction_opt_in():
    from app.seeds.market_seed import validate_demo_reset

    with pytest.raises(RuntimeError, match="production"):
        validate_demo_reset(
            environment="production",
            explicit_opt_in=True,
            database_url="postgresql+asyncpg://seed:secret@db/verdaxis",
            current_database="verdaxis",
        )
    with pytest.raises(RuntimeError, match="explicit"):
        validate_demo_reset(
            environment="staging",
            explicit_opt_in=False,
            database_url="postgresql+asyncpg://seed:secret@db/verdaxis_staging",
            current_database="verdaxis_staging",
        )
    validate_demo_reset(
        environment="staging",
        explicit_opt_in=True,
        database_url="postgresql+asyncpg://seed:secret@db/verdaxis_staging",
        current_database="verdaxis_staging",
    )


@pytest.mark.parametrize("field", ["current_stock_mt", "incoming_stock_mt", "reserved_stock_mt", "price_per_mt_usd"])
def test_inventory_api_uses_finite_two_decimal_values(field):
    payload = {
        "port_id": "SGSIN",
        "fuel_type": "Methanol",
        "current_stock_mt": "100.00",
        "incoming_stock_mt": "0.00",
        "reserved_stock_mt": "0.00",
        "price_per_mt_usd": "700.00",
    }
    payload[field] = "NaN"
    with pytest.raises(ValidationError):
        InventoryCreate(**payload)
    payload[field] = "1.001"
    with pytest.raises(ValidationError):
        InventoryCreate(**payload)


def test_trade_snapshots_market_identity_without_order_fallback():
    from app.models.orderbook import Trade

    assert {
        "product_id",
        "product_name",
        "market_product",
        "delivery_point_id",
        "delivery_point_name",
        "delivery_point_region",
        "availability_window",
    }.issubset(
        Trade.__table__.c.keys()
    )


def test_market_slice_lock_keys_have_deterministic_sorted_batch_helper():
    from app.services.market_locks import market_slice_lock_sort_key, acquire_market_slice_locks

    first = (OrderSide.ASK, uuid4(), uuid4(), "SPOT")
    second = (OrderSide.BID, uuid4(), uuid4(), "2026-Q3")
    assert market_slice_lock_sort_key(first) < market_slice_lock_sort_key(second) or market_slice_lock_sort_key(second) < market_slice_lock_sort_key(first)
    assert callable(acquire_market_slice_locks)


def test_public_provenance_policy_never_labels_unknown_as_live():
    from app.services.market_provenance import order_market_provenance

    result = order_market_provenance(SimpleNamespace(provenance=OrganizationProvenance.UNKNOWN, delivery_point_id=None))
    assert result["source_kind"] == "UNKNOWN"
    assert result["demo_status"] == "UNKNOWN"


def _order_payload(**overrides):
    payload = {
        "side": "BID",
        "product_id": uuid4(),
        "delivery_point_id": uuid4(),
        "quantity_mt": "100.00",
        "price_per_mt_usd": "700.00",
        "availability_window": "SPOT",
    }
    payload.update(overrides)
    return payload


@pytest.mark.parametrize("field,value", [("quantity_mt", "1.001"), ("price_per_mt_usd", "1.001")])
def test_order_schema_rejects_over_precision(field, value):
    with pytest.raises(ValidationError):
        OrderCreate(**_order_payload(**{field: value}))


@pytest.mark.parametrize("value", ["NaN", "Infinity", "-Infinity"])
def test_order_schema_rejects_non_finite_numbers(value):
    with pytest.raises(ValidationError):
        OrderCreate(**_order_payload(quantity_mt=value))


def test_order_schema_rejects_naive_and_past_expiry():
    with pytest.raises(ValidationError):
        OrderCreate(**_order_payload(expires_at=datetime.now() + timedelta(hours=1)))
    with pytest.raises(ValidationError):
        OrderCreate(**_order_payload(expires_at=datetime.now(UTC) - timedelta(seconds=1)))


def test_trade_payloads_apply_same_finite_precision_policy():
    with pytest.raises(ValidationError):
        TradeCreate(order_id=uuid4(), quantity_mt="1.001")
    with pytest.raises(ValidationError):
        TradeDeliverPayload(final_quantity_mt="NaN", final_price_per_mt="2")


def test_provenance_is_explicit_and_unknown_is_not_real():
    assert classify_organization_provenance(DEMO_ACTIVITY_BUYER_ORG_ID) == OrganizationProvenance.DEMO
    assert classify_organization_provenance(DEMO_ACTIVITY_SELLER_ORG_ID) == OrganizationProvenance.DEMO
    assert classify_organization_provenance(uuid4()) == OrganizationProvenance.UNKNOWN
    assert classify_organization_provenance(next(iter(INTEGRATION_TEST_ORG_IDS))) == OrganizationProvenance.TEST
    assert not execution_provenance_compatible(OrganizationProvenance.UNKNOWN, OrganizationProvenance.REAL)
    assert not execution_provenance_compatible(OrganizationProvenance.UNKNOWN, OrganizationProvenance.UNKNOWN)
    assert execution_provenance_compatible(OrganizationProvenance.REAL, OrganizationProvenance.REAL)
    assert not execution_provenance_compatible(OrganizationProvenance.DEMO, OrganizationProvenance.DEMO)
    assert execution_provenance_compatible(
        OrganizationProvenance.DEMO,
        OrganizationProvenance.DEMO,
        left_org_id=DEMO_ACTIVITY_BUYER_ORG_ID,
        right_org_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        allowed_demo_org_pair={
            DEMO_ACTIVITY_BUYER_ORG_ID,
            DEMO_ACTIVITY_SELLER_ORG_ID,
        },
    )
    assert not execution_provenance_compatible(OrganizationProvenance.DEMO, OrganizationProvenance.REAL)
    assert not execution_provenance_compatible(OrganizationProvenance.TEST, OrganizationProvenance.REAL)
    assert not execution_provenance_compatible(OrganizationProvenance.TEST, OrganizationProvenance.TEST)
    assert not execution_provenance_compatible(OrganizationProvenance.CANARY, OrganizationProvenance.CANARY)


def test_release_has_no_callable_provenance_promotion_helper():
    from app.services import provenance

    assert not hasattr(provenance, "promote_organization_provenance")
    assert not hasattr(provenance, "trusted_promote_organization_provenance")
    assert not hasattr(provenance, "promote_approved_organizations_to_real")


def test_provenance_snapshots_and_idempotency_columns_are_modelled():
    assert "provenance" in Organization.__table__.c
    assert "provenance" in OrderBookOrder.__table__.c
    assert "buyer_provenance" in Trade.__table__.c
    assert "seller_provenance" in Trade.__table__.c
    assert "idempotency_key" in OrderBookOrder.__table__.c
    assert "idempotency_operation" in OrderBookOrder.__table__.c
    assert "idempotency_request_hash" in Trade.__table__.c
    assert "idempotency_operation" in Trade.__table__.c
    assert "delivery_point_region" in Trade.__table__.c


def test_trade_response_uses_snapshots_without_touching_lazy_orders():
    from datetime import UTC, datetime
    from uuid import uuid4

    from app.models.orderbook import Initiator, TradeStatus
    from app.routers.trades import build_trade_response

    trade = Trade(
        id=uuid4(),
        buyer_id=uuid4(),
        seller_id=uuid4(),
        initiator_org_id=uuid4(),
        buyer_provenance=OrganizationProvenance.REAL,
        seller_provenance=OrganizationProvenance.REAL,
        initiated_by=Initiator.BUYER,
        quantity_mt=Decimal("12.34"),
        price_per_mt_usd=Decimal("701.25"),
        status=TradeStatus.PENDING_CONFIRMATION,
        product_id=uuid4(),
        product_name="Immutable Product",
        fuel_type="Methanol",
        fuel_grade="Bio",
        market_product="BIO_METHANOL",
        delivery_point_id=uuid4(),
        delivery_point_name="Singapore",
        delivery_point_region="Asia",
        availability_window="SPOT",
        created_at=datetime.now(UTC),
    )

    response = build_trade_response(trade)

    assert response.product_name == "Immutable Product"
    assert response.market_product == "BIO_METHANOL"
    assert response.delivery_point_name == "Singapore"
    assert response.region == "Asia"


def test_idempotency_lock_is_bounded_and_maps_database_timeouts():
    source = Path("app/services/idempotency.py").read_text()
    assert "SET LOCAL lock_timeout" in source
    assert "SET LOCAL statement_timeout" in source
    assert '"55P03"' in source
    assert '"40P01"' in source
    assert '"57014"' in source
    assert "await db.rollback()" in source


def test_demo_registry_is_pair_specific_and_current_windows_are_canonical():
    assert DEMO_ACTIVITY_BUYER_ORG_ID != DEMO_ACTIVITY_SELLER_ORG_ID
    windows = tradable_availability_windows(today=datetime(2026, 7, 20, tzinfo=UTC).date())
    assert windows[0] == "SPOT"
    assert "2026-06" not in windows
    assert "2026-Q2" not in windows
    assert all(window == "SPOT" or len(window) in (7, 8) for window in windows)


def test_demo_matching_requires_exact_order_pair_allowlist():
    parameters = signature(match_order).parameters
    assert "allowed_demo_order_pair" in parameters
    assert "allow_demo_pair" not in parameters


def test_market_data_eligibility_does_not_reference_names_or_domains():
    clause = market_data_eligible_organization_clause(Organization)
    rendered = str(clause.compile(compile_kwargs={"literal_binds": True}))
    assert "verification_status" in rendered
    assert "REAL" in rendered
    assert "UNKNOWN" not in rendered
    assert "name" not in rendered.lower()
    assert "domain" not in rendered.lower()


def test_public_order_collection_quarantines_unknown_test_and_canary():
    from app.services.market_data_eligibility import public_order_collection_provenance_clause

    rendered = str(
        public_order_collection_provenance_clause(OrderBookOrder).compile(
            compile_kwargs={"literal_binds": True}
        )
    )
    assert "REAL" in rendered
    assert "DEMO" in rendered
    assert "UNKNOWN" not in rendered
    assert "TEST" not in rendered
    assert "CANARY" not in rendered


def test_bilateral_execution_has_no_runtime_enable_flags():
    configured = Settings(ENVIRONMENT="test", JWT_SECRET="x" * 32)
    assert not hasattr(configured, "RFQ_ACCEPTANCE_ENABLED")
    assert not hasattr(configured, "NEGOTIATION_ACCEPTANCE_ENABLED")


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("endpoint", "arguments"),
    [
        (
            "rfq",
            {"rfq_id": uuid4(), "quote_id": uuid4()},
        ),
        (
            "negotiation",
            {"negotiation_id": uuid4()},
        ),
    ],
)
async def test_bilateral_execution_is_permanently_409_disabled(endpoint, arguments):
    if endpoint == "rfq":
        from app.routers.rfq import accept_quote as handler
    else:
        from app.routers.negotiations import accept_negotiation as handler

    db = AsyncMock()
    with pytest.raises(HTTPException) as rejected:
        await unwrap(handler)(
            request=SimpleNamespace(),
            db=db,
            current_user=SimpleNamespace(organization_id=uuid4()),
            _security_admission=None,
            **arguments,
        )

    assert rejected.value.status_code == 409
    assert "disabled" in rejected.value.detail.lower()
    db.execute.assert_not_awaited()
