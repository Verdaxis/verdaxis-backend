# Price_discovery

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Price_discovery subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/reference` → out: PriceDiscoveryResponse [auth, db]
  `app/routers/price_discovery.py`
- `GET` `/reference/export` → out: PriceDiscoveryResponse [auth, db]
  `app/routers/price_discovery.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/price_discovery.py`

---
_Back to [overview.md](./overview.md)_