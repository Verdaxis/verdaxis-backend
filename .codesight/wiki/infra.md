# Infra

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Infra subsystem handles **6 routes** and touches: auth, db, cache.

## Routes

- `GET` `/` [auth, db, cache]
  `app/main.py`
- `GET` `/health` [auth, db, cache]
  `app/main.py`
- `GET` `/health/live` [auth, db, cache]
  `app/main.py`
- `GET` `/health/ready` [auth, db, cache]
  `app/main.py`
- `GET` `/status` → in: Annotated [auth, db, upload]
  `app/routers/kyc.py`
- `POST` `/` → in: TradeCreate, out: TradeResponse [auth, db]
  `app/routers/trades.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/main.py`
- `app/routers/kyc.py`
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_