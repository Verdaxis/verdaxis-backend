# Compliance

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Compliance subsystem handles **1 routes** and touches: auth, db.

## Routes

- `GET` `/compliance/ledger` → in: Annotated, out: List [auth, db, upload]
  `app/routers/compliance.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/compliance.py`

---
_Back to [overview.md](./overview.md)_