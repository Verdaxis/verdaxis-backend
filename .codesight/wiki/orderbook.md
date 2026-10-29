# Orderbook

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Orderbook subsystem handles **15 routes** and touches: db, queue, auth.

## Routes

- `GET` `/snapshot` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/bids` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/asks` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/with-ci` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/my` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/my/latest-ask-template` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/aggregated` → in: MarketProduc, out: OrderBookSnapshotResponse
  `app/routers/orderbook.py`
- `GET` `/product-counts` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/map-summary` → in: MarketProduc, out: OrderBookSnapshotResponse
  `app/routers/orderbook.py`
- `GET` `/map-summary/compact` → in: MarketProduc, out: OrderBookSnapshotResponse
  `app/routers/orderbook.py`
- `GET` `/regions` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `GET` `/fuel-types` → in: MarketProduc, out: OrderBookSnapshotResponse [db]
  `app/routers/orderbook.py`
- `PUT` `/{order_id}` params(order_id) → out: OrderBookSnapshotResponse [db, queue]
  `app/routers/orderbook.py`
- `POST` `/{order_id}/cancel` params(order_id) → out: OrderBookSnapshotResponse [auth, db]
  `app/routers/orderbook.py`
- `DELETE` `/{order_id}` params(order_id) → out: OrderBookSnapshotResponse [auth, db]
  `app/routers/orderbook.py`

## High-Impact Files

- `app/routers/orderbook.py` — imported by 24 files

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/orderbook.py`

---
_Back to [overview.md](./overview.md)_