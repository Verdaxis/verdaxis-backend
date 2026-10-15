"""Test-only projection of the read-only helper; no rebuild or persistence."""
from datetime import datetime

from sqlalchemy import event


async def project(query):
    import os

    from app.database import AsyncSessionLocal
    from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
    from app.models.orderbook import OrderSide
    from app.services.live_benchmarks import _calculate_live_slice_benchmarks

    now = datetime.fromisoformat(query.get("as_of", [os.environ["RUST_CONTRACT_CLOCK"]])[0])
    selection = query.get("selection", ["all"])[0]
    keys = [
        (side, product.market_product.value, point.id, window)
        for side in (OrderSide.ASK, OrderSide.BID)
        for product in CANONICAL_PRODUCTS
        for point in CANONICAL_DELIVERY_POINTS
        for window in ("SPOT", "2026-11", "2099-01")
    ]
    keys.sort(key=lambda key: (key[0].value, key[1], str(key[2]), key[3]))
    if selection == "empty":
        keys = []
    elif selection == "sparse":
        keys = keys[::7]
    async with AsyncSessionLocal() as session:
        connection = await session.connection()
        statements = []

        def count(conn, cursor, statement, parameters, context, executemany):
            statements.append(statement)

        event.listen(connection.sync_connection, "before_cursor_execute", count)
        try:
            result = await _calculate_live_slice_benchmarks(session, keys, as_of=now)
        finally:
            event.remove(connection.sync_connection, "before_cursor_execute", count)
        assert len(statements) == bool(keys), statements
    return [
        dict(
            side=key[0].value,
            product=key[1],
            point=str(key[2]),
            window=key[3],
            price=str(result[key][0]) if key in result else None,
            quantity=str(result[key][1]) if key in result else None,
            count=result[key][2] if key in result else None,
        )
        for key in keys
    ]
