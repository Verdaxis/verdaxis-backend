# Orderbook

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Orderbook subsystem handles **10 routes** and touches: auth, db, cache, queue.

## Routes

- `GET` `/bids` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/asks` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/with-ci` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/my` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/my/latest-ask-template` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/aggregated` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/regions` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `GET` `/fuel-types` → in: Optional, out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `PUT` `/{order_id}` params(order_id) → out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`
- `DELETE` `/{order_id}` params(order_id) → out: PaginatedResponse [auth, db, cache, queue]
  `app/routers/orderbook.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/orderbook.py`

---
_Back to [overview.md](./overview.md)_