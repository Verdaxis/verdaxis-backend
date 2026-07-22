# Auth_simple

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth_simple subsystem handles **16 routes** and touches: auth, db, cache, queue, email.

## Routes

- `GET` `/api/stream-token` → out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/register-with-org` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `GET` `/api/verify-email` → out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/resend-verification-email` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `GET` `/api/me` → out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/me` → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/forgot-password` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/reset-password` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/approve/{user_id}` params(user_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization/{organization_id}/approve` params(organization_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization/{organization_id}/reject` params(organization_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/reject/{user_id}` params(user_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `GET` `/api/organization-joins` → out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization-joins/{join_request_id}/approve` params(join_request_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization-joins/{join_request_id}/reject` params(join_request_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`
- `POST` `/api/survey` → in: _Request, out: RegistrationResponse [auth, db, cache, queue, email]
  `app/routers/auth_simple.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`

---
_Back to [overview.md](./overview.md)_