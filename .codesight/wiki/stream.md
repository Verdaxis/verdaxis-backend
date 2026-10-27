# Stream

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Stream subsystem handles **3 routes** and touches: auth, cache, db.

## Routes

- `GET` `/api/prices` [auth, cache]
  `app/routers/stream.py`
- `GET` `/api/orderbook` [cache]
  `app/routers/stream.py`
- `GET` `/api/trades` [auth, db, cache]
  `app/routers/stream.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/stream.py`

---
_Back to [overview.md](./overview.md)_