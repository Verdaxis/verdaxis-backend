# Admin_analytics

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Admin_analytics subsystem handles **12 routes** and touches: auth, db.

## Routes

- `GET` `/api/product-usage` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/overview` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/daily` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/users` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `PUT` `/api/users/{user_id}/reject` params(user_id) → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/overview` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/acquisition` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/activation` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/engagement` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/marketplace` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/retention` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`
- `GET` `/api/product-analytics/reliability` → out: ProductUsageResponse [auth, db]
  `app/routers/admin_analytics.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/admin_analytics.py`

---
_Back to [overview.md](./overview.md)_