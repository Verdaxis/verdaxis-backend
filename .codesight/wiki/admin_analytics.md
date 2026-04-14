# Admin_analytics

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Admin_analytics subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/overview` → in: Annotated, out: OverviewResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/daily` → in: Annotated, out: OverviewResponse [auth, db]
  `app/routers/admin_analytics.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/admin_analytics.py`

---
_Back to [overview.md](./overview.md)_