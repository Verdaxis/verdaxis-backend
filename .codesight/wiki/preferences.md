# Preferences

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Preferences subsystem handles **2 routes** and touches: db.

## Routes

- `GET` `/api` → in: Use [db]
  `app/routers/preferences.py`
- `PUT` `/api/{namespace}` params(namespace) [db]
  `app/routers/preferences.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/preferences.py`

---
_Back to [overview.md](./overview.md)_