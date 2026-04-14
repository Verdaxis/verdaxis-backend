# Vessels

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Vessels subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/vessels` → in: Annotated, out: List [auth, db]
  `app/routers/vessels.py`
- `GET` `/vessels/{vessel_id}` params(vessel_id) → in: Annotated, out: List [auth, db]
  `app/routers/vessels.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/vessels.py`

---
_Back to [overview.md](./overview.md)_