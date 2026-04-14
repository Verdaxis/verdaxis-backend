# Trades

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Trades subsystem handles **3 routes** and touches: auth, db.

## Routes

- `PUT` `/{trade_id}/decline` params(trade_id) → in: UUID, out: TradeResponse [auth, db]
  `app/routers/trades.py`
- `PUT` `/{trade_id}/deliver` params(trade_id) → in: UUID, out: TradeResponse [auth, db]
  `app/routers/trades.py`
- `POST` `/{trade_id}/pay` params(trade_id) → in: TradeCreate, out: TradeResponse [auth, db]
  `app/routers/trades.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_