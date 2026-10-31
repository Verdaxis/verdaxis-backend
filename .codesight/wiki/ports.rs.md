# Ports.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Ports.rs subsystem handles **2 routes** and touches: db, cache.

## Routes

- `GET` `/api/ports` [db, cache]
  `rs/read/src/routes/ports.rs`
- `GET` `/api/ports/{port_id}` params(port_id) [db, cache]
  `rs/read/src/routes/ports.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/routes/ports.rs`

---
_Back to [overview.md](./overview.md)_