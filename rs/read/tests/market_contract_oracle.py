"""Seed the Task 6 visibility matrix and answer it with the Python oracle.

`run_market_contract.sh` migrates a disposable database, then runs this helper
against it. The helper inserts one fixture row per rule in the public-market
rules table and writes the ids Python selects, under one fixed clock, to a JSON
file the Rust test reads. It never contacts staging or production and it
requires a non-superuser fixture writer.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from uuid import UUID, uuid5, NAMESPACE_URL

ROOT = Path(__file__).resolve().parents[3]

# The disposable runner sets this; the Rust side is given the same instant.
CLOCK_ENV = "RUST_MARKET_TEST_CLOCK"
_TAG = "rust-market-contract"


def fid(name: str) -> UUID:
    """A deterministic fixture identity, unique to this helper."""
    return uuid5(NAMESPACE_URL, f"{_TAG}:{name}")


async def _insert(session, statement: str, values: dict) -> None:
    from sqlalchemy import text

    await session.execute(text(statement), values)


async def seed(session, clock: datetime) -> dict:
    """Insert the fixture matrix. Every row is explicit; nothing is inferred."""
    from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
    from sqlalchemy import text

    product = {"Bio-LNG": None, "UCOME B100": None, "Bio Methanol": None,
               "e-Methanol": None, "Bio Ethanol": None, "Synthetic Ethanol": None}
    # The catalog migrations already seed these rows; this only completes or
    # deactivates them so the matrix is independent of seed history.
    for spec in CANONICAL_PRODUCTS:
        await _insert(session, INSERT_PRODUCT, {
            "id": spec.id,
            "name": spec.name,
            "fuel_type": spec.fuel_type,
            "fuel_grade": spec.fuel_grade,
            "unit": spec.unit,
            "min_lot_size": spec.min_lot_size,
            "is_active": True,
        })
        product[spec.name] = spec.id
    point = {}
    for spec in CANONICAL_DELIVERY_POINTS:
        await _insert(session, INSERT_POINT, {
            "id": spec.id,
            "name": spec.name,
            "region": spec.region,
            "timezone": spec.timezone,
            "is_active": True,
        })
        point[spec.name] = spec.id

    # The inactive cases: one canonical product and one canonical point are
    # still fully canonical by identity but carry an inactive row.
    await _insert(session, DEACTIVATE_PRODUCT, {"id": product["Synthetic Ethanol"]})
    await _insert(session, DEACTIVATE_POINT, {"id": point["Santos"]})

    # Non-canonical decoys: an alias product and an alias delivery point.
    alias_product = fid("product:alias")
    await _insert(session, INSERT_PRODUCT, {
        "id": alias_product, "name": "Methanol Green", "fuel_type": "Methanol",
        "fuel_grade": "Green", "unit": "MT", "min_lot_size": 100, "is_active": True,
    })
    alias_point = fid("point:alias")
    await _insert(session, INSERT_POINT, {
        "id": alias_point, "name": "Rotterdam II", "region": "Europe",
        "timezone": "Europe/Amsterdam", "is_active": True,
    })

    # The organizations trigger admits DEMO and TEST provenance only for the
    # deterministic seed identities, and refuses CANARY outright. A CANARY
    # market row is therefore unrepresentable in a freshly migrated database
    # and is not part of this matrix.
    from app.demo_identities import (
        DEMO_SEED_BUYERS,
        DEMO_SEED_SUPPLIERS,
        KNOWN_TEST_ORG_IDS,
    )

    test_ids = sorted(KNOWN_TEST_ORG_IDS)
    organizations = {
        "real": ("REAL", "APPROVED"),
        "real_alt": ("REAL", "APPROVED"),
        "real_admin_only": ("REAL", "APPROVED"),
        "demo_buyer": ("DEMO", "APPROVED"),
        "demo_seller": ("DEMO", "APPROVED"),
        "test_a": ("TEST", "APPROVED"),
        "test_b": ("TEST", "APPROVED"),
        "unknown": ("UNKNOWN", "APPROVED"),
        "pending": ("REAL", "PENDING"),
    }
    org = {}
    for key, (provenance, verification) in organizations.items():
        fixed = {
            "demo_buyer": DEMO_SEED_BUYERS[0][0],
            "demo_seller": DEMO_SEED_SUPPLIERS[0][0],
            "test_a": test_ids[0],
            "test_b": test_ids[1],
        }.get(key)
        identifier = fixed or fid(f"org:{key}")
        org[key] = identifier
        await _insert(session, INSERT_ORGANIZATION, {
            "id": identifier,
            "name": f"Fixture {key}",
            "verification_status": verification,
            "provenance": provenance,
        })

    users = {
        # name -> (org, role, status, verified, must_change, kyc_status, kyc_org)
        "real": ("real", "BUYER", "APPROVED", True, False, "APPROVED", "real"),
        "rejected": ("real", "BUYER", "APPROVED", True, False, "REJECTED", "real"),
        "grace_ok": ("real", "BUYER", "APPROVED", True, False, "PENDING", None),
        "grace_edge": ("real", "BUYER", "APPROVED", True, False, "PENDING", None),
        "moved": ("real_alt", "BUYER", "APPROVED", True, False, "APPROVED", "real_alt"),
        "unverified": ("real", "BUYER", "APPROVED", False, False, "APPROVED", "real"),
        "must_change": ("real", "BUYER", "APPROVED", True, True, "APPROVED", "real"),
        "pending": ("real", "BUYER", "PENDING", True, False, "APPROVED", "real"),
        "admin": ("real", "ADMIN", "APPROVED", True, False, "PENDING", None),
        "customer": ("real", "SUPPLIER", "APPROVED", True, False, "APPROVED", "real"),
        "admin_only": ("real_admin_only", "ADMIN", "APPROVED", True, False, "PENDING", None),
    }
    users.update({
        "rejected_with_grace": ("real", "BUYER", "APPROVED", True, False, "REJECTED", "real"),
        "wrong_kyc_org": ("real", "BUYER", "APPROVED", True, False, "APPROVED", "real_alt"),
        "wrong_grace_org": ("real", "BUYER", "APPROVED", True, False, "PENDING", None),
        "grace_before": ("real", "BUYER", "APPROVED", True, False, "PENDING", None),
        "grace_after": ("real", "BUYER", "APPROVED", True, False, "PENDING", None),
        "org_revoked": ("pending", "BUYER", "APPROVED", True, False, "APPROVED", "pending"),
        "admin_rejected": ("real", "ADMIN", "REJECTED", True, False, "APPROVED", "real"),
    })
    user = {}
    for key, (org_key, role, status, verified, must_change, kyc, kyc_org) in users.items():
        identifier = fid(f"user:{key}")
        user[key] = identifier
        await _insert(session, INSERT_USER, {
            "id": identifier,
            "email": f"fixture-{key}@rust-market.test",
            "role": role,
            "status": status,
            "organization_id": org[org_key],
            "email_verified": verified,
            "must_change_password": must_change,
            "kyc_status": kyc,
            "kyc_organization_id": org[kyc_org] if kyc_org else None,
        })
    for key, delta in (("grace_ok", timedelta(hours=1)), ("grace_edge", timedelta(0)),
                       ("rejected_with_grace", timedelta(hours=1)),
                       ("wrong_grace_org", timedelta(hours=1)),
                       ("grace_before", timedelta(microseconds=-1)),
                       ("grace_after", timedelta(microseconds=1))):
        await _insert(session, INSERT_GRACE, {
            "id": fid(f"grace:{key}"),
            "user_id": user[key],
            "organization_id": org["real_alt"] if key == "wrong_grace_org" else org["real"],
            "expires_at": clock + delta,
        })

    methanol = product["Bio Methanol"]
    fame = product["UCOME B100"]
    lng = product["Bio-LNG"]
    singapore = point["Singapore"]
    rotterdam = point["Rotterdam"]

    def order(name: str, **fields) -> dict:
        values = {
            "id": fid(f"order:{name}"),
            "organization_id": org["real"],
            "owner_user_id": user["real"],
            "created_by_actor_user_id": None,
            "creation_method": "SELF_SERVICE",
            "provenance": "REAL",
            "side": "BID",
            "product_id": methanol,
            "delivery_point_id": singapore,
            "quantity_mt": "100.00",
            "remaining_quantity_mt": "100.00",
            "price_per_mt_usd": "600.00",
            "availability_window": "SPOT",
            "status": "OPEN",
            "expires_at": None,
            "off_spec": False,
            "certification_declared": False,
            "certification_scheme": None,
            "specification_standard": None,
            "msds_available": False,
            "carbon_intensity_gco2_mj": None,
            "feedstock": None,
            "origin": None,
            "certifications": "[]",
            "fame_terms": None,
            "lng_terms": None,
        }
        values.update(fields)
        for key in ("certifications", "fame_terms", "lng_terms"):
            if isinstance(values[key], (dict, list)):
                values[key] = json.dumps(values[key])
        return values

    scalar_declarations = {
        "certification_scheme": "ISCC EU",
        "certification_declared": True,
        "specification_standard": "EN 14214",
        "msds_available": True,
        "carbon_intensity_gco2_mj": "18.40",
        "feedstock": "Used cooking oil",
        "origin": "Singapore",
    }
    fame_terms = {
        "schema_version": 1,
        "side": "ASK",
        "neat_fame": True,
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "EN 14214:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "CERT-1",
        "certificate_holder": "Fixture holder",
        "certificate_valid_until": (clock + timedelta(days=365)).date().isoformat(),
        "evidence_status": "DECLARED",
        "document_references": [],
        "evidence_due": "BEFORE_LOADING",
    }
    lng_terms = {
        "side": "ASK",
        "sustainability_scheme": "ISCC_EU",
        "chain_of_custody": "MASS_BALANCE",
        "union_database_registered": True,
        "bio_share_pct": "85.00",
        "feedstock": "MANURE",
        "production_country": "NL",
        "wtt_ghg_gco2e_mj": "12.30",
        "specification_standard": "ISO 23306:2020",
        "delivery_method": "SHIP_TO_SHIP",
        "seller_is_physical_supplier": True,
        "pos_timing": "WITH_DELIVERY",
        "scheme_certificate_number": "ISCC-EU-1",
        "physical_supplier_name": "Fixture supplier",
    }

    future = clock + timedelta(hours=24)
    past = clock - timedelta(hours=24)
    rows = [
        order("real_scalar_bid"),
        # The database requires an order's provenance to equal its owner
        # organization's, so each synthetic class uses its own fixture org.
        order("demo_ownerless_bid", provenance="DEMO", organization_id=org["demo_buyer"],
              owner_user_id=None, expires_at=future),
        order("unknown_ownerless_bid", provenance="UNKNOWN",
              organization_id=org["unknown"], owner_user_id=None),
        order("unknown_expired_bid", provenance="UNKNOWN",
              organization_id=org["unknown"], owner_user_id=None, expires_at=past),
        order("test_ownerless_bid", provenance="TEST", organization_id=org["test_a"],
              owner_user_id=None, expires_at=future),
        order("rejected_owner_bid", owner_user_id=user["rejected"]),
        order("grace_ok_bid", owner_user_id=user["grace_ok"]),
        order("grace_edge_bid", owner_user_id=user["grace_edge"]),
        order("moved_owner_bid", owner_user_id=user["moved"]),
        order("unverified_owner_bid", owner_user_id=user["unverified"]),
        order("must_change_owner_bid", owner_user_id=user["must_change"]),
        order("pending_owner_bid", owner_user_id=user["pending"]),
        order("org_pending_bid", organization_id=org["pending"], owner_user_id=None),
        order("assisted_ok_bid", creation_method="MARKET_SUPPORT",
              owner_user_id=user["admin"], created_by_actor_user_id=user["admin"]),
        order("assisted_no_member_bid", organization_id=org["real_admin_only"],
              creation_method="MARKET_SUPPORT", owner_user_id=user["admin_only"],
              created_by_actor_user_id=user["admin_only"]),
        order("offspec_bid", off_spec=True),
        order("alias_product_bid", product_id=alias_product),
        order("inactive_product_bid", product_id=product["Synthetic Ethanol"]),
        order("alias_point_bid", delivery_point_id=alias_point),
        order("inactive_point_bid", delivery_point_id=point["Santos"]),
        order("ask_missing_declaration", side="ASK"),
        order("ask_ok", side="ASK", **scalar_declarations),
        order("fame_ask_ok", side="ASK", product_id=fame, fame_terms=fame_terms,
              **{k: v for k, v in scalar_declarations.items() if k != "carbon_intensity_gco2_mj"}),
        order("fame_ask_expired", side="ASK", product_id=fame,
              fame_terms={**fame_terms, "certificate_valid_until": "2020-01-01"},
              **{k: v for k, v in scalar_declarations.items() if k != "carbon_intensity_gco2_mj"}),
        order("lng_ask_ok", side="ASK", product_id=lng, delivery_point_id=rotterdam,
              lng_terms=lng_terms,
              **{k: v for k, v in scalar_declarations.items() if k != "carbon_intensity_gco2_mj"}),
        order("lng_ask_reseller", side="ASK", product_id=lng, delivery_point_id=rotterdam,
              lng_terms={**lng_terms, "seller_is_physical_supplier": False},
              **{k: v for k, v in scalar_declarations.items() if k != "carbon_intensity_gco2_mj"}),
        order("partial_bid", status="PARTIALLY_FILLED", remaining_quantity_mt="40.00"),
        order("filled_bid", status="FILLED", remaining_quantity_mt="0.00"),
        order("demo_expired_bid", provenance="DEMO", organization_id=org["demo_buyer"],
              owner_user_id=None, expires_at=past),
        order("real_expired_bid", expires_at=past),
    ]
    additions = {
        name: order(name, owner_user_id=user[name])
        for name in ("rejected_with_grace", "wrong_kyc_org", "wrong_grace_org", "grace_before", "grace_after")
    }
    additions.update({
        "org_revoked": order("org_revoked", organization_id=org["pending"], owner_user_id=user["org_revoked"]),
        "assisted_rejected": order("assisted_rejected", creation_method="MARKET_SUPPORT",
            owner_user_id=user["admin_rejected"], created_by_actor_user_id=user["admin_rejected"]),
        "assisted_mismatched": order("assisted_mismatched", creation_method="MARKET_SUPPORT",
            owner_user_id=user["admin"], created_by_actor_user_id=user["admin_only"]),
        "admin_kyc_only": order("admin_kyc_only", organization_id=org["real_admin_only"],
            creation_method="MARKET_SUPPORT", owner_user_id=user["admin_only"], created_by_actor_user_id=user["admin_only"]),
        "expiry_equal": order("expiry_equal", expires_at=clock),
        "demo_null_expiry": order("demo_null_expiry", provenance="DEMO", organization_id=org["demo_buyer"], owner_user_id=None),
        "zero_remaining": order("zero_remaining", remaining_quantity_mt="0.00"),
        "typed_invalid_visible": order("typed_invalid_visible", side="ASK", product_id=fame,
            fame_terms={**fame_terms, "certificate_holder": ""}, **scalar_declarations),
        "fame_wrong_lane": order("fame_wrong_lane", side="ASK", product_id=fame,
            delivery_point_id=rotterdam, fame_terms=fame_terms, **scalar_declarations),
        "lng_wrong_lane": order("lng_wrong_lane", side="ASK", product_id=lng,
            delivery_point_id=singapore, lng_terms=lng_terms, **scalar_declarations),
    })
    from sqlalchemy.exc import IntegrityError, DBAPIError
    cases = {}
    for values in rows:
        await _insert(session, INSERT_ORDER, values)
    for name, values in additions.items():
        try:
            async with session.begin_nested():
                await _insert(session, INSERT_ORDER, values)
        except (IntegrityError, DBAPIError) as error:
            if name not in {"fame_wrong_lane", "lng_wrong_lane", "demo_null_expiry", "zero_remaining", "assisted_mismatched"}:
                raise
            cases[name] = {"guard_refused": True, "sqlstate": getattr(error.orig, "sqlstate", None)}
        else:
            cases[name] = {"id": str(values["id"])}
    await session.execute(text("UPDATE users SET kyc_status='APPROVED', kyc_organization_id=organization_id WHERE id=:id"), {"id": user["admin_only"]})

    trade_product = {
        "product_id": methanol,
        "product_name": "Bio Methanol",
        "fuel_type": "Methanol",
        "fuel_grade": "Bio",
        "market_product": "BIO_METHANOL",
        "delivery_point_id": singapore,
        "delivery_point_name": "Singapore",
        "delivery_point_region": "Asia",
        "availability_window": "SPOT",
    }

    def trade(name: str, **fields) -> dict:
        values = {
            "id": fid(f"trade:{name}"),
            **trade_product,
            "buyer_id": org["real"],
            "seller_id": org["real_alt"],
            "buyer_provenance": "REAL",
            "seller_provenance": "REAL",
            "initiated_by": "BUYER",
            "initiator_org_id": org["real"],
            "quantity_mt": "100.00",
            "price_per_mt_usd": "600.00",
            "status": "CONFIRMED",
            "confirmed_at": clock,
            "market_snapshot_version": 1,
            "delivered_at": None,
            "paid_at": None,
            "final_quantity_mt": None,
            "final_price_per_mt": None,
            "final_total_usd": None,
        }
        values.update(fields)
        return values

    trades = [
        trade("real_real"),
        trade("demo_demo", buyer_id=org["demo_buyer"], seller_id=org["demo_seller"],
              buyer_provenance="DEMO", seller_provenance="DEMO",
              initiator_org_id=org["demo_buyer"], status="CONFIRMED"),
        trade("delivered", status="DELIVERED", delivered_at=clock,
              final_quantity_mt="100.00", final_price_per_mt="600.00", final_total_usd="60000.00"),
        trade("pending", status="PENDING_CONFIRMATION", confirmed_at=None),
        trade("cancelled", status="CANCELLED", confirmed_at=None),
        trade("paid", status="PAID", delivered_at=clock, paid_at=clock,
              final_quantity_mt="100.00", final_price_per_mt="600.00",
              final_total_usd="60000.00"),
        # A mixed-provenance pair, a mismatched or non-canonical snapshot, and
        # a TEST or CANARY pair are all unrepresentable: the snapshot guard,
        # the version-1 domain rule, and the party provenance rule refuse them
        # before any read can see them.
    ]
    for values in trades:
        await _insert(session, INSERT_TRADE, values)

    # Guards are acceptance evidence too: never disable them to create impossible rows.
    for name, statement, values in (
        ("canary_org", INSERT_ORGANIZATION, {"id": fid("org:canary"), "name": "Canary", "verification_status": "APPROVED", "provenance": "CANARY"}),
        ("unregistered_test_org", INSERT_ORGANIZATION, {"id": fid("org:test-invalid"), "name": "Test", "verification_status": "APPROVED", "provenance": "TEST"}),
        ("mixed_trade", INSERT_TRADE, trade("mixed_trade", seller_id=org["demo_seller"], seller_provenance="DEMO")),
        ("legacy_trade_insert", INSERT_TRADE, trade("legacy_trade_insert", market_snapshot_version=0)),
    ):
        try:
            async with session.begin_nested():
                await _insert(session, statement, values)
        except DBAPIError as error:
            cases[name] = {"guard_refused": True, "sqlstate": getattr(error.orig, "sqlstate", None)}
        else:
            raise AssertionError(f"expected database guard refusal: {name}")

    # The attestation is a policy check, not an authorization claim.
    row = (await session.execute(text(ATTESTATION_SQL))).one()
    if row[0] != os.environ.get("RUST_MARKET_EXPECTED_DATABASE"):
        raise RuntimeError("the oracle refused an unattested fixture database")
    if row[1]:
        raise RuntimeError("the oracle refuses a superuser fixture writer")
    return cases


async def select(session) -> dict:
    """Answer every matrix row with the production Python predicate."""
    from sqlalchemy import select as sql_select

    from app.models.orderbook import OrderBookOrder, Trade
    from app.services.market_provenance import public_trade_evidence_clause
    from app.services.public_order_scope import public_order_scope

    async def visible(include_off_spec: bool) -> set[str]:
        filters, joins = public_order_scope(include_off_spec=include_off_spec)
        statement = sql_select(OrderBookOrder.id)
        for target, condition in joins:
            statement = statement.join(target, condition)
        result = await session.execute(statement.where(*filters))
        return {str(value) for value in result.scalars()}

    all_orders = {
        str(value) for value in (
            await session.execute(sql_select(OrderBookOrder.id))
        ).scalars()
    }
    all_trades = {str(value) for value in (
        await session.execute(sql_select(Trade.id))
    ).scalars()}
    trade_selected = {str(value) for value in (
        await session.execute(
            sql_select(Trade.id).where(public_trade_evidence_clause(Trade))
        )
    ).scalars()}
    from app.models.catalog import Product, DeliveryPoint
    from app.services.market_data_eligibility import (
        current_public_order_clause, public_order_collection_provenance_clause,
        active_market_catalog_clauses,
    )
    listings = sql_select(OrderBookOrder.id).join(Product).join(DeliveryPoint).where(
        OrderBookOrder.side == "ASK",
        OrderBookOrder.status.in_(("OPEN", "PARTIALLY_FILLED")),
        current_public_order_clause(OrderBookOrder),
        public_order_collection_provenance_clause(OrderBookOrder),
        *active_market_catalog_clauses(Product, DeliveryPoint),
    ).order_by(OrderBookOrder.created_at.desc(), OrderBookOrder.id)
    listing_ids = [str(value) for value in (await session.execute(listings)).scalars()]
    from app.services.order_terms import executable_order_terms
    visible_ids = await visible(False)
    digest_inputs = {}
    for order_row in (await session.execute(sql_select(OrderBookOrder))).scalars():
        if str(order_row.id) in visible_ids:
            digest_inputs[str(order_row.id)] = executable_order_terms(order_row)
    # Exact product/point filter combinations cannot leak another lane.
    from app.market_catalog import PRODUCT_IDS, CANONICAL_DELIVERY_POINTS
    points = {point.name: point.id for point in CANONICAL_DELIVERY_POINTS}
    lane_filters = []
    for product_id in (PRODUCT_IDS["UCOME_B100"], PRODUCT_IDS["BIO_LNG"]):
        for point_id in (points["Singapore"], points["Rotterdam"]):
            filters, joins = public_order_scope()
            query = sql_select(OrderBookOrder.id)
            for target, condition in joins:
                query = query.join(target, condition)
            result = await session.execute(query.where(*filters,
                OrderBookOrder.product_id == product_id, OrderBookOrder.delivery_point_id == point_id))
            lane_filters.append({"product": str(product_id), "point": str(point_id),
                                 "ids": sorted(str(value) for value in result.scalars())})
    cutoffs = {}
    instant = datetime.fromisoformat(os.environ[CLOCK_ENV])
    for name, kwargs in (("since_equal", {"confirmed_since": instant}),
                         ("before_equal", {"confirmed_before": instant})):
        cutoffs[name] = sorted(str(value) for value in (await session.execute(
            sql_select(Trade.id).where(public_trade_evidence_clause(Trade, **kwargs))
        )).scalars())
    return {
        "lane_filters": lane_filters,
        "digest_inputs": digest_inputs,
        "cutoffs": cutoffs,
        "listings": listing_ids,
        "all_orders": sorted(all_orders),
        "selected": sorted(await visible(False)),
        "selected_with_off_spec": sorted(await visible(True)),
        "all_trades": sorted(all_trades),
        "trade_selected": sorted(trade_selected),
    }


ATTESTATION_SQL = (
    "SELECT current_database(), rolsuper FROM pg_catalog.pg_roles "
    "WHERE rolname = current_user"
)

INSERT_PRODUCT = (
    "INSERT INTO products (id, name, fuel_type, fuel_grade, unit, min_lot_size, is_active) "
    "VALUES (:id, :name, :fuel_type, :fuel_grade, :unit, :min_lot_size, :is_active) "
    "ON CONFLICT (id) DO NOTHING"
)
INSERT_POINT = (
    "INSERT INTO delivery_points (id, name, region, timezone, is_active) "
    "VALUES (:id, :name, :region, :timezone, :is_active) ON CONFLICT (id) DO NOTHING"
)
DEACTIVATE_PRODUCT = "UPDATE products SET is_active = false WHERE id = :id"
DEACTIVATE_POINT = "UPDATE delivery_points SET is_active = false WHERE id = :id"
INSERT_ORGANIZATION = (
    "INSERT INTO organizations (id, name, type, verification_status, provenance) "
    "VALUES (:id, :name, 'FUEL_SUPPLIER', :verification_status, :provenance) "
    "ON CONFLICT (id) DO NOTHING"
)
INSERT_USER = (
    "INSERT INTO users (id, email, password_hash, role, status, organization_id, "
    "email_verified, must_change_password, kyc_status, kyc_organization_id) "
    "VALUES (:id, :email, 'fixture-hash', :role, :status, :organization_id, "
    ":email_verified, :must_change_password, :kyc_status, :kyc_organization_id)"
)
INSERT_GRACE = (
    "INSERT INTO kyc_execution_grace (id, user_id, organization_id, "
    "kyc_status_at_enrollment, enrolled_at, expires_at, created_by) "
    "VALUES (:id, :user_id, :organization_id, 'PENDING', now(), :expires_at, 'fixture')"
)
INSERT_ORDER = (
    "INSERT INTO orderbook_orders (id, organization_id, owner_user_id, "
    "created_by_actor_user_id, creation_method, provenance, side, product_id, "
    "delivery_point_id, quantity_mt, remaining_quantity_mt, price_per_mt_usd, "
    "availability_window, status, expires_at, off_spec, certification_declared, "
    "certification_scheme, specification_standard, msds_available, "
    "carbon_intensity_gco2_mj, feedstock, origin, certifications, fame_terms, lng_terms) "
    "VALUES (:id, :organization_id, :owner_user_id, :created_by_actor_user_id, "
    ":creation_method, :provenance, :side, :product_id, :delivery_point_id, :quantity_mt, "
    ":remaining_quantity_mt, :price_per_mt_usd, :availability_window, :status, :expires_at, "
    ":off_spec, :certification_declared, :certification_scheme, :specification_standard, "
    ":msds_available, :carbon_intensity_gco2_mj, :feedstock, :origin, "
    "CAST(:certifications AS json), CAST(:fame_terms AS json), CAST(:lng_terms AS json))"
)
INSERT_TRADE = (
    "INSERT INTO trades (id, product_id, product_name, fuel_type, fuel_grade, "
    "market_product, delivery_point_id, delivery_point_name, delivery_point_region, "
    "availability_window, buyer_id, seller_id, buyer_provenance, seller_provenance, "
    "initiated_by, initiator_org_id, quantity_mt, price_per_mt_usd, status, "
    "confirmed_at, market_snapshot_version, delivered_at, paid_at, "
    "final_quantity_mt, final_price_per_mt, final_total_usd) "
    "VALUES (:id, :product_id, :product_name, :fuel_type, :fuel_grade, :market_product, "
    ":delivery_point_id, :delivery_point_name, :delivery_point_region, "
    ":availability_window, :buyer_id, :seller_id, :buyer_provenance, :seller_provenance, "
    ":initiated_by, :initiator_org_id, :quantity_mt, :price_per_mt_usd, :status, "
    ":confirmed_at, :market_snapshot_version, :delivered_at, :paid_at, "
    ":final_quantity_mt, :final_price_per_mt, :final_total_usd)"
)


async def _main() -> None:
    from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

    url = os.environ["RUST_MARKET_WRITER_URL"]
    output = Path(os.environ["RUST_MARKET_TEST_EXPECTED"])
    instant = datetime.fromisoformat(os.environ[CLOCK_ENV])
    if instant.tzinfo is None:
        raise RuntimeError(f"{CLOCK_ENV} must be timezone-aware")
    instant = instant.astimezone(timezone.utc)

    from zoneinfo import ZoneInfo

    singapore_date = instant.astimezone(ZoneInfo("Asia/Singapore")).date().isoformat()

    from tests.rust_contract.clock import fixed_clock_patches

    stack = fixed_clock_patches(instant)
    engine = create_async_engine(url)
    sessionmaker = async_sessionmaker(engine, expire_on_commit=False)
    try:
        async with sessionmaker() as session:
            cases = await seed(session, instant)
            await session.commit()
        async with sessionmaker() as session:
            selected = await select(session)
            for name, case in cases.items():
                if "id" in case:
                    wanted = name in {"grace_after", "typed_invalid_visible"}
                    assert (case["id"] in selected["selected"]) == wanted, name
                    case["visible"] = wanted
    finally:
        stack.close()
    payload = {
        "cases": cases,
        "clock": instant.isoformat(),
        "singapore_date": singapore_date,
        **selected,
    }
    output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(f"oracle selected {len(payload['selected'])} of {len(payload['all_orders'])} orders "
          f"and {len(payload['trade_selected'])} of {len(payload['all_trades'])} trades")


def main(argv: list[str] | None = None) -> int:
    argparse.ArgumentParser(description=__doc__).parse_args(argv)
    try:
        asyncio.run(_main())
    except (KeyError, RuntimeError) as error:
        print(f"market contract oracle failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
