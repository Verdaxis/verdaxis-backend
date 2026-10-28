# Inventory

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Inventory subsystem handles **7 routes** and touches: db.

## Routes

- `GET` `/inventory` → in: Annotated, out: List [db]
  `app/routers/inventory.py`
- `POST` `/inventory` → out: List
  `app/routers/inventory.py`
- `PATCH` `/inventory/{item_id}` params(item_id) → out: List [db]
  `app/routers/inventory.py`
- `DELETE` `/inventory/{item_id}` params(item_id) → out: List [db]
  `app/routers/inventory.py`
- `POST` `/inventory/{item_id}/publish` params(item_id) → out: List [db]
  `app/routers/inventory.py`
- `GET` `/listings` → in: Annotated, out: List [db]
  `app/routers/inventory.py`
- `GET` `/listings/my` → in: Annotated, out: List [db]
  `app/routers/inventory.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/inventory.py`

---
_Back to [overview.md](./overview.md)_