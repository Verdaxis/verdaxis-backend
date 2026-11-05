# Auth_simple

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth_simple subsystem handles **18 routes** and touches: auth, db, email.

## Routes

- `GET` `/api/stream-token` → in: Annotated, out: LoginResponse [auth]
  `app/routers/auth_simple.py`
- `POST` `/api/invitations/resolve` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/invitations/accept` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/register-with-org` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/verify-email` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/resend-verification-email` → in: _Request, out: LoginResponse [db, email]
  `app/routers/auth_simple.py`
- `GET` `/api/me` → in: Annotated, out: LoginResponse
  `app/routers/auth_simple.py`
- `PUT` `/api/me` → in: UserUpdate, out: LoginResponse
  `app/routers/auth_simple.py`
- `POST` `/api/forgot-password` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `POST` `/api/reset-password` → in: _Request, out: LoginResponse [auth, db]
  `app/routers/auth_simple.py`
- `PUT` `/api/approve/{user_id}` params(user_id) → in: UserUpdate, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization/{organization_id}/approve` params(organization_id) → in: UserUpdate, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization/{organization_id}/reject` params(organization_id) → in: UserUpdate, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `PUT` `/api/reject/{user_id}` params(user_id) → in: UserUpdate, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `GET` `/api/organization-joins` → in: Annotated, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization-joins/{join_request_id}/approve` params(join_request_id) → in: UserUpdate, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `PUT` `/api/organization-joins/{join_request_id}/reject` params(join_request_id) → in: UserUpdate, out: LoginResponse
  `app/routers/auth_simple.py`
- `POST` `/api/survey` → in: _Request, out: LoginResponse [auth]
  `app/routers/auth_simple.py`

## Related Models

- **Body_login_api_auth_login_post** (6 fields) → [database.md](./database.md)

## High-Impact Files

- `app/routers/auth_simple.py` — imported by 61 files

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`

---
_Back to [overview.md](./overview.md)_