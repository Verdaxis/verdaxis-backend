# Auth

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth subsystem handles **6 routes** and touches: auth, db.

## Routes

- `POST` `/api/login` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/refresh` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/logout` → in: _Request, out: LoginResponse [auth]
  `app/routers/auth_simple.py`
- `POST` `/api/register` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `PUT` `/api/me/password` → in: UserUpdate, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `PUT` `/{trade_id}/confirm` params(trade_id) → out: TradeResponse [db]
  `app/routers/trades.py`

## Middleware

- **auth_20261002_session_cutoff** (auth) — `alembic/versions/auth_20261002_session_cutoff.py`
- **auth_2026_07_add_must_change_password** (auth) — `alembic/versions/auth_2026_07_add_must_change_password.py`
- **execution** (auth) — `app/middleware/execution.py`
- **preauth_rate_limit** (auth) — `app/middleware/preauth_rate_limit.py`
- **rbac** (auth) — `app/middleware/rbac.py`
- **auth-maintenance-timer** (auth) — `docs/auth-maintenance-timer.md`
- **auth_tokens** (auth) — `rs/read/tests/auth_tokens.rs`
- **auth_transaction_panic** (auth) — `rs/read/tests/auth_transaction_panic.rs`
- **auth_transactions** (auth) — `rs/read/tests/auth_transactions.rs`
- **auth_writer** (auth) — `tests/rust_contract/phase2/auth_writer.py`
- **test_preauth_rate_limit** (auth) — `tests/unit/test_preauth_rate_limit.py`
- **auth_limits** (auth) — `work/t22-glm-draft/auth_limits.rs`

## Related Models

- **Body_login_api_auth_login_post** (6 fields) → [database.md](./database.md)

## High-Impact Files

- `app/routers/auth_simple.py` — imported by 61 files

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`
- `app/routers/trades.py`

---
_Back to [overview.md](./overview.md)_