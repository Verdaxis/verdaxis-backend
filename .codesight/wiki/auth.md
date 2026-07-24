# Auth

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth subsystem handles **6 routes** and touches: auth, db, cache, queue, email.

## Routes

- `POST` `/api/login` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/refresh` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/logout` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/register` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/me/password` → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/{trade_id}/confirm` params(trade_id) → out: TradeResponse [auth, db, queue]
  `app/routers/trades.py`

## Middleware

- **auth_2026_07_add_must_change_password** (auth) — `alembic/versions/auth_2026_07_add_must_change_password.py`
- **auth_maintenance** (auth) — `app/cli/auth_maintenance.py`
- **execution** (auth) — `app/middleware/execution.py`
- **preauth_rate_limit** (auth) — `app/middleware/preauth_rate_limit.py`
- **rbac** (auth) — `app/middleware/rbac.py`
- **auth_simple** (auth) — `app/routers/auth_simple.py`
- **auth_maintenance** (auth) — `app/services/auth_maintenance.py`
- **auth-maintenance-timer** (auth) — `docs/auth-maintenance-timer.md`
- **test_preauth_rate_limit** (auth) — `tests/unit/test_preauth_rate_limit.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_