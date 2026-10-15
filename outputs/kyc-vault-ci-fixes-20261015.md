# KYC vault CI fix verification

`6d2340a`: checked all five runtime pins; updated config and main only.
`ddd8fb8`: NOWAIT locks on users, organizations and kyc_documents before retention check/drop.

Exact CI unit command ran: `DATABASE_URL="sqlite+aiosqlite:///:memory:" pytest tests/unit tests/monitor/test_source_ownership.py -q`.
**1996 passed, 19 failed**, 493 warnings. Failures are limited to macOS Bash 3.2 deployment tests (missing mapfile and unsupported lowercase substitution).
Ownership tests: 6 passed. Downgrade unit contracts: 4 passed. PostgreSQL tests extended but not run; no local PostgreSQL is available. No host/database contact or push.

## Separate deployment failures

- `tests/unit/test_deploy_health.py::test_deploy_dry_run_only_attests_pinned_archive_without_candidate_execution`
- `tests/unit/test_deploy_health.py::test_deploy_dry_run_rejects_candidate_symlink_artifacts[deploy/systemd/runtime-units.manifest]`
- `tests/unit/test_deploy_health.py::test_deploy_dry_run_rejects_candidate_symlink_artifacts[deploy/migration-checkpoints.tsv]`
- `tests/unit/test_deploy_health.py::test_deploy_dry_run_rejects_candidate_symlink_artifacts[deploy/systemd/verdaxis-backend-staging.service]`
- `tests/unit/test_deploy_health.py::test_deploy_dry_run_rejects_candidate_symlink_artifacts[scripts/preflight_runtime.py]`
- `tests/unit/test_deploy_health.py::test_deploy_dry_run_rejects_candidate_symlink_artifacts[scripts/verify_deploy_start_guard.py]`
- `tests/unit/test_deploy_health.py::test_real_deploy_refuses_symbolic_checkpoint_before_source_mutation`
- `tests/unit/test_deploy_health.py::test_real_deploy_refuses_moved_remote_before_source_mutation`
- `tests/unit/test_deploy_health.py::test_real_deploy_refuses_unallowlisted_checkpoint_before_source_mutation`
- `tests/unit/test_deploy_health.py::test_real_deploy_refuses_unexpected_live_revision_before_source_mutation`
- `tests/unit/test_deploy_health.py::test_deploy_dry_run_rejects_malformed_checkpoint_policy`
- `tests/unit/test_deploy_health.py::test_deploy_failure_keeps_selected_code_and_identity_aligned_and_guarded[preflight]`
- `tests/unit/test_deploy_health.py::test_deploy_failure_keeps_selected_code_and_identity_aligned_and_guarded[dependency]`
- `tests/unit/test_deploy_health.py::test_deploy_failure_keeps_selected_code_and_identity_aligned_and_guarded[alembic]`
- `tests/unit/test_deploy_health.py::test_deploy_failure_keeps_selected_code_and_identity_aligned_and_guarded[acl]`
- `tests/unit/test_deploy_health.py::test_failed_post_restart_health_stops_service_and_restores_guard`
- `tests/unit/test_deploy_health.py::test_deploy_lock_serializes_concurrent_runs_and_state_survives_interruption`
- `tests/unit/test_deploy_health.py::test_sigkill_in_restart_window_releases_both_locks_and_closes_authorization`
- `tests/unit/test_deploy_health.py::test_real_deploy_refuses_unconstrained_dependency_install`
