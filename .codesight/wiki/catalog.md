# Catalog

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Catalog subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/products` → in: AsyncSessio, out: list [auth, db]
  `app/routers/catalog.py`
- `GET` `/delivery-points` → in: AsyncSessio, out: list [auth, db]
  `app/routers/catalog.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/catalog.py`

---
_Back to [overview.md](./overview.md)_