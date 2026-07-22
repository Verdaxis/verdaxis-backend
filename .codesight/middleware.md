# Middleware

## auth
- auth_2026_07_add_must_change_password — `alembic/versions/auth_2026_07_add_must_change_password.py`
- auth_maintenance — `app/cli/auth_maintenance.py`
- execution — `app/middleware/execution.py`
- preauth_rate_limit — `app/middleware/preauth_rate_limit.py`
- rbac — `app/middleware/rbac.py`
- auth_simple — `app/routers/auth_simple.py`
- auth_maintenance — `app/services/auth_maintenance.py`
- auth-maintenance-timer — `docs/auth-maintenance-timer.md`
- test_preauth_rate_limit — `tests/unit/test_preauth_rate_limit.py`

## custom
- subscription — `app/middleware/subscription.py`
- integrated-migration-cutover — `docs/runbooks/integrated-migration-cutover.md`

## rate-limit
- rate_limit — `app/rate_limit.py`

## cors
- test_idempotency_cors — `tests/unit/test_idempotency_cors.py`
