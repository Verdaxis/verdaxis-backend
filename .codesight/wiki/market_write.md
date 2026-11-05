# Market_write

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Market_write subsystem handles **19 routes** and touches: auth, db.

## Routes

- `PUT` `/api/orderbook/{id}` params(id) [auth, db]
  `rs/read/src/routes/market_write/amend.rs`
- `POST` `/api/inventory/{item_id}/publish` params(item_id) [auth, db]
  `rs/read/src/routes/market_write/inventory.rs`
- `GET` `/api/inventory` [auth, db]
  `rs/read/src/routes/market_write/inventory.rs`
- `POST` `/api/inventory` [auth, db]
  `rs/read/src/routes/market_write/inventory.rs`
- `PATCH` `/api/inventory/{item_id}` params(item_id) [auth, db]
  `rs/read/src/routes/market_write/inventory.rs`
- `DELETE` `/api/inventory/{item_id}` params(item_id) [auth, db]
  `rs/read/src/routes/market_write/inventory.rs`
- `GET` `/api/listings/my` [auth, db]
  `rs/read/src/routes/market_write/inventory.rs`
- `POST` `/api/orderbook` [auth, db]
  `rs/read/src/routes/market_write/order_create.rs`
- `GET` `/api/orderbook/my` [auth, db]
  `rs/read/src/routes/market_write/orders.rs`
- `GET` `/api/orderbook/my/latest-ask-template` [auth, db]
  `rs/read/src/routes/market_write/orders.rs`
- `DELETE` `/api/orderbook/{id}` params(id) [auth, db]
  `rs/read/src/routes/market_write/orders.rs`
- `POST` `/api/orderbook/{id}/cancel` params(id) [auth, db]
  `rs/read/src/routes/market_write/orders.rs`
- `PUT` `/api/trades/{trade_id}/decline` params(trade_id) [auth, db]
  `rs/read/src/routes/market_write/trade_lifecycle.rs`
- `PUT` `/api/trades/{trade_id}/deliver` params(trade_id) [auth, db]
  `rs/read/src/routes/market_write/trade_lifecycle.rs`
- `POST` `/api/trades/{trade_id}/pay` params(trade_id) [auth, db]
  `rs/read/src/routes/market_write/trade_lifecycle.rs`
- `POST` `/api/trades/` [auth, db]
  `rs/read/src/routes/market_write/trades.rs`
- `POST` `/api/trades` [auth, db]
  `rs/read/src/routes/market_write/trades.rs`
- `GET` `/api/trades/my` [auth, db]
  `rs/read/src/routes/market_write/trades.rs`
- `GET` `/api/trades/summary` [auth, db]
  `rs/read/src/routes/market_write/trades.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/routes/market_write/amend.rs`
- `rs/read/src/routes/market_write/inventory.rs`
- `rs/read/src/routes/market_write/order_create.rs`
- `rs/read/src/routes/market_write/orders.rs`
- `rs/read/src/routes/market_write/trade_lifecycle.rs`
- `rs/read/src/routes/market_write/trades.rs`

---
_Back to [overview.md](./overview.md)_