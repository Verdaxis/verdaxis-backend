# Matchmaking

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Matchmaking subsystem handles **2 routes** and touches: db.

## Routes

- `GET` `/api/suggestions` → in: Annotated [db]
  `app/routers/matchmaking.py`
- `PATCH` `/api/suggestions/{order_id}/dismiss` params(order_id) → in: UUID [db]
  `app/routers/matchmaking.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/matchmaking.py`

---
_Back to [overview.md](./overview.md)_