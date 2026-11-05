# Middleware

## auth
- auth_20261002_session_cutoff — `alembic/versions/auth_20261002_session_cutoff.py`
- auth_2026_07_add_must_change_password — `alembic/versions/auth_2026_07_add_must_change_password.py`
- execution — `app/middleware/execution.py`
- preauth_rate_limit — `app/middleware/preauth_rate_limit.py`
- rbac — `app/middleware/rbac.py`
- auth-maintenance-timer — `docs/auth-maintenance-timer.md`
- authority — `rs/read/src/order_write/authority.rs`
- auth — `rs/read/src/streams/auth.rs`
- auth_coexistence_postgres — `rs/read/tests/auth_coexistence_postgres.rs`
- auth_email_verification — `rs/read/tests/auth_email_verification.rs`
- auth_email_verification_postgres — `rs/read/tests/auth_email_verification_postgres.rs`
- auth_expiry — `rs/read/tests/auth_expiry.rs`
- auth_http — `rs/read/tests/auth_http.rs`
- auth_invitation_accept — `rs/read/tests/auth_invitation_accept.rs`
- auth_invitation_accept_postgres — `rs/read/tests/auth_invitation_accept_postgres.rs`
- auth_issue — `rs/read/tests/auth_issue.rs`
- auth_login — `rs/read/tests/auth_login.rs`
- auth_login_postgres — `rs/read/tests/auth_login_postgres.rs`
- auth_logout — `rs/read/tests/auth_logout.rs`
- auth_password — `rs/read/tests/auth_password.rs`
- auth_password_change — `rs/read/tests/auth_password_change.rs`
- auth_password_reset — `rs/read/tests/auth_password_reset.rs`
- auth_password_reset_postgres — `rs/read/tests/auth_password_reset_postgres.rs`
- auth_races_postgres — `rs/read/tests/auth_races_postgres.rs`
- auth_refresh — `rs/read/tests/auth_refresh.rs`
- auth_refresh_postgres — `rs/read/tests/auth_refresh_postgres.rs`
- auth_registration — `rs/read/tests/auth_registration.rs`
- auth_registration_postgres — `rs/read/tests/auth_registration_postgres.rs`
- auth_revocation_postgres — `rs/read/tests/auth_revocation_postgres.rs`
- auth_store_postgres — `rs/read/tests/auth_store_postgres.rs`
- auth_stream — `rs/read/tests/auth_stream.rs`
- auth_token_types — `rs/read/tests/auth_token_types.rs`
- auth_tokens — `rs/read/tests/auth_tokens.rs`
- auth_transaction_panic — `rs/read/tests/auth_transaction_panic.rs`
- auth_transactions — `rs/read/tests/auth_transactions.rs`
- auth_writer — `tests/rust_contract/phase2/auth_writer.py`
- test_preauth_rate_limit — `tests/unit/test_preauth_rate_limit.py`

## custom
- market_support_scope — `app/middleware/market_support_scope.py`
- public_market_read_admission — `app/middleware/public_market_read_admission.py`
- subscription — `app/middleware/subscription.py`
- integrated-migration-cutover — `docs/runbooks/integrated-migration-cutover.md`
- streams_watchlist_guard — `rs/read/tests/streams_watchlist_guard.rs`
- verify_deploy_start_guard — `scripts/verify_deploy_start_guard.py`
- test_phase6_watchlist_guard — `tests/postgres/test_phase6_watchlist_guard.py`

## error-handler
- network_guard — `tests/phase0/network_guard.py`

## rate-limit
- test_disposable_rate_limits — `tests/unit/test_disposable_rate_limits.py`

## cors
- test_idempotency_cors — `tests/unit/test_idempotency_cors.py`
