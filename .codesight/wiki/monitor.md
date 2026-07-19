# Monitor

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Monitor subsystem handles **1 routes** and touches: auth, db.

## Routes

- `POST` `/signup-canary-cleanup` → in: CanaryCleanupRequest, out: CanaryCleanupResponse [auth, db]
  `app/routers/monitor.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/monitor.py`

---
_Back to [overview.md](./overview.md)_