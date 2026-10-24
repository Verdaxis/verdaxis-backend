# Phase 3 Python auth suites: business-rule checklist

Each row is a business rule that stays true through the Phase 3 cutover. The
coverage cell names the Rust test that proves it, or the phase that still owns
the route. This is a reference list, not a runnable gate: the Python suites
stay until their routes are deleted.

Source: the Phase 3 plan, section 10.2. Files are read from this repository at
the repository root.

| Python suite | Business rule | Coverage |
|---|---|---|
| `tests/unit/test_auth.py` | bcrypt hash and verify; 72-byte pre-hash boundary; 1024-byte maximum; bounded password workers; access and refresh claim sets; expiry; login-day rows | `password_prehash_boundary_72_and_maximum_1024`, `password_worker_bound_is_four`, `access_token_claims_are_stable`, `login_day_upsert_is_one_row` |
| `tests/unit/test_auth_cookies.py` | Cookie attributes per environment; device cookie shape; only hashes persisted; refresh without a device cookie refused; no body-token path; logout clears both | `refresh_cookie_attributes_match_environment`, `device_cookie_attributes_and_clear`, `refresh_requires_matching_device_cookie`, `refresh_body_token_fallback_is_refused`, `logout_revokes_device_family_and_clears_cookies` |
| `tests/unit/test_auth_lock_modes.py` | Shared read lock vs exclusive write lock selection on auth paths | `auth_lock_order_serializes_device_user_refresh` |
| `tests/unit/test_auth_maintenance.py` | Bounded cleanup of refresh rows, pending registrations, reset and verification tokens | `reset_token_cleanup_is_bounded` (refresh-row cleanup stays with the Phase 10 owner) |
| `tests/unit/test_auth_revocation_contract.py` | Microsecond `iat_us`; monotonic cutoffs; revocation contract | `revocation_cutoff_is_monotonic_under_concurrency`, `refresh_cutoff_order_password_then_revocation`, `pd7_access_token_iat_out_of_range_is_401` |
| `tests/unit/test_auth_survey.py` | Write-once survey | Phase 4 (survey route) |
| `tests/unit/test_browser_security_boundaries.py` | Origin guard; cookie and CORS boundaries | `origin_guard_matrix`, `refresh_cookie_attributes_match_environment` |
| `tests/unit/test_preauth_rate_limit.py` | Prefix buckets; 429; `Retry-After` | `preauth_and_endpoint_limits_share_one_counter` |
| `tests/unit/test_security_v2_followup.py` | Cutoff precision; cookie security per environment; logout failure path | `password_change_bumps_cutoff_monotonically`, `refresh_cookie_attributes_match_environment`, `logout_error_path_clears_cookies` |
| `tests/unit/test_security_v2_remediation.py` | Token and cookie handling remediation contracts | `origin_guard_matrix`, `refresh_replay_after_grace_revokes_family`, `auth_errors_use_the_detail_code_envelope` |
| `tests/unit/test_sprint1_security_contracts.py` | Auth and RBAC contracts | `auth_errors_use_the_detail_code_envelope`, `origin_guard_matrix` (RBAC stays Phase 4) |
| `tests/unit/test_admin_invitations.py` | Invitation create, resolve and accept | `invitation_accept_consumes_token_and_issues_one_family` (create and resolve are Phase 4) |
| `tests/unit/test_refresh_error_contract.py` | Terminal vs transient refresh codes; 409 grace | `refresh_duplicate_within_grace_is_409`, `refresh_replay_after_grace_revokes_family` |
| `tests/unit/test_email_verification.py` | Verification flow | `verify_email_consumes_token_once`, `verify_advances_referral_and_assigns_code_once` |
| `tests/unit/test_signup_canary_email_suppression.py` | Canary suppresses signup and verification email | `canary_suppresses_signup_and_verification_email` |
| `tests/unit/test_stream_tokens.py` | Stream token claims, environment, 60 s life | `stream_token_claims_and_ttl` |
| `tests/unit/test_stream_authorization.py` | Stream token authorization and tenant binding | Phase 6 (`rust_stream_token_passes_python_stream_check` is the Task 13 cross-check) |
| `tests/unit/test_admin_review_queue.py` | Admin review projection | Phase 4 (review-queue routes) |
| `tests/unit/test_organization_join_review.py` | Join approve and reject rules | Phase 4 (organization-join routes) |
| `tests/unit/test_profile_role_contract.py` | Profile role rules | Phase 4 (profile routes) |
| `tests/postgres/test_auth_read_locks.py` | Shared reads overlap; a status update waits | `auth_lock_order_serializes_device_user_refresh` |
| `tests/postgres/test_account_rejection_sessions.py` | Reject and re-approve; cutoff downgrade; legacy null role | `refresh_vs_admin_revocation_never_issues_after_cutoff`, `revocation_cutoff_is_monotonic_under_concurrency` (legacy null role is Phase 4) |
| `tests/postgres/test_device_session_races.py` | Device lock; a legacy null-device family cannot rotate | `device_session_limit_is_one_live_family`, `python_and_rust_device_lock_contend_on_one_key` |
| `tests/postgres/test_refresh_session_concurrency.py` | Duplicate refresh; logout vs rotation; rejection vs refresh; password-change lock order | `concurrent_refresh_same_token_yields_one_successor`, `refresh_vs_logout_revokes_successor`, `refresh_vs_password_change_never_issues_after_cutoff` |
| `tests/postgres/test_account_approval_email_concurrency.py` | Approval email transition | Phase 4 (approval route) |
| `tests/postgres/test_product_analytics_login_concurrency.py` | Login-day upsert | `login_day_upsert_is_one_row` |
| `tests/postgres/test_organization_market_approval.py` | Organization approval and rejection | Phase 4 (organization routes) |
| `tests/postgres/test_user_status_transitions.py` | Status-history append | `registration_is_one_transaction`, `register_join_request_is_created_for_domain_match` |
| `tests/integration/test_auth_hardening.py` | Real HTTP login returns both tokens; access token on `/me`; rotation; invalid refresh; access-as-refresh; unauthenticated `/me`; health | `rust_access_token_verifies_in_python_me`, `rust_refresh_token_rotates_in_python`, `logout_revokes_device_family_and_clears_cookies` (frontend journeys replace the HTTP suite as the Phase 3 gate) |
