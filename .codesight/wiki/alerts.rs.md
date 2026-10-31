# Alerts.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Alerts.rs subsystem handles **3 routes** and touches: auth, db.

## Routes

- `GET` `/api/alerts` [auth, db]
  `rs/read/src/crud/alerts.rs`
- `POST` `/api/alerts` [auth, db]
  `rs/read/src/crud/alerts.rs`
- `DELETE` `/api/alerts/{alert_id}` params(alert_id) [auth, db]
  `rs/read/src/crud/alerts.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/crud/alerts.rs`

---
_Back to [overview.md](./overview.md)_