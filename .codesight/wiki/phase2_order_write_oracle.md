# Phase2_order_write_oracle

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Phase2_order_write_oracle subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/.well-known/verdaxis-disposable-test` [auth]
  `scripts/rust_contract/phase2_order_write_oracle.py`
- `POST` `/__contract__/order-write` [auth, db]
  `scripts/rust_contract/phase2_order_write_oracle.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `scripts/rust_contract/phase2_order_write_oracle.py`

---
_Back to [overview.md](./overview.md)_