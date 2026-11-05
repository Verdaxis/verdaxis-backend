# Orderbook_lists.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Orderbook_lists.rs subsystem handles **4 routes** and touches: auth, db, cache.

## Routes

- `GET` `/api/orderbook/bids` [auth, db, cache]
  `rs/read/src/routes/orderbook_lists.rs`
- `GET` `/api/orderbook/asks` [auth, db, cache]
  `rs/read/src/routes/orderbook_lists.rs`
- `POST` `/api/orderbook` [auth, db, cache]
  `rs/read/src/routes/orderbook_lists.rs`
- `GET` `/api/orderbook/with-ci` [auth, db, cache]
  `rs/read/src/routes/orderbook_lists.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/routes/orderbook_lists.rs`

---
_Back to [overview.md](./overview.md)_