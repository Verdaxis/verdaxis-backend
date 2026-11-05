# Auth

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Auth subsystem handles **42 routes** and touches: auth, db, email, queue.

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
- `GET` `/api/auth/stream-token` [auth]
  `rs/read/examples/phase3_auth.rs`
- `GET` `/api/auth/me` [auth]
  `rs/read/examples/phase3_auth.rs`
- `POST` `/api/auth/verify-email` [auth, db, email]
  `rs/read/src/auth/email_verification.rs`
- `POST` `/api/auth/resend-verification-email` [auth, db, email]
  `rs/read/src/auth/email_verification.rs`
- `POST` `/api/auth/verify-email/` [auth, db, email]
  `rs/read/src/auth/email_verification.rs`
- `POST` `/api/auth/resend-verification-email/` [auth, db, email]
  `rs/read/src/auth/email_verification.rs`
- `POST` `/api/auth/invitations/accept` [auth, db]
  `rs/read/src/auth/invitation_accept.rs`
- `POST` `/api/auth/invitations/accept/` [auth, db]
  `rs/read/src/auth/invitation_accept.rs`
- `POST` `/api/auth/login` [auth, db]
  `rs/read/src/auth/login.rs`
- `POST` `/api/auth/login/` [auth, db]
  `rs/read/src/auth/login.rs`
- `POST` `/api/auth/logout` [auth, db]
  `rs/read/src/auth/logout.rs`
- `POST` `/api/auth/logout/` [auth, db]
  `rs/read/src/auth/logout.rs`
- `PUT` `/api/auth/me/password` [auth, db]
  `rs/read/src/auth/password_change.rs`
- `PUT` `/api/auth/me/password/` [auth, db]
  `rs/read/src/auth/password_change.rs`
- `POST` `/api/auth/forgot-password` [auth, db]
  `rs/read/src/auth/password_reset.rs`
- `POST` `/api/auth/reset-password` [auth, db]
  `rs/read/src/auth/password_reset.rs`
- `POST` `/api/auth/forgot-password/` [auth, db]
  `rs/read/src/auth/password_reset.rs`
- `POST` `/api/auth/reset-password/` [auth, db]
  `rs/read/src/auth/password_reset.rs`
- `POST` `/api/auth/refresh` [auth, db]
  `rs/read/src/auth/refresh.rs`
- `POST` `/api/auth/refresh/` [auth, db]
  `rs/read/src/auth/refresh.rs`
- `POST` `/api/auth/register` [auth, db]
  `rs/read/src/auth/registration.rs`
- `POST` `/api/auth/register/` [auth, db]
  `rs/read/src/auth/registration.rs`
- `POST` `/api/auth/register-with-org` [auth, db]
  `rs/read/src/auth/registration.rs`
- `POST` `/api/auth/register-with-org/` [auth, db]
  `rs/read/src/auth/registration.rs`
- `PUT` `/api/auth/approve/{user_id}` params(user_id) [auth, db, queue]
  `rs/read/src/business/admission.rs`
- `PUT` `/api/auth/reject/{user_id}` params(user_id) [auth, db, queue]
  `rs/read/src/business/admission.rs`
- `GET` `/api/auth/admin/review-queue` [auth, db, queue]
  `rs/read/src/business/admission.rs`
- `GET` `/api/auth/admin/review-queue/{user_id}` params(user_id) [auth, db, queue]
  `rs/read/src/business/admission.rs`
- `PUT` `/api/trades/{trade_id}/confirm` params(trade_id) [auth, db]
  `rs/read/src/routes/market_write/trade_lifecycle.rs`
- `GET` `/api/auth/stream-token/` [auth]
  `rs/read/src/runtime/phase3.rs`
- `PUT` `/api/auth/organization/{organization_id}/approve` params(organization_id) [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `PUT` `/api/auth/organization/{organization_id}/reject` params(organization_id) [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `GET` `/api/auth/organization-joins` [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `PUT` `/api/auth/organization-joins/{join_request_id}/approve` params(join_request_id) [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `PUT` `/api/auth/organization-joins/{join_request_id}/reject` params(join_request_id) [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `POST` `/api/auth/login/probe` [auth, db]
  `rs/read/tests/auth_login.rs`

## Middleware

- **auth_20261002_session_cutoff** (auth) — `alembic/versions/auth_20261002_session_cutoff.py`
- **auth_2026_07_add_must_change_password** (auth) — `alembic/versions/auth_2026_07_add_must_change_password.py`
- **execution** (auth) — `app/middleware/execution.py`
- **preauth_rate_limit** (auth) — `app/middleware/preauth_rate_limit.py`
- **rbac** (auth) — `app/middleware/rbac.py`
- **auth-maintenance-timer** (auth) — `docs/auth-maintenance-timer.md`
- **authority** (auth) — `rs/read/src/order_write/authority.rs`
- **auth** (auth) — `rs/read/src/streams/auth.rs`
- **auth_coexistence_postgres** (auth) — `rs/read/tests/auth_coexistence_postgres.rs`
- **auth_email_verification** (auth) — `rs/read/tests/auth_email_verification.rs`
- **auth_email_verification_postgres** (auth) — `rs/read/tests/auth_email_verification_postgres.rs`
- **auth_expiry** (auth) — `rs/read/tests/auth_expiry.rs`
- **auth_http** (auth) — `rs/read/tests/auth_http.rs`
- **auth_invitation_accept** (auth) — `rs/read/tests/auth_invitation_accept.rs`
- **auth_invitation_accept_postgres** (auth) — `rs/read/tests/auth_invitation_accept_postgres.rs`
- **auth_issue** (auth) — `rs/read/tests/auth_issue.rs`
- **auth_login** (auth) — `rs/read/tests/auth_login.rs`
- **auth_login_postgres** (auth) — `rs/read/tests/auth_login_postgres.rs`
- **auth_logout** (auth) — `rs/read/tests/auth_logout.rs`
- **auth_password** (auth) — `rs/read/tests/auth_password.rs`
- **auth_password_change** (auth) — `rs/read/tests/auth_password_change.rs`
- **auth_password_reset** (auth) — `rs/read/tests/auth_password_reset.rs`
- **auth_password_reset_postgres** (auth) — `rs/read/tests/auth_password_reset_postgres.rs`
- **auth_races_postgres** (auth) — `rs/read/tests/auth_races_postgres.rs`
- **auth_refresh** (auth) — `rs/read/tests/auth_refresh.rs`
- **auth_refresh_postgres** (auth) — `rs/read/tests/auth_refresh_postgres.rs`
- **auth_registration** (auth) — `rs/read/tests/auth_registration.rs`
- **auth_registration_postgres** (auth) — `rs/read/tests/auth_registration_postgres.rs`
- **auth_revocation_postgres** (auth) — `rs/read/tests/auth_revocation_postgres.rs`
- **auth_store_postgres** (auth) — `rs/read/tests/auth_store_postgres.rs`
- **auth_stream** (auth) — `rs/read/tests/auth_stream.rs`
- **auth_token_types** (auth) — `rs/read/tests/auth_token_types.rs`
- **auth_tokens** (auth) — `rs/read/tests/auth_tokens.rs`
- **auth_transaction_panic** (auth) — `rs/read/tests/auth_transaction_panic.rs`
- **auth_transactions** (auth) — `rs/read/tests/auth_transactions.rs`
- **auth_writer** (auth) — `tests/rust_contract/phase2/auth_writer.py`
- **test_preauth_rate_limit** (auth) — `tests/unit/test_preauth_rate_limit.py`

## Related Models

- **Body_login_api_auth_login_post** (6 fields) → [database.md](./database.md)

## High-Impact Files

- `app/routers/auth_simple.py` — imported by 65 files

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/auth_simple.py`
- `app/routers/trades.py`
- `rs/read/examples/phase3_auth.rs`
- `rs/read/src/auth/email_verification.rs`
- `rs/read/src/auth/invitation_accept.rs`
- `rs/read/src/auth/login.rs`
- `rs/read/src/auth/logout.rs`
- `rs/read/src/auth/password_change.rs`
- `rs/read/src/auth/password_reset.rs`
- `rs/read/src/auth/refresh.rs`
- `rs/read/src/auth/registration.rs`
- `rs/read/src/business/admission.rs`
- `rs/read/src/routes/market_write/trade_lifecycle.rs`
- `rs/read/src/runtime/phase3.rs`
- `rs/read/src/runtime/phase4.rs`
- `rs/read/tests/auth_login.rs`

---
_Back to [overview.md](./overview.md)_