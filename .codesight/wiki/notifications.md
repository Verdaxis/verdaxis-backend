# Notifications

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Notifications subsystem handles **3 routes** and touches: auth, db.

## Routes

- `GET` `/unread-count` → in: in, out: List [auth, db]
  `app/routers/notifications.py`
- `PATCH` `/{notification_id}/read` params(notification_id) → in: uuid, out: List [auth, db]
  `app/routers/notifications.py`
- `PATCH` `/read-all` → in: uuid, out: List [auth, db]
  `app/routers/notifications.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/notifications.py`

---
_Back to [overview.md](./overview.md)_