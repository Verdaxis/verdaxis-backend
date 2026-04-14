# Admin

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Admin subsystem handles **9 routes** and touches: auth, db.

## Routes

- `GET` `/admin/audit-logs` → in: Annotated, out: list [auth, db]
  `app/routers/audit.py`
- `PUT` `/admin/{user_id}/approve` params(user_id) → in: uuid [auth, db, upload]
  `app/routers/kyc.py`
- `PUT` `/admin/{user_id}/reject` params(user_id) → in: uuid [auth, db, upload]
  `app/routers/kyc.py`
- `GET` `/admin/commissions` → in: Use, out: list [auth, db]
  `app/routers/orders.py`
- `GET` `/admin/commissions/summary` → in: Use, out: list [auth, db]
  `app/routers/orders.py`
- `PUT` `/admin/commissions/{commission_id}` params(commission_id) → in: UUID, out: list [auth, db]
  `app/routers/orders.py`
- `GET` `/admin/subscriptions` → in: Annotated, out: SubscriptionResponse [auth, db]
  `app/routers/subscriptions.py`
- `GET` `/admin/subscriptions/{org_id}` params(org_id) → in: Annotated, out: SubscriptionResponse [auth, db]
  `app/routers/subscriptions.py`
- `PUT` `/admin/subscriptions/{org_id}` params(org_id) → in: uuid, out: SubscriptionResponse [auth, db]
  `app/routers/subscriptions.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/audit.py`
- `app/routers/kyc.py`
- `app/routers/orders.py`
- `app/routers/subscriptions.py`

---
_Back to [overview.md](./overview.md)_