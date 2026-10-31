# Preferences.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Preferences.rs subsystem handles **2 routes** and touches: auth, db.

## Routes

- `GET` `/api/users/me/preferences` [auth, db]
  `rs/read/src/crud/preferences.rs`
- `PUT` `/api/users/me/preferences/{namespace}` params(namespace) [auth, db]
  `rs/read/src/crud/preferences.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/crud/preferences.rs`

---
_Back to [overview.md](./overview.md)_