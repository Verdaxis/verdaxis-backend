# Ports

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Ports subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/ports` → in: Annotated, out: List [auth, db]
  `app/routers/ports.py`
- `GET` `/ports/{port_id}` params(port_id) → in: Annotated, out: List [auth, db]
  `app/routers/ports.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/ports.py`

---
_Back to [overview.md](./overview.md)_