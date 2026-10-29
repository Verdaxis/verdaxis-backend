# Catalog.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Catalog.rs subsystem handles **2 routes** and touches: db, cache.

## Routes

- `GET` `/api/catalog/products` [db, cache]
  `rs/read/src/routes/catalog.rs`
- `GET` `/api/catalog/delivery-points` [db, cache]
  `rs/read/src/routes/catalog.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/routes/catalog.rs`

---
_Back to [overview.md](./overview.md)_