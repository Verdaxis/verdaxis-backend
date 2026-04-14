# Kyc

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Kyc subsystem handles **1 routes** and touches: auth, db.

## Routes

- `POST` `/submit` → in: Annotated [auth, db, upload]
  `app/routers/kyc.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/kyc.py`

---
_Back to [overview.md](./overview.md)_