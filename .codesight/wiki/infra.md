# Infra

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Infra subsystem handles **6 routes** and touches: db.

## Routes

- `GET` `/`
  `app/main.py`
- `GET` `/health`
  `app/main.py`
- `GET` `/health/live`
  `app/main.py`
- `GET` `/health/ready` [db]
  `app/main.py`
- `GET` `/status` → in: Annotated
  `app/routers/kyc.py`
- `POST` `/` → out: TradeResponse [db]
  `app/routers/trades.py`

## High-Impact Files

- `app/main.py` — imported by 36 files

## Source Files

Read these before implementing or modifying this subsystem:
- `app/main.py`
- `app/routers/kyc.py`
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_