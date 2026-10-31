"""Phase 5 cross-group writer: the retained Python writer path.

Called by phase5_postgres.rs with the harness venv's interpreter against the
same disposable database the Rust reads use. Rows are created through the
Python application's own SQLAlchemy models — the same tables, defaults,
constraints and triggers the production writers use — never through SQL text
the Rust tests could drift from.

Commands (argv[1]):

  commit           Create one REAL organization, its supplier and buyer
                   accounts, an open order-book BID (the demand source) and an
                   inventory row (the availability source), in one committed
                   transaction. argv[2] labels the pair so repeated calls stay
                   independent.
  clean            Delete every row this helper created (organizations named
                   `Cross Group%` and their orders, inventory and accounts).
  snapshot         Print, as one JSON object, the business row values the
                   composed reads depend on.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from uuid import NAMESPACE_URL, uuid5


def fid(name: str) -> str:
    return uuid5(NAMESPACE_URL, f"phase5-cross-group:{name}")


async def write_pair(label: str) -> dict[str, str]:
    """Create the supplier, buyer, one open BID and one inventory row."""
    # The app's own session factory binds to the disposable writer URL the
    # same way the deployed settings bind to the live database.
    url = os.environ["RUST_COMPLIANCE_TEST_WRITER_URL"]
    if url.startswith("postgres://"):
        url = "postgresql+asyncpg://" + url[len("postgres://"):]
    elif url.startswith("postgresql+asyncpg://"):
        pass
    else:
        url = "postgresql+asyncpg://" + url.split("://", 1)[1]
    os.environ["DATABASE_URL"] = url
    from app.database import AsyncSessionLocal, engine
    from app.market_catalog import CANONICAL_PRODUCTS
    from app.models.catalog import DeliveryPoint, Product
    from app.models.marketplace import FuelType, InventoryItem
    from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
    from app.models.port import Port
    from app.models.user import Organization, User, UserRole

    organization = fid(f"org:{label}")
    supplier = fid(f"user:{label}:supplier")
    buyer = fid(f"user:{label}:buyer")
    async with AsyncSessionLocal() as session:
        session.add(
            Organization(
                id=organization,
                name=f"Cross Group {label}",
                type="SUPPLIER",
                verification_status="APPROVED",
                provenance="REAL",
            )
        )
        session.add(
            User(
                id=supplier,
                email=f"supplier-{label}@cross-group.test",
                password_hash="synthetic",
                status="APPROVED",
                email_verified=True,
                kyc_status="APPROVED",
                kyc_organization_id=organization,
                organization_id=organization,
                role=UserRole.SUPPLIER,
            )
        )
        session.add(
            User(
                id=buyer,
                email=f"buyer-{label}@cross-group.test",
                password_hash="synthetic",
                status="APPROVED",
                email_verified=True,
                kyc_status="APPROVED",
                kyc_organization_id=organization,
                organization_id=organization,
                role=UserRole.BUYER,
            )
        )
        await session.flush()

        from sqlalchemy import select

        # Bio Methanol carries no declared-terms constraint; Bio-LNG is
        # Rotterdam-only by design and UCOME B100 requires the full FAME
        # terms object. Dalian is one of Bio Methanol's catalog lanes.
        spec = next(p for p in CANONICAL_PRODUCTS if p.name == "Bio Methanol")
        product = (
            await session.execute(select(Product).where(Product.id == spec.id))
        ).scalar_one()
        # Bio Methanol declares no exclusive lane list; Dalian is one of its
        # policy lanes (see deploy/rust/market-policy.json).
        from app.market_catalog import CANONICAL_DELIVERY_POINTS

        dalian = next(d for d in CANONICAL_DELIVERY_POINTS if d.name == "Dalian")
        point = (
            await session.execute(select(DeliveryPoint).where(DeliveryPoint.id == dalian.id))
        ).scalar_one()
        # The availability read joins the ports table for coordinates; create
        # the port row through the model if this disposable database has none.
        from sqlalchemy import func

        port = (
            await session.execute(select(Port).where(Port.name == point.name))
        ).scalar_one_or_none()
        if port is None:
            port = Port(
                # The ports primary key is a string code, not a UUID column.
                id=f"X{point.name[:3].upper()}",
                name=point.name,
                country="CN",
                location=func.ST_SetSRID(func.ST_MakePoint(121.6, 38.9), 4326),
                timezone="Asia/Shanghai",
                is_active=True,
            )
            session.add(port)
            await session.flush()

        order = OrderBookOrder(
            id=fid(f"order:{label}"),
            organization_id=organization,
            owner_user_id=buyer,
            created_by_actor_user_id=buyer,
            creation_method="SELF_SERVICE",
            provenance="REAL",
            side=OrderSide.BID,
            product_id=product.id,
            delivery_point_id=point.id,
            quantity_mt=Decimal("100.00"),
            remaining_quantity_mt=Decimal("100.00"),
            price_per_mt_usd=Decimal("600.00"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
            expires_at=datetime.now(timezone.utc) + timedelta(days=30),
        )
        session.add(order)

        session.add(
            InventoryItem(
                id=fid(f"inventory:{label}"),
                supplier_id=organization,
                owner_user_id=supplier,
                port_id=port.id,
                fuel_type=FuelType(spec.fuel_type),
                product_name=spec.name,
                current_stock_mt=Decimal(1200),
                incoming_stock_mt=Decimal(0),
                price_per_mt_usd=Decimal("125.50"),
            )
        )
        await session.commit()
    await engine.dispose()
    return {
        "organization": str(organization),
        "supplier": str(supplier),
        "buyer": str(buyer),
        "order": str(fid(f"order:{label}")),
        "inventory": str(fid(f"inventory:{label}")),
    }


async def snapshot() -> dict[str, object]:
    url = os.environ["RUST_COMPLIANCE_TEST_WRITER_URL"]
    if url.startswith("postgres://"):
        url = "postgresql+asyncpg://" + url[len("postgres://"):]
    elif url.startswith("postgresql+asyncpg://"):
        pass
    else:
        url = "postgresql+asyncpg://" + url.split("://", 1)[1]
    os.environ["DATABASE_URL"] = url
    from sqlalchemy import text

    from app.database import AsyncSessionLocal, engine

    async with AsyncSessionLocal() as session:
        rows = {}
        rows["open_bids"] = (
            await session.execute(
                text(
                    "SELECT count(*) FROM orderbook_orders WHERE side='BID' "
                    "AND status IN ('OPEN','PARTIALLY_FILLED')"
                )
            )
        ).scalar_one()
        rows["inventory_stock"] = (
            await session.execute(
                text("SELECT coalesce(sum(current_stock_mt),0) FROM inventory_items")
            )
        ).scalar_one()
        rows["vessels"] = (
            await session.execute(text("SELECT count(*) FROM vessels"))
        ).scalar_one()
    await engine.dispose()
    return rows


async def clean() -> None:
    url = os.environ["RUST_COMPLIANCE_TEST_WRITER_URL"]
    if url.startswith("postgres://"):
        url = "postgresql+asyncpg://" + url[len("postgres://"):]
    elif url.startswith("postgresql+asyncpg://"):
        pass
    else:
        url = "postgresql+asyncpg://" + url.split("://", 1)[1]
    os.environ["DATABASE_URL"] = url
    from sqlalchemy import text

    from app.database import AsyncSessionLocal, engine

    async with AsyncSessionLocal() as session:
        await session.execute(
            text(
                "DELETE FROM orderbook_orders WHERE organization_id IN "
                "(SELECT id FROM organizations WHERE name LIKE 'Cross Group%')"
            )
        )
        await session.execute(
            text(
                "DELETE FROM inventory_items WHERE supplier_id IN "
                "(SELECT id FROM organizations WHERE name LIKE 'Cross Group%')"
            )
        )
        await session.execute(
            text(
                "DELETE FROM users WHERE organization_id IN "
                "(SELECT id FROM organizations WHERE name LIKE 'Cross Group%')"
            )
        )
        await session.execute(text("DELETE FROM organizations WHERE name LIKE 'Cross Group%'"))
        await session.commit()
    await engine.dispose()


async def main() -> None:
    command = sys.argv[1]
    if command == "clean":
        await clean()
        print("WRITER_CLEAN=done")
    elif command == "commit":
        label = sys.argv[2]
        result = await write_pair(label)
        print(f"WRITER_COMMIT={json.dumps(result)}")
    elif command == "snapshot":
        print(f"WRITER_SNAPSHOT={json.dumps(await snapshot(), default=str)}")
    else:
        raise SystemExit(f"unknown command: {command}")


asyncio.run(main())
