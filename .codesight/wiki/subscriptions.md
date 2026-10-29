# Subscriptions

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Subscriptions subsystem handles **2 routes**.

## Routes

- `GET` `/subscriptions/fees` → in: Annotated, out: FeeScheduleResponse
  `app/routers/subscriptions.py`
- `GET` `/subscriptions/me` → in: Annotated, out: FeeScheduleResponse
  `app/routers/subscriptions.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/subscriptions.py`

---
_Back to [overview.md](./overview.md)_