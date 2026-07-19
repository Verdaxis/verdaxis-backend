# Auth

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth subsystem handles **8 routes** and touches: auth, db, cache, email.

## Routes

- `POST` `/api/login` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/refresh` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/logout` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/register` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/me/password` → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/compliance/verify` → in: Annotated, out: List [auth, db, upload]
  `app/routers/compliance.py`
- `POST` `/refresh` → in: AsyncSessio [auth, db]
  `app/routers/news.py`
- `PUT` `/{trade_id}/confirm` params(trade_id) → in: UUID, out: TradeResponse [auth, db]
  `app/routers/trades.py`

## Middleware

- **auth_2026_07_add_must_change_password** (auth) — `alembic/versions/auth_2026_07_add_must_change_password.py`
- **preauth_rate_limit** (auth) — `app/middleware/preauth_rate_limit.py`
- **rbac** (auth) — `app/middleware/rbac.py`
- **auth_simple** (auth) — `app/routers/auth_simple.py`
- **test_preauth_rate_limit** (auth) — `tests/unit/test_preauth_rate_limit.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`
- `app/routers/compliance.py`
- `app/routers/news.py`
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_