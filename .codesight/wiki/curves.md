# Curves

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Curves subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/api/v1` → in: UUI, out: ForwardCurveResponse [auth, db]
  `app/routers/curves.py`
- `GET` `/api/v1/export` → in: UUI, out: ForwardCurveResponse [auth, db]
  `app/routers/curves.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/curves.py`

---
_Back to [overview.md](./overview.md)_