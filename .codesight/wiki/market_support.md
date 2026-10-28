# Market_support

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Market_support subsystem handles **16 routes** and touches: db, auth.

## Routes

- `GET` `/capabilities` → in: Annotated, out: list
  `app/routers/market_support.py`
- `POST` `/capability-assignments` → out: list [db]
  `app/routers/market_support.py`
- `POST` `/capability-assignments/{assignment_id}/revoke` params(assignment_id) → out: list [db]
  `app/routers/market_support.py`
- `GET` `/organizations` → in: Annotated, out: list [db]
  `app/routers/market_support.py`
- `GET` `/organizations/{organization_id}/entry` params(organization_id) → in: Annotated, out: list
  `app/routers/market_support.py`
- `POST` `/contexts` → out: list [db]
  `app/routers/market_support.py`
- `GET` `/contexts/active` → in: Annotated, out: list [db]
  `app/routers/market_support.py`
- `GET` `/contexts/{context_id}` params(context_id) → in: Annotated, out: list
  `app/routers/market_support.py`
- `POST` `/contexts/{context_id}/exit` params(context_id) → out: list
  `app/routers/market_support.py`
- `POST` `/organizations/{organization_id}/authorizations` params(organization_id) → out: list [auth, db]
  `app/routers/market_support.py`
- `GET` `/organizations/{organization_id}/authorizations` params(organization_id) → in: Annotated, out: list [auth, db]
  `app/routers/market_support.py`
- `POST` `/organizations/{organization_id}/listings` params(organization_id) → out: list [auth, db]
  `app/routers/market_support.py`
- `GET` `/organizations/{organization_id}/listings` params(organization_id) → in: Annotated, out: list [db]
  `app/routers/market_support.py`
- `POST` `/organizations/{organization_id}/listings/{order_id}/cancel` params(organization_id, order_id) → out: list
  `app/routers/market_support.py`
- `POST` `/organizations/{organization_id}/authorizations/{authorization_id}/revoke` params(organization_id, authorization_id) → out: list [auth, db]
  `app/routers/market_support.py`
- `GET` `/organizations/{organization_id}/context` params(organization_id) → in: Annotated, out: list [auth, db]
  `app/routers/market_support.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/market_support.py`

---
_Back to [overview.md](./overview.md)_