# Subscription

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Subscription subsystem handles **1 routes** and touches: auth, db.

## Routes

- `GET` `/premium-feature` → in: Subscriptio [auth, db]
  `app/middleware/subscription.py`

## Related Models

- **Subscription** (6 fields) → [database.md](./database.md)

## Source Files

Read these before implementing or modifying this subsystem:
- `app/middleware/subscription.py`

---
_Back to [overview.md](./overview.md)_