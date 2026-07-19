# Auth_simple

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth_simple subsystem handles **12 routes** and touches: auth, db, cache, email.

## Routes

- `GET` `/api/stream-token` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/register-with-org` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `GET` `/api/verify-email` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/resend-verification` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/resend-verification-email` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `GET` `/api/me` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/me` → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/forgot-password` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/reset-password` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/approve/{user_id}` params(user_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/switch-role/{target_role}` params(target_role) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`
- `POST` `/api/survey` → out: RegistrationResponse [auth, db, cache, email]
  `app/routers/auth_simple.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`

---
_Back to [overview.md](./overview.md)_