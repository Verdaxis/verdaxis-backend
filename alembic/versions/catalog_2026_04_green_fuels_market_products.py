"""align active product catalog to approved green fuels market products

Revision ID: catalog_2026_04_green_fuels
Revises: ob_2026_04_availability
Create Date: 2026-04-08
"""

from alembic import op
import sqlalchemy as sa
import uuid


revision = "catalog_2026_04_green_fuels"
down_revision = "ob_2026_04_availability"
branch_labels = None
depends_on = None


_NS = uuid.UUID("a1b2c3d4-e5f6-7890-abcd-ef1234567890")


def _product_id(name: str) -> uuid.UUID:
    return uuid.uuid5(_NS, f"product:{name}")


def upgrade() -> None:
    bind = op.get_bind()

    active_products = [
        {
            "id": _product_id("Bio Ethanol"),
            "name": "Bio Ethanol",
            "fuel_type": "Ethanol",
            "fuel_grade": "Bio",
            "unit": "MT",
            "min_lot_size": 200,
            "spec_description": "Second-generation bioethanol from waste feedstocks",
        },
        {
            "id": _product_id("Bio Methanol"),
            "name": "Bio Methanol",
            "fuel_type": "Methanol",
            "fuel_grade": "Bio",
            "unit": "MT",
            "min_lot_size": 200,
            "spec_description": "Bio-methanol produced from biogenic feedstocks for marine fuel use",
        },
        {
            "id": _product_id("e-Methanol"),
            "name": "e-Methanol",
            "fuel_type": "Methanol",
            "fuel_grade": "E",
            "unit": "MT",
            "min_lot_size": 200,
            "spec_description": "Synthetic methanol produced from renewable hydrogen and captured CO2",
        },
        {
            "id": _product_id("Synthetic Ethanol"),
            "name": "Synthetic Ethanol",
            "fuel_type": "Ethanol",
            "fuel_grade": "Synthetic",
            "unit": "MT",
            "min_lot_size": 200,
            "spec_description": "Synthetic ethanol produced via power-to-liquids or equivalent synthetic pathways",
        },
    ]

    for product in active_products:
        bind.execute(
            sa.text(
                """
                INSERT INTO products (id, name, fuel_type, fuel_grade, unit, min_lot_size, spec_description, is_active)
                VALUES (:id, :name, :fuel_type, :fuel_grade, :unit, :min_lot_size, :spec_description, true)
                ON CONFLICT (id) DO UPDATE
                SET name = EXCLUDED.name,
                    fuel_type = EXCLUDED.fuel_type,
                    fuel_grade = EXCLUDED.fuel_grade,
                    unit = EXCLUDED.unit,
                    min_lot_size = EXCLUDED.min_lot_size,
                    spec_description = EXCLUDED.spec_description,
                    is_active = true
                """
            ),
            product,
        )

    bind.execute(
        sa.text(
            """
            UPDATE products
            SET is_active = false
            WHERE name IN ('Biomethane', 'Ammonia Green', 'Hydrogen Green', 'Biofuel Bio', 'Methanol Green', 'Ethanol Green')
              AND id NOT IN :active_ids
            """
        ).bindparams(sa.bindparam("active_ids", expanding=True)),
        {"active_ids": [product["id"] for product in active_products]},
    )


def downgrade() -> None:
    bind = op.get_bind()

    bind.execute(
        sa.text(
            """
            DELETE FROM products
            WHERE id IN :new_ids
            """
        ).bindparams(sa.bindparam("new_ids", expanding=True)),
        {
            "new_ids": [
                _product_id("Bio Ethanol"),
                _product_id("Bio Methanol"),
                _product_id("e-Methanol"),
                _product_id("Synthetic Ethanol"),
            ]
        },
    )

    bind.execute(
        sa.text(
            """
            UPDATE products
            SET is_active = true
            WHERE name IN ('Biomethane', 'Ammonia Green', 'Hydrogen Green', 'Biofuel Bio', 'Methanol Green', 'Ethanol Green')
            """
        )
    )
