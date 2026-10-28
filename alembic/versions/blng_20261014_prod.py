"""Add the Bio-LNG market product, its Rotterdam lane, and lng_terms storage.

Revision ID: blng_20261014_prod
Revises: uadl_20261003_delivery_reports
Create Date: 2026-10-14

Contract: docs/plans/2026-10-14-bio-lng-design.md. All identities and SQL below
are frozen migration-local literals. Orders store side-specific ``lng_terms``;
trades store an immutable ``lng_terms_snapshot`` of both sides. The snapshot
trigger keeps the prior provenance, delivery, and immutability rules and adds
the Bio-LNG identity and snapshot immutability. The app role keeps its
existing table grants, so the trigger, not the ACL policy, enforces immutability.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "blng_20261014_prod"
down_revision = "uadl_20261003_delivery_reports"
branch_labels = None
depends_on = None

_BIO_LNG_ID = "8f6f34a5-2927-54a6-8564-cca40ae7cb8a"
_ROTTERDAM_ID = "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"
_BIO_LNG_IDENTITY = (_BIO_LNG_ID, "Bio-LNG", "LNG", "Bio", "BIO_LNG")
_BIO_LNG_DESCRIPTION = (
    "Liquefied biomethane (bio-LNG) for marine fuel use, delivered to "
    "ISO 23306:2020 with a certified Proof of Sustainability."
)
_PRIOR_MARKET_PRODUCTS = (
    "'BIO_METHANOL','E_METHANOL','BIO_ETHANOL','SYNTHETIC_ETHANOL','B30','B100'"
)
_SIGNAL_TABLES = ("market_indications", "fair_price_bands", "physical_stems")

# Terms are required on Bio-LNG rows at Rotterdam and forbidden on every other row.
_ORDER_LNG_TERMS = (
    f"(product_id IS DISTINCT FROM '{_BIO_LNG_ID}' AND lng_terms IS NULL) OR "
    f"(product_id = '{_BIO_LNG_ID}' AND delivery_point_id = '{_ROTTERDAM_ID}' "
    "AND lng_terms IS NOT NULL AND (json_typeof(lng_terms) = 'object' "
    "AND lng_terms->>'side' = side)) IS TRUE"
)
_TRADE_LNG_TERMS = (
    f"(product_id IS DISTINCT FROM '{_BIO_LNG_ID}' AND lng_terms_snapshot IS NULL) OR "
    f"(product_id = '{_BIO_LNG_ID}' AND delivery_point_id = '{_ROTTERDAM_ID}' "
    "AND lng_terms_snapshot IS NOT NULL AND ("
    "json_typeof(lng_terms_snapshot->'bid') = 'object' "
    "AND lng_terms_snapshot->'bid'->>'side' = 'BID' "
    "AND json_typeof(lng_terms_snapshot->'ask') = 'object' "
    "AND lng_terms_snapshot->'ask'->>'side' = 'ASK')) IS TRUE"
)
_TERM_COLUMNS = (
    ("orderbook_orders", "lng_terms", "ck_orderbook_orders_lng_terms", _ORDER_LNG_TERMS),
    ("trades", "lng_terms_snapshot", "ck_trades_lng_terms", _TRADE_LNG_TERMS),
)

# Include CASCADE foreign keys and code-only references before removing the
# product. Historical audit and browsing records are kept unchanged.
_BIO_LNG_REFERENCES = (
    ("orderbook_orders", "product_id = :product_id OR lng_terms IS NOT NULL"),
    (
        "trades",
        "product_id = :product_id OR market_product = 'BIO_LNG' "
        "OR lng_terms_snapshot IS NOT NULL",
    ),
    ("rfqs", "product_id = :product_id"),
    ("negotiations", "product_id = :product_id"),
    ("contracts", "product_id = :product_id"),
    ("supply_listings", "product_id = :product_id"),
    # Inventory publishing strips whitespace and resolves the name independently of fuel_type.
    ("inventory_items", "trim(product_name, ' \t\r\n\f\v') = 'Bio-LNG'"),
    ("market_support_authorizations", "product_id = :product_id"),
    ("price_alerts", "product_id = :product_id"),
    ("watchlist_entries", "product_id = :product_id"),
    ("watchlist_targets", "market_product_code = 'BIO_LNG'"),
    ("benchmarks", "market_product = 'BIO_LNG'"),
    ("live_slice_benchmarks", "market_product = 'BIO_LNG'"),
    ("market_indications", "market_product = 'BIO_LNG'"),
    ("fair_price_bands", "market_product = 'BIO_LNG'"),
    ("physical_stems", "market_product = 'BIO_LNG'"),
)

_DEMO_IDS = (
    "4da7b285-34ee-5443-9406-f96b4ed1a251",
    "0dbce576-2026-5925-ab66-674d505e98ad",
    "3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a",
    "277491df-cb0d-5f2d-a2cf-5746829c6da6",
    "3b302066-d65c-5c3e-8fcc-70b3da3bcafd",
    "79609f48-0a3e-560e-a1e1-63d90601d84a",
    "2c4e387e-de22-5adb-ad88-9274ba84ebe1",
    "612953c7-567a-58b3-bc42-ee817d2bbe74",
    "93ccda09-54b3-53ee-afc0-759d3048161f",
    "82426590-0963-5486-9b05-f81e97afe6ef",
    "acc3f20a-fe94-4463-9029-a55e35634eb7",
    "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
    "7cc77115-0a9f-4ec4-8c74-05aa10050111",
    "d1e43e55-3fb0-4b5e-9f0b-93aa10050222",
)

_PRIOR_PRODUCT_IDENTITIES = (
    ("9510c713-6e39-5080-add3-0a7c29657b79", "Bio Methanol", "Methanol", "Bio", "BIO_METHANOL"),
    ("f9b20492-b445-59cd-b292-a386d913f488", "e-Methanol", "Methanol", "E", "E_METHANOL"),
    ("c4a688be-f7c2-5edc-8f93-6b34e387609c", "Bio Ethanol", "Ethanol", "Bio", "BIO_ETHANOL"),
    ("d186bffb-766d-5944-8825-989abbdcfc46", "Synthetic Ethanol", "Ethanol", "Synthetic", "SYNTHETIC_ETHANOL"),
    ("c4cecebc-3d2d-5840-8021-57a9a11bc673", "B30", "Biofuel", "B30", "B30"),
    ("74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5", "B100", "Biofuel", "B100", "B100"),
)
_DELIVERY_POINT_IDENTITIES = (
    ("78281fb3-e726-5396-802e-34b01fda21e8", "Dalian", "Asia"),
    ("262d36ae-6f35-5785-b9e8-9e221b0f1b78", "Busan", "Asia"),
    ("633c0593-f9b9-52ad-abe5-a0285f6888d3", "Shanghai", "Asia"),
    ("73835e92-820e-584b-8280-bb61c63aa28e", "Singapore", "Asia"),
    ("1379d36c-1ca9-55b7-9c0d-5235a0ba1f36", "Rotterdam", "Europe"),
    ("a083db06-b050-56c2-a274-3897eac2fdae", "Houston", "Americas"),
    ("1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534", "Los Angeles", "Americas"),
    ("a4762444-4e0b-5bc1-b647-a185047adae5", "Santos", "Americas"),
)

_TRADE_SNAPSHOT = (
    "(market_snapshot_version IS NULL OR (market_snapshot_version = 1 "
    "AND product_id IS NOT NULL AND product_name IS NOT NULL AND trim(product_name) <> '' "
    "AND fuel_type IS NOT NULL AND trim(fuel_type) <> '' "
    "AND fuel_grade IS NOT NULL AND trim(fuel_grade) <> '' "
    "AND market_product IN ({market_products}) "
    "AND availability_window IS NOT NULL AND (availability_window = 'SPOT' "
    "OR availability_window ~ '^[0-9]{{4}}-(0[1-9]|1[0-2])$' "
    "OR availability_window ~ '^[0-9]{{4}}-Q[1-4]$' "
    "OR availability_window ~ '^[0-9]{{4}}-CAL$') "
    "AND delivery_point_id IS NOT NULL AND delivery_point_name IS NOT NULL "
    "AND delivery_point_region IS NOT NULL))"
)


def _quoted(values: tuple[str, ...]) -> str:
    return ",".join(f"'{value}'" for value in values)


def _literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _market_products(*, include_bio_lng: bool) -> str:
    return _PRIOR_MARKET_PRODUCTS + (",'BIO_LNG'" if include_bio_lng else "")


def _product_row_case(alias: str, *, include_bio_lng: bool) -> str:
    identities = _PRIOR_PRODUCT_IDENTITIES + ((_BIO_LNG_IDENTITY,) if include_bio_lng else ())
    branches = " ".join(
        "WHEN " + " AND ".join(
            (
                f"{alias}.id = {_literal(product_id)}",
                f"{alias}.name = {_literal(name)}",
                f"{alias}.fuel_type = {_literal(fuel_type)}",
                f"{alias}.fuel_grade = {_literal(fuel_grade)}",
            )
        ) + f" THEN {_literal(market_product)}"
        for product_id, name, fuel_type, fuel_grade, market_product in identities
    )
    return f"CASE {branches} ELSE NULL END"


def _delivery_row_predicate(alias: str) -> str:
    return "(" + " OR ".join(
        "(" + " AND ".join(
            (
                f"{alias}.id = {_literal(point_id)}",
                f"{alias}.name = {_literal(name)}",
                f"{alias}.region = {_literal(region)}",
            )
        ) + ")"
        for point_id, name, region in _DELIVERY_POINT_IDENTITIES
    ) + ")"


def _replace_checks(*, include_bio_lng: bool) -> None:
    market_products = _market_products(include_bio_lng=include_bio_lng)
    op.drop_constraint("ck_trades_snapshot", "trades", type_="check")
    op.create_check_constraint(
        "ck_trades_snapshot", "trades",
        _TRADE_SNAPSHOT.format(market_products=market_products),
    )
    for table in _SIGNAL_TABLES:
        name = f"ck_{table}_market_product"
        op.drop_constraint(name, table, type_="check")
        op.create_check_constraint(name, table, f"market_product IN ({market_products})")


def _replace_snapshot_function(*, include_bio_lng: bool) -> None:
    # With include_bio_lng=False this renders the parent revision's function
    # byte for byte, so a downgrade restores the exact prior guard.
    snapshot_changed = (
        " OR OLD.lng_terms_snapshot::jsonb IS DISTINCT FROM NEW.lng_terms_snapshot::jsonb"
        if include_bio_lng else ""
    )
    op.execute(sa.text(f"""
            CREATE OR REPLACE FUNCTION verdaxis_validate_trade_snapshot()
            RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE current_buyer_provenance text;
            DECLARE current_seller_provenance text;
            DECLARE product_row products%ROWTYPE;
            DECLARE delivery_point_row delivery_points%ROWTYPE;
            DECLARE expected_market_product text;
            BEGIN
                IF TG_OP = 'INSERT' THEN
                    SELECT provenance INTO STRICT current_buyer_provenance
                    FROM organizations WHERE id = NEW.buyer_id;
                    SELECT provenance INTO STRICT current_seller_provenance
                    FROM organizations WHERE id = NEW.seller_id;
                    IF NEW.buyer_provenance IS DISTINCT FROM current_buyer_provenance
                       OR NEW.seller_provenance IS DISTINCT FROM current_seller_provenance THEN
                        RAISE EXCEPTION 'trade provenance snapshots must equal current party provenance';
                    END IF;
                    IF NEW.market_snapshot_version IS DISTINCT FROM 1 THEN
                        RAISE EXCEPTION 'new trades require immutable market snapshot version 1';
                    END IF;
                    SELECT * INTO STRICT product_row
                    FROM products WHERE id = NEW.product_id;
                    expected_market_product := {_product_row_case("product_row", include_bio_lng=include_bio_lng)};
                    IF product_row.is_active IS DISTINCT FROM true
                       OR expected_market_product IS NULL THEN
                        RAISE EXCEPTION 'trade product snapshot must reference an active canonical product';
                    END IF;
                    IF ROW(
                        NEW.product_name, NEW.fuel_type, NEW.fuel_grade,
                        NEW.market_product
                    ) IS DISTINCT FROM ROW(
                        product_row.name, product_row.fuel_type,
                        product_row.fuel_grade, expected_market_product
                    ) THEN
                        RAISE EXCEPTION 'trade product snapshot labels do not match referenced product';
                    END IF;
                    SELECT * INTO STRICT delivery_point_row
                    FROM delivery_points WHERE id = NEW.delivery_point_id;
                    IF delivery_point_row.is_active IS DISTINCT FROM true
                       OR NOT {_delivery_row_predicate("delivery_point_row")} THEN
                        RAISE EXCEPTION 'trade delivery snapshot must reference an active canonical delivery point';
                    END IF;
                    IF ROW(
                        NEW.delivery_point_name, NEW.delivery_point_region
                    ) IS DISTINCT FROM ROW(
                        delivery_point_row.name, delivery_point_row.region
                    ) THEN
                        RAISE EXCEPTION 'trade delivery snapshot labels do not match referenced delivery point';
                    END IF;
                    IF NEW.availability_window IS NULL OR NOT (
                        NEW.availability_window = 'SPOT'
                        OR NEW.availability_window ~ '^[0-9]{{4}}-(0[1-9]|1[0-2])$'
                        OR NEW.availability_window ~ '^[0-9]{{4}}-Q[1-4]$'
                        OR NEW.availability_window ~ '^[0-9]{{4}}-CAL$'
                    ) THEN
                        RAISE EXCEPTION 'trade snapshot requires a canonical availability window';
                    END IF;
                    IF NOT (
                        (NEW.buyer_provenance = 'REAL' AND NEW.seller_provenance = 'REAL')
                        OR (NEW.buyer_provenance = 'DEMO' AND NEW.seller_provenance = 'DEMO'
                            AND NEW.buyer_id IN ({_quoted(_DEMO_IDS)})
                            AND NEW.seller_id IN ({_quoted(_DEMO_IDS)}))
                    ) THEN
                        RAISE EXCEPTION 'trade execution requires REAL-REAL or an allowlisted DEMO-DEMO pair';
                    END IF;
                ELSE
                    IF ROW(
                        OLD.bid_order_id, OLD.ask_order_id,
                        OLD.buyer_id, OLD.seller_id, OLD.initiator_org_id,
                        OLD.buyer_provenance, OLD.seller_provenance,
                        OLD.product_id, OLD.product_name, OLD.fuel_type, OLD.fuel_grade,
                        OLD.market_product, OLD.delivery_point_id, OLD.delivery_point_name,
                        OLD.delivery_point_region, OLD.availability_window, OLD.market_snapshot_version
                    ) IS DISTINCT FROM ROW(
                        NEW.bid_order_id, NEW.ask_order_id,
                        NEW.buyer_id, NEW.seller_id, NEW.initiator_org_id,
                        NEW.buyer_provenance, NEW.seller_provenance,
                        NEW.product_id, NEW.product_name, NEW.fuel_type, NEW.fuel_grade,
                        NEW.market_product, NEW.delivery_point_id, NEW.delivery_point_name,
                        NEW.delivery_point_region, NEW.availability_window, NEW.market_snapshot_version
                    ){snapshot_changed} THEN
                        RAISE EXCEPTION 'trade parties, provenance, and market snapshots are immutable';
                    END IF;
                END IF;
                RETURN NEW;
            END;
            $$
    """))


def _insert_product() -> None:
    connection = op.get_bind()
    existing = connection.execute(
        sa.text("SELECT id FROM products WHERE id = :product_id OR name = 'Bio-LNG'"),
        {"product_id": _BIO_LNG_ID},
    ).first()
    if existing is not None:
        raise RuntimeError("a Bio-LNG product identity already exists; refusing to overwrite it")
    connection.execute(
        sa.text("""
            INSERT INTO products (
                id, name, fuel_type, fuel_grade, unit, min_lot_size,
                spec_description, is_active
            ) VALUES (
                :product_id, 'Bio-LNG', 'LNG', 'Bio', 'MT', 1, :description, true
            )
        """),
        {"product_id": _BIO_LNG_ID, "description": _BIO_LNG_DESCRIPTION},
    )


def upgrade() -> None:
    _insert_product()
    for table, column, check_name, expression in _TERM_COLUMNS:
        op.add_column(table, sa.Column(column, sa.JSON(), nullable=True))
        op.create_check_constraint(check_name, table, expression)
    _replace_checks(include_bio_lng=True)
    _replace_snapshot_function(include_bio_lng=True)


def downgrade() -> None:
    connection = op.get_bind()
    # Lock first and never wait between tables: runtime market paths use
    # different lock orders. A busy market must abort this downgrade safely.
    tables = ", ".join(("products", *(table for table, _ in _BIO_LNG_REFERENCES)))
    connection.execute(sa.text(f"LOCK TABLE {tables} IN ACCESS EXCLUSIVE MODE NOWAIT"))
    for table, predicate in _BIO_LNG_REFERENCES:
        in_use = connection.execute(
            sa.text(f"SELECT EXISTS (SELECT 1 FROM {table} WHERE {predicate})"),
            {"product_id": _BIO_LNG_ID},
        ).scalar_one()
        if in_use:
            raise RuntimeError(f"cannot downgrade Bio-LNG while {table} references it")

    _replace_snapshot_function(include_bio_lng=False)
    _replace_checks(include_bio_lng=False)
    for table, column, check_name, _expression in reversed(_TERM_COLUMNS):
        op.drop_constraint(check_name, table, type_="check")
        op.drop_column(table, column)
    connection.execute(
        sa.text("DELETE FROM products WHERE id = :product_id"),
        {"product_id": _BIO_LNG_ID},
    )
