"""Enable guarded UCOME B100 orders and immutable execution snapshots.

Revision ID: fame_20260922_b100_orderbook
Revises: fame_20260922_supplier_offers

Existing RFQs and supplier indications remain unchanged. Catalog identities,
contract guards and trigger definitions are frozen here, independent of app code.
"""
from alembic import op
import sqlalchemy as sa

revision = "fame_20260922_b100_orderbook"
down_revision = "fame_20260922_supplier_offers"
branch_labels = None
depends_on = None

_PRODUCT_ID = "e561e43f-d9b2-598e-981c-f1d28d515ddc"
_SINGAPORE_ID = "73835e92-820e-584b-8280-bb61c63aa28e"


def _fame_terms_object(column: str, side: str) -> str:
    """Require a versioned side-specific JSON object, rejecting absent keys."""
    return (
        f"(json_typeof({column}) = 'object' "
        f"AND ({column}->'schema_version')::jsonb = '1'::jsonb "
        f"AND {column}->>'side' = {side}) IS TRUE"
    )


def _fame_pair_snapshot(column: str) -> str:
    bid_shape = _fame_terms_object(f"({column}->'bid')", "'BID'")
    ask_shape = _fame_terms_object(f"({column}->'ask')", "'ASK'")
    return (
        f"(json_typeof({column}) = 'object' "
        f"AND ({column}->'schema_version')::jsonb = '1'::jsonb "
        f"AND ({bid_shape}) AND ({ask_shape})) IS TRUE"
    )


def _fame_product_terms(column: str, shape: str, *, minimum_quantity: str = "1") -> str:
    return (
        f"(product_id IS DISTINCT FROM '{_PRODUCT_ID}' AND {column} IS NULL) OR "
        f"(product_id = '{_PRODUCT_ID}' "
        f"AND delivery_point_id = '{_SINGAPORE_ID}' "
        f"AND delivery_point_id IS NOT NULL AND quantity_mt >= {minimum_quantity} "
        f"AND {column} IS NOT NULL AND ({shape})) IS TRUE"
    )

ORDER_FAME_TERMS = _fame_product_terms("fame_terms", _fame_terms_object("fame_terms", "side"))

# A standing order has a 1 MT input minimum; partial fills use the shared
# 0.01 MT execution precision, including the final remainder.
TRADE_FAME_TERMS = _fame_product_terms(
    "fame_terms_snapshot", _fame_pair_snapshot("fame_terms_snapshot"), minimum_quantity="0.01",
)

NEGOTIATION_FAME_TERMS = TRADE_FAME_TERMS

MARKET_SUPPORT_FAME_TERMS = _fame_product_terms("fame_terms", _fame_terms_object("fame_terms", "order_side"))

INVENTORY_FAME_TERMS = (
    "(product_name IS DISTINCT FROM 'UCOME B100' AND fame_terms IS NULL) OR "
    "(product_name = 'UCOME B100' AND fuel_type = 'FAME' AND fame_terms IS NOT NULL "
    "AND (" + _fame_terms_object("fame_terms", "'ASK'") + ")) IS TRUE"
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

_PRODUCT_IDENTITIES = (
    ("9510c713-6e39-5080-add3-0a7c29657b79", "Bio Methanol", "Methanol", "Bio", "BIO_METHANOL"),
    ("f9b20492-b445-59cd-b292-a386d913f488", "e-Methanol", "Methanol", "E", "E_METHANOL"),
    ("c4a688be-f7c2-5edc-8f93-6b34e387609c", "Bio Ethanol", "Ethanol", "Bio", "BIO_ETHANOL"),
    ("d186bffb-766d-5944-8825-989abbdcfc46", "Synthetic Ethanol", "Ethanol", "Synthetic", "SYNTHETIC_ETHANOL"),
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


def _quoted(values: tuple[str, ...]) -> str:
    return ",".join(f"'{value}'" for value in values)


def _literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _product_row_case(alias: str, include_fame: bool) -> str:
    branches = " ".join(
        "WHEN " + " AND ".join(
            (
                f"{alias}.id = {_literal(product_id)}",
                f"{alias}.name = {_literal(name)}",
                f"{alias}.fuel_type = {_literal(fuel_type)}",
                f"{alias}.fuel_grade = {_literal(fuel_grade)}",
            )
        ) + f" THEN {_literal(market_product)}"
        for product_id, name, fuel_type, fuel_grade, market_product in (
            _PRODUCT_IDENTITIES + ((_PRODUCT_ID, "UCOME B100", "FAME", "UCOME", "UCOME_B100"),)
            if include_fame else _PRODUCT_IDENTITIES
        )
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


def _trade_guard_sql(include_fame: bool) -> str:
    snapshot_changed = (
        " OR OLD.fame_terms_snapshot::jsonb IS DISTINCT FROM NEW.fame_terms_snapshot::jsonb"
        if include_fame else ""
    )
    return f"""
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
                    expected_market_product := {_product_row_case("product_row", include_fame)};
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
        """

_OLD_TRADE_SNAPSHOT = (
    '(market_snapshot_version IS NULL OR (market_snapshot_version = 1 AND product_id IS NOT NULL AND '
    "product_name IS NOT NULL AND trim(product_name) <> '' AND fuel_type IS NOT NULL AND "
    "trim(fuel_type) <> '' AND fuel_grade IS NOT NULL AND trim(fuel_grade) <> '' AND market_product "
    "IN ('BIO_METHANOL','E_METHANOL','BIO_ETHANOL','SYNTHETIC_ETHANOL') AND availability_window IS "
    "NOT NULL AND (availability_window = 'SPOT' OR availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$' "
    "OR availability_window ~ '^[0-9]{4}-Q[1-4]$' OR availability_window ~ '^[0-9]{4}-CAL$') AND "
    'delivery_point_id IS NOT NULL AND delivery_point_name IS NOT NULL AND delivery_point_region IS '
    'NOT NULL))'
)
_NEW_TRADE_SNAPSHOT = (
    '(market_snapshot_version IS NULL OR (market_snapshot_version = 1 AND product_id IS NOT NULL AND '
    "product_name IS NOT NULL AND trim(product_name) <> '' AND fuel_type IS NOT NULL AND "
    "trim(fuel_type) <> '' AND fuel_grade IS NOT NULL AND trim(fuel_grade) <> '' AND market_product "
    "IN ('UCOME_B100','BIO_METHANOL','E_METHANOL','BIO_ETHANOL','SYNTHETIC_ETHANOL') AND "
    "availability_window IS NOT NULL AND (availability_window = 'SPOT' OR availability_window ~ "
    "'^[0-9]{4}-(0[1-9]|1[0-2])$' OR availability_window ~ '^[0-9]{4}-Q[1-4]$' OR availability_window "
    "~ '^[0-9]{4}-CAL$') AND delivery_point_id IS NOT NULL AND delivery_point_name IS NOT NULL AND "
    'delivery_point_region IS NOT NULL))'
)

_TERM_COLUMNS = (
    ("orderbook_orders", "fame_terms", "ck_orderbook_orders_fame_terms", ORDER_FAME_TERMS),
    ("trades", "fame_terms_snapshot", "ck_trades_fame_terms", TRADE_FAME_TERMS),
    ("negotiations", "fame_terms_snapshot", "ck_negotiations_fame_terms", NEGOTIATION_FAME_TERMS),
    ("inventory_items", "fame_terms", "ck_inventory_items_fame_terms", INVENTORY_FAME_TERMS),
    ("market_support_authorizations", "fame_terms", "ck_market_support_auth_fame_terms", MARKET_SUPPORT_FAME_TERMS),
)
_OLD_EXECUTION_CHECKS = (
    ("orderbook_orders", "ck_orderbook_orders_execution_product"),
    ("negotiations", "ck_negotiations_execution_product"),
    ("market_support_authorizations", "ck_market_support_auth_execution_product"),
)
_SIGNAL_TABLES = ("market_indications", "fair_price_bands", "physical_stems")
_OLD_SIGNAL_PRODUCT = "market_product IN ('BIO_METHANOL', 'E_METHANOL', 'BIO_ETHANOL', 'SYNTHETIC_ETHANOL')"
_NEW_SIGNAL_PRODUCT = (
    "market_product IN ('UCOME_B100', 'BIO_METHANOL', 'E_METHANOL', 'BIO_ETHANOL', 'SYNTHETIC_ETHANOL') "
    "AND (market_product <> 'UCOME_B100' OR "
    "replace(CAST(delivery_point_id AS TEXT), '-', '') = '73835e92820e584b8280bb61c63aa28e')"
)


def _catalog_description(executable: bool) -> None:
    description = (
        "Neat B100 used cooking oil methyl ester for wholesale Singapore trading. "
        "1 MT is the platform input minimum; orders require compatible quality and delivery terms."
        if executable else
        "Neat B100 used cooking oil methyl ester for wholesale Singapore RFQs. "
        "1 MT is the platform input minimum; contract minimum fill and quality terms are negotiated."
    )
    op.get_bind().execute(sa.text(
        "UPDATE products SET spec_description = :description WHERE id = :product_id"
    ), {"description": description, "product_id": _PRODUCT_ID})


def upgrade() -> None:
    for table, column, check_name, expression in _TERM_COLUMNS:
        op.add_column(table, sa.Column(column, sa.JSON(), nullable=True))
        op.create_check_constraint(check_name, table, expression)
    for table, check_name in _OLD_EXECUTION_CHECKS:
        op.drop_constraint(check_name, table, type_="check")
    op.drop_constraint("ck_trades_snapshot", "trades", type_="check")
    op.create_check_constraint("ck_trades_snapshot", "trades", _NEW_TRADE_SNAPSHOT)
    op.execute(sa.text(_trade_guard_sql(include_fame=True)))
    for table in _SIGNAL_TABLES:
        op.drop_constraint(f"ck_{table}_market_product", table, type_="check")
        op.create_check_constraint(f"ck_{table}_market_product", table, _NEW_SIGNAL_PRODUCT)
    _catalog_description(executable=True)


def downgrade() -> None:
    connection = op.get_bind()
    tables = [table for table, *_rest in _TERM_COLUMNS] + list(_SIGNAL_TABLES)
    connection.execute(sa.text(
        "LOCK TABLE " + ", ".join(tables) + " IN ACCESS EXCLUSIVE MODE NOWAIT"
    ))
    history_queries = [
        f"EXISTS (SELECT 1 FROM {table} WHERE {column} IS NOT NULL)"
        for table, column, *_rest in _TERM_COLUMNS
    ]
    history_queries.extend(
        f"EXISTS (SELECT 1 FROM {table} WHERE market_product = 'UCOME_B100')"
        for table in _SIGNAL_TABLES
    )
    if connection.execute(sa.text("SELECT " + " OR ".join(history_queries))).scalar_one():
        raise RuntimeError("Cannot remove B100 execution fields while contract or market evidence history exists")
    # Every affected row is now representable by the parent schema. RFQ and
    # supplier-offer history stays in place and never enters the execution book.
    op.execute(sa.text(_trade_guard_sql(include_fame=False)))
    op.drop_constraint("ck_trades_snapshot", "trades", type_="check")
    op.create_check_constraint("ck_trades_snapshot", "trades", _OLD_TRADE_SNAPSHOT)
    for table in _SIGNAL_TABLES:
        op.drop_constraint(f"ck_{table}_market_product", table, type_="check")
        op.create_check_constraint(f"ck_{table}_market_product", table, _OLD_SIGNAL_PRODUCT)
    for table, check_name in _OLD_EXECUTION_CHECKS:
        op.create_check_constraint(check_name, table, f"product_id <> '{_PRODUCT_ID}'")
    for table, column, check_name, _expression in reversed(_TERM_COLUMNS):
        op.drop_constraint(check_name, table, type_="check")
        op.drop_column(table, column)
    _catalog_description(executable=False)
