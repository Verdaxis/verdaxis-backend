# Trades

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Trades subsystem handles **4 routes** and touches: db, auth.

## Routes

- `GET` `/summary` → out: TradeResponse [db]
  `app/routers/trades.py`
- `PUT` `/{trade_id}/decline` params(trade_id) → out: TradeResponse
  `app/routers/trades.py`
- `PUT` `/{trade_id}/deliver` params(trade_id) → out: TradeResponse [auth]
  `app/routers/trades.py`
- `POST` `/{trade_id}/pay` params(trade_id) → out: TradeResponse
  `app/routers/trades.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_