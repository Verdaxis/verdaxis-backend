# Curves

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Curves subsystem handles **5 routes** and touches: auth, db.

## Routes

- `GET` `/api/v1/table` → in: Optional, out: ForwardCurveTableResponse [auth, db]
  `app/routers/curves.py`
- `GET` `/api/v1/slice` → in: Optional, out: ForwardCurveTableResponse [auth, db]
  `app/routers/curves.py`
- `GET` `/api/v1/board` → in: Optional, out: ForwardCurveTableResponse [auth, db]
  `app/routers/curves.py`
- `GET` `/api/v1` → in: Optional, out: ForwardCurveTableResponse [auth, db]
  `app/routers/curves.py`
- `GET` `/api/v1/export` → in: Optional, out: ForwardCurveTableResponse [auth, db]
  `app/routers/curves.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/curves.py`

---
_Back to [overview.md](./overview.md)_