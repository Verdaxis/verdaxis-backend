"""Populate the fixed SQL market matrix without application imports."""

import json
import os
from datetime import datetime, timedelta
from uuid import UUID, uuid5, NAMESPACE_URL

from disposable_postgres import bind_sql, psql, rows


def fid(name):
    return uuid5(NAMESPACE_URL, f"rust-market-contract:{name}")


def guarded_insert_sql(statement, values, require_refusal=False):
    # Some fixture rows can be refused by database guards. Retain any rows that
    # the schema admits, as the SQL visibility tests also cover those states.
    statement = bind_sql(statement, values)
    on_success = "RAISE EXCEPTION 'expected fixture guard refusal'; " if require_refusal else ""
    return ("DO $fixture$ BEGIN BEGIN " + statement +
            "; EXCEPTION WHEN integrity_constraint_violation OR raise_exception THEN RETURN; END; " +
            on_success + "END; $fixture$;")


def seed(url, clock):
    """Insert the fixture matrix. Every row is explicit; nothing is inferred."""
    identity = rows(url, "SELECT current_database() AS database_name, rolsuper FROM pg_roles WHERE rolname=current_user")[0]
    if identity["database_name"] != os.environ["RUST_MARKET_EXPECTED_DATABASE"] or identity["rolsuper"]:
        raise ValueError("the market fixture requires its named database and a non-superuser writer")
    product = {row["name"]: UUID(row["id"]) for row in rows(url, "SELECT id, name FROM products")}
    point = {row["name"]: UUID(row["id"]) for row in rows(url, "SELECT id, name FROM delivery_points")}
    statements = []

    def insert(statement, values):
        statements.append(bind_sql(statement, values) + ";")

    # The inactive cases: one canonical product and one canonical point are
    # still fully canonical by identity but carry an inactive row.
    insert(DEACTIVATE_PRODUCT, {"id": product["Synthetic Ethanol"]})
    insert(DEACTIVATE_POINT, {"id": point["Santos"]})

    # Non-canonical decoys: an alias product and an alias delivery point.
    alias_product = fid("product:alias")
    insert(INSERT_PRODUCT, {
        "id": alias_product, "name": "Methanol Green", "fuel_type": "Methanol",
        "fuel_grade": "Green", "unit": "MT", "min_lot_size": 100, "is_active": True,
    })
    alias_point = fid("point:alias")
    insert(INSERT_POINT, {
        "id": alias_point, "name": "Rotterdam II", "region": "Europe",
        "timezone": "Europe/Amsterdam", "is_active": True,
    })

    # The organizations trigger admits DEMO and TEST provenance only for the
    # deterministic seed identities, and refuses CANARY outright. A CANARY
    # market row is therefore unrepresentable in a freshly migrated database
    # and is not part of this matrix.
    test_ids = [UUID(f"9e63f7a1-0000-4000-8000-{number:012d}") for number in (1, 2)]
    organizations = {
        "real": ("REAL", "APPROVED"),
        "real_alt": ("REAL", "APPROVED"),
        "real_admin_only": ("REAL", "APPROVED"),
        "advisory_customer": ("REAL", "APPROVED"),
        "foreign_customer": ("REAL", "APPROVED"),
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
            "demo_buyer": UUID("4da7b285-34ee-5443-9406-f96b4ed1a251"),
            "demo_seller": UUID("79609f48-0a3e-560e-a1e1-63d90601d84a"),
            "test_a": test_ids[0],
            "test_b": test_ids[1],
        }.get(key)
        identifier = fixed or fid(f"org:{key}")
        org[key] = identifier
        insert(INSERT_ORGANIZATION, {
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
        "pending_no_grace": ("real", "BUYER", "APPROVED", True, False, "PENDING", None),
        "submitted_no_grace": ("real", "BUYER", "APPROVED", True, False, "SUBMITTED", "real"),
        "foreign_review_grace": ("real", "BUYER", "APPROVED", True, False, "APPROVED", "real_alt"),
        "advisory_customer": ("advisory_customer", "SUPPLIER", "APPROVED", True, False, "SUBMITTED", "advisory_customer"),
        "foreign_customer": ("foreign_customer", "SUPPLIER", "APPROVED", True, False, "APPROVED", "real_alt"),
    })
    user = {}
    for key, (org_key, role, status, verified, must_change, kyc, kyc_org) in users.items():
        identifier = fid(f"user:{key}")
        user[key] = identifier
        insert(INSERT_USER, {
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
                       ("grace_after", timedelta(microseconds=1)),
                       ("foreign_review_grace", timedelta(hours=1)),
                       ("foreign_customer", timedelta(hours=1))):
        insert(INSERT_GRACE, {
            "id": fid(f"grace:{key}"),
            "user_id": user[key],
            "organization_id": org["real_alt"] if key == "wrong_grace_org" else org["foreign_customer"] if key == "foreign_customer" else org["real"],
            "expires_at": clock + delta,
        })

    methanol = product["Bio Methanol"]
    fame = product["UCOME B100"]
    lng = product["Bio-LNG"]
    b30 = product["B30"]
    b100 = product["B100"]
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
            "carbon_intensity_method": None,
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
    # Fixed-biofuel ASK declarations: the exact B30/B100 specification
    # standards plus the whole-supplied-fuel metadata the Phase 0 clause
    # requires. biofuel_ask_* rows below use these; the negative rows drop
    # one field at a time to exercise every side of the clause.
    biofuel_declarations = {
        **scalar_declarations,
        "specification_standard": "ISO 8217:2024 RF 380",
        "carbon_intensity_method": "Whole-blend CI and LCV",
    }
    biofuel100_declarations = {
        **scalar_declarations,
        "specification_standard": "ISO 8217:2024 DFA",
        "carbon_intensity_method": "Whole-fuel FAME CI and LCV",
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

    fame_terms.update({key: None for key in (
        "astm_grade", "en_climate_class", "cfpp_c", "cloud_point_c", "ci_methodology",
        "ci_boundary", "ci_basis", "certificate_scope", "lhv_mj_kg", "quality_evidence",
        "sustainability_evidence", "batch_reference", "producing_site", "production_origin",
        "feedstock_origin", "shipping_location", "ci_gco2e_mj")})
    fame_terms["nomination_status"] = "PENDING"
    lng_terms.update(feedstock_detail=None, min_methane_number=None)

    future = clock + timedelta(hours=24)
    past = clock - timedelta(hours=24)
    rows_to_insert = [
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
        # Fixed-biofuel contract rows. The positive rows carry the exact
        # policy standard and all supplier metadata; each negative row breaks
        # one clause arm so the mirror is proven on both sides.
        order("b30_ask_ok", side="ASK", product_id=b30, **biofuel_declarations),
        order("b100_ask_ok", side="ASK", product_id=b100, **biofuel100_declarations),
        order("b30_ask_wrong_standard", side="ASK", product_id=b30,
              **{**biofuel_declarations, "specification_standard": "EN 14214"}),
        order("b30_ask_missing_ci_method", side="ASK", product_id=b30,
              **{k: v for k, v in biofuel_declarations.items() if k != "carbon_intensity_method"}),
        order("b100_ask_missing_feedstock", side="ASK", product_id=b100,
              **{k: v for k, v in biofuel100_declarations.items() if k != "feedstock"}),
        order("b30_bid_ok", product_id=b30),
        order("b100_bid_ok", product_id=b100),
        order("partial_bid", status="PARTIALLY_FILLED", remaining_quantity_mt="40.00"),
        order("filled_bid", status="FILLED", remaining_quantity_mt="0.00"),
        order("demo_expired_bid", provenance="DEMO", organization_id=org["demo_buyer"],
              owner_user_id=None, expires_at=past),
        order("real_expired_bid", expires_at=past),
    ]
    additions = {
        name: order(name, owner_user_id=user[name])
        for name in ("rejected_with_grace", "wrong_kyc_org", "wrong_grace_org", "grace_before", "grace_after",
                     "pending_no_grace", "submitted_no_grace", "foreign_review_grace")
    }
    additions.update({
        "org_revoked": order("org_revoked", organization_id=org["pending"], owner_user_id=user["org_revoked"]),
        "assisted_rejected": order("assisted_rejected", creation_method="MARKET_SUPPORT",
            owner_user_id=user["admin_rejected"], created_by_actor_user_id=user["admin_rejected"]),
        "assisted_mismatched": order("assisted_mismatched", creation_method="MARKET_SUPPORT",
            owner_user_id=user["admin"], created_by_actor_user_id=user["admin_only"]),
        "admin_kyc_only": order("admin_kyc_only", organization_id=org["real_admin_only"],
            creation_method="MARKET_SUPPORT", owner_user_id=user["admin_only"], created_by_actor_user_id=user["admin_only"]),
        "assisted_advisory": order("assisted_advisory", organization_id=org["advisory_customer"],
            creation_method="MARKET_SUPPORT", owner_user_id=user["admin"], created_by_actor_user_id=user["admin"]),
        "assisted_foreign_grace": order("assisted_foreign_grace", organization_id=org["foreign_customer"],
            creation_method="MARKET_SUPPORT", owner_user_id=user["admin"], created_by_actor_user_id=user["admin"]),
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
    for values in rows_to_insert:
        insert(INSERT_ORDER, values)
    refused = {"fame_wrong_lane", "lng_wrong_lane", "demo_null_expiry", "zero_remaining", "assisted_mismatched"}
    for name, values in additions.items():
        if name in refused:
            statements.append(guarded_insert_sql(INSERT_ORDER, values))
        else:
            insert(INSERT_ORDER, values)
    insert("UPDATE users SET kyc_status='APPROVED', kyc_organization_id=organization_id WHERE id=:id",
           {"id": user["admin_only"]})

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
        insert(INSERT_TRADE, values)

    # Guards are acceptance evidence too: never disable them to create impossible rows.
    for name, statement, values in (
        ("canary_org", INSERT_ORGANIZATION, {"id": fid("org:canary"), "name": "Canary", "verification_status": "APPROVED", "provenance": "CANARY"}),
        ("unregistered_test_org", INSERT_ORGANIZATION, {"id": fid("org:test-invalid"), "name": "Test", "verification_status": "APPROVED", "provenance": "TEST"}),
        ("mixed_trade", INSERT_TRADE, trade("mixed_trade", seller_id=org["demo_seller"], seller_provenance="DEMO")),
        ("legacy_trade_insert", INSERT_TRADE, trade("legacy_trade_insert", market_snapshot_version=0)),
    ):
        statements.append(guarded_insert_sql(statement, values, require_refusal=True))
    psql(url, "BEGIN;\n" + "\n".join(statements) + "\nCOMMIT;")


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
    "carbon_intensity_gco2_mj, carbon_intensity_method, feedstock, origin, "
    "certifications, fame_terms, lng_terms) "
    "VALUES (:id, :organization_id, :owner_user_id, :created_by_actor_user_id, "
    ":creation_method, :provenance, :side, :product_id, :delivery_point_id, :quantity_mt, "
    ":remaining_quantity_mt, :price_per_mt_usd, :availability_window, :status, :expires_at, "
    ":off_spec, :certification_declared, :certification_scheme, :specification_standard, "
    ":msds_available, :carbon_intensity_gco2_mj, :carbon_intensity_method, :feedstock, :origin, "
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


if __name__ == "__main__":
    instant = datetime.fromisoformat(os.environ["RUST_MARKET_TEST_CLOCK"])
    if instant.tzinfo is None:
        raise ValueError("the market test clock must have a time zone")
    seed(os.environ["RUST_MARKET_WRITER_URL"], instant)
