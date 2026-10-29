# Middleware

## auth
- auth_20261002_session_cutoff — `alembic/versions/auth_20261002_session_cutoff.py`
- auth_2026_07_add_must_change_password — `alembic/versions/auth_2026_07_add_must_change_password.py`
- execution — `app/middleware/execution.py`
- preauth_rate_limit — `app/middleware/preauth_rate_limit.py`
- rbac — `app/middleware/rbac.py`
- auth-maintenance-timer — `docs/auth-maintenance-timer.md`
- auth_tokens — `rs/read/tests/auth_tokens.rs`
- auth_transaction_panic — `rs/read/tests/auth_transaction_panic.rs`
- auth_transactions — `rs/read/tests/auth_transactions.rs`
- auth_writer — `tests/rust_contract/phase2/auth_writer.py`
- test_preauth_rate_limit — `tests/unit/test_preauth_rate_limit.py`
- auth_limits — `work/t22-glm-draft/auth_limits.rs`

## custom
- market_support_scope — `app/middleware/market_support_scope.py`
- public_market_read_admission — `app/middleware/public_market_read_admission.py`
- subscription — `app/middleware/subscription.py`
- integrated-migration-cutover — `docs/runbooks/integrated-migration-cutover.md`
- verify_deploy_start_guard — `scripts/verify_deploy_start_guard.py`

## error-handler
- network_guard — `tests/phase0/network_guard.py`

## rate-limit
- test_disposable_rate_limits — `tests/unit/test_disposable_rate_limits.py`

## cors
- test_idempotency_cors — `tests/unit/test_idempotency_cors.py`
