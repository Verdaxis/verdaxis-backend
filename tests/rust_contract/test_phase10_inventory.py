"""Phase 10 source inventory. Parse source as data; never import the application."""

import ast
import copy
import hashlib
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase10"
METHODS = {"get", "post", "put", "patch", "delete", "head", "options"}
UNIT_ROOTS = ("deploy/systemd", "deploy/monitor", "deploy/external_monitor/systemd")
HELPERS = {
    "verify_migration_revision.py", "preflight_runtime.py", "apply_migration_checkpoint.py",
    "converge_runtime_acls.py", "verify_deploy_start_guard.py", "validate_health_response.py",
    "verify_systemd_source.py", "verify_rust_artifact.py", "prepare_rust_budget.py",
    "prepare_rust_runtime.py", "verify_rust_start.py", "rust_cache_authority.py",
    "rust_caddy_candidate.py", "rust_passive_no_harm.py",
}


def document(name):
    path = FIXTURES / f"{name}.json"
    assert path.is_file(), f"missing disposition/evidence inventory: {name}"
    return json.loads(path.read_text())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def literal(node):
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.Attribute) and ast.unparse(node) == "settings.API_V1_STR":
        return "/api"
    raise AssertionError(f"unresolved static route expression: {ast.unparse(node)}")


def keyword(call, name, default=""):
    return next((literal(item.value) for item in call.keywords if item.arg == name), default)


def source_routes(root=ROOT):
    """Resolve only routers actually mounted by main, including call-style registrations."""
    main_path = root / "app/main.py"
    main = ast.parse(main_path.read_text())
    imports = {
        alias.asname or alias.name: item.module.replace(".", "/") + ".py"
        for item in main.body if isinstance(item, ast.ImportFrom)
        and item.module and item.module.startswith("app.routers.")
        for alias in item.names if alias.name == "router"
    }
    mounts = []
    for node in ast.walk(main):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            if ast.unparse(node.func) == "app.include_router":
                mounts.append((imports[node.args[0].id], keyword(node, "prefix")))
    assert len(mounts) == 38, "mounted router inventory changed"
    result = []
    for source, mount_prefix in [("app/main.py", ""), *mounts]:
        tree = ast.parse((root / source).read_text())
        router_prefix = ""
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and ast.unparse(node.func) == "APIRouter":
                router_prefix = keyword(node, "prefix")
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
                continue
            owner = ast.unparse(node.func.value)
            if owner not in {"router", "app"} or node.func.attr not in METHODS:
                continue
            path = mount_prefix + router_prefix + literal(node.args[0])
            method = node.func.attr.upper()
            result.append({"id": f"route.{method} {path}", "source": source,
                           "method": method, "path": path, "line": node.lineno})
    return sorted(result, key=lambda row: row["id"])


def source_units(root=ROOT):
    return sorted(str(path.relative_to(root)) for base in UNIT_ROOTS
                  for path in (root / base).iterdir() if path.suffix in {".service", ".timer"})


def source_scripts(root=ROOT):
    return sorted(str(path.relative_to(root)) for path in (root / "scripts").iterdir()
                  if path.suffix in {".py", ".sh"})


def unique_rows(rows):
    indexed = {row["id"]: row for row in rows}
    assert len(indexed) == len(rows), "duplicate artifact id"
    return indexed


def validate_dispositions(rows, scenarios):
    unique_rows(rows)
    for row in rows:
        assert row["owner"].strip(), f"ownerless: {row['id']}"
        assert row["end_state"] in {"rust", "shell", "gone", "retired-offhost"}
        assert row["status"] in {"planned", "accepted"}
        assert row["source_sha256"] and len(row["source_sha256"]) == 64
        assert row["rollback"] and row["installed_check"]
        assert set(row["evidence"]) <= scenarios, f"unknown scenario: {row['id']}"
        if row["end_state"] in {"rust", "shell"}:
            assert row["evidence"], f"untested: {row['id']}"
        elif row["end_state"] == "gone":
            assert row["deleting_task"] in {20, 26, 32}
            assert row["last_reference_change"], f"no removal seam: {row['id']}"
        else:
            assert row["offhost_location"]


def validate_source(snapshot, ledger, root=ROOT):
    """Keep the snapshot after deletion; require explicit accepted removal evidence."""
    indexed = unique_rows(ledger["artifacts"])
    for row in snapshot:
        item = indexed[row["id"]]
        path = root / row["source"]
        if ledger["phase"] == "pre-removal":
            assert path.is_file(), f"missing source before removal: {path}"
            assert digest(path) == row["source_sha256"], f"source drift: {path}"
        else:
            assert ledger["phase"] == "post-removal"
            assert item["status"] == "accepted"
            proof = item.get("deletion_evidence")
            assert proof, f"missing deletion evidence: {row['id']}"
            evidence = root / proof["path"]
            assert evidence.is_file() and digest(evidence) == proof["sha256"]
            if item["end_state"] == "gone":
                assert not path.exists(), f"deleted artifact remains: {path}"
                for reference in item.get("reference_tokens", [row["source"]]):
                    for base in ("deploy",):
                        for candidate in (root / base).rglob("*"):
                            if candidate.is_file():
                                assert reference not in candidate.read_text(errors="replace"), (
                                    f"removed artifact still referenced: {candidate}"
                                )


def test_phase10_routes_are_exactly_188():
    rows = document("routes")["artifacts"]
    assert len(rows) == 188, "missing route disposition rows"
    assert len(unique_rows(rows)) == 188
    assert sum(row["source"] == "app/main.py" for row in rows) == 4
    assert sum(row["source"] == "app/routers/alerts.py" for row in rows) == 3
    ledger = document("removal-ledger")
    if ledger["phase"] == "pre-removal":
        expected = source_routes()
        assert [{key: row[key] for key in expected[0]} for row in rows] == expected
    assert sum(row["deleting_task"] == 20 for row in rows) == 20
    validate_source(rows, ledger)


def test_phase10_units_are_exactly_41():
    rows = document("units")["artifacts"]
    assert len(rows) == 41, "missing unit disposition rows"
    assert [sum(row["source"].startswith(base + "/") for row in rows)
            for base in UNIT_ROOTS] == [20, 11, 10]
    ledger = document("removal-ledger")
    if ledger["phase"] == "pre-removal":
        assert sorted(row["source"] for row in rows) == source_units()
    validate_source(rows, ledger)


def test_phase10_scripts_are_exactly_40():
    rows = document("scripts")["artifacts"]
    assert len(rows) == 40, "missing script disposition rows"
    assert sum(row["source"].endswith(".py") for row in rows) == 35
    ledger = document("removal-ledger")
    if ledger["phase"] == "pre-removal":
        assert sorted(row["source"] for row in rows) == source_scripts()
    validate_source(rows, ledger)


def test_phase10_helpers_are_exactly_14():
    rows = [row for row in document("removal-ledger")["artifacts"] if row["kind"] == "helper"]
    assert len(rows) == 14, "missing helper disposition rows"
    assert {Path(row["source"]).name for row in rows} == HELPERS


def test_phase10_no_row_is_ownerless_or_untested():
    ledger = document("removal-ledger")
    manifest = document("evidence-manifest")
    validate_inventory_contract(ledger, manifest)
    indexed = unique_rows(ledger["artifacts"])
    for name in ("routes", "units", "scripts"):
        for row in document(name)["artifacts"]:
            assert indexed[row["id"]] == row
    assert sum(row["kind"] == "cli" for row in indexed.values()) == 5
    validate_source(list(indexed.values()), ledger)
    assert manifest["phase7_decision"] == "retire-routes-preserve-tables"
    assert manifest["results"] == [], "Task 1 is an evidence plan, not gate acceptance"


def test_phase10_inventory_rejects_missing_and_duplicate_dispositions():
    rows = document("removal-ledger")["artifacts"]
    assert rows, "missing disposition rows"
    with pytest.raises(AssertionError, match="duplicate"):
        unique_rows([rows[0], rows[0]])
    broken = dict(rows[0], owner="")
    with pytest.raises(AssertionError, match="ownerless"):
        validate_dispositions([broken], set())


def test_phase10_post_removal_requires_evidence(tmp_path):
    row = {"id": "script.example", "source": "scripts/example.py", "source_sha256": "0" * 64}
    ledger = {"phase": "post-removal", "artifacts": [dict(row, status="accepted", end_state="gone")]}
    with pytest.raises(AssertionError, match="missing deletion evidence"):
        validate_source([row], ledger, tmp_path)


# Reviewed plan test identifiers, grouped by owner task; not parsed from evidence data.
REQUIRED_SCENARIOS_BY_TASK = {
    1: [
        "test_phase10_helpers_are_exactly_14",
        "test_phase10_no_row_is_ownerless_or_untested",
        "test_phase10_routes_are_exactly_188",
        "test_phase10_scripts_are_exactly_40",
        "test_phase10_units_are_exactly_41",
    ],
    2: [
        "dispatch_never_panics_on_bad_utf8_arguments",
        "every_command_has_help_and_no_hidden_flag",
        "legacy_bind_form_still_serves",
        "subcommand_dispatch_is_exact",
        "version_json_is_still_first",
    ],
    3: [
        "job_preflight_refuses_dangling_state_and_live_deploy",
        "mail_transport_logs_recipient_digest_never_address",
        "retry_stops_after_three_attempts",
        "state_lock_is_nonblocking",
        "state_reader_refuses_oversize_symlink_and_invalid_json",
        "target_attestation_rejects_wrong_database_and_role",
        "write_state_is_atomic_and_0600",
    ],
    4: [
        "deploy_guard_allows_absent_state",
        "deploy_guard_rejects_dangling_state_and_wrong_sha",
        "deploy_guard_requires_both_locks_for_restart_authorization",
        "guard_scripts_contain_no_python",
        "health_guard_accepts_exactly_four_keys",
        "rust_start_requires_regular_budget_and_revision_files",
    ],
    5: [
        "baseline_contains_reference_rows_and_no_application_rows",
        "baseline_dump_has_no_owner_or_privilege_clauses",
        "baseline_fingerprint_matches_the_committed_artifact",
        "baseline_restores_into_an_empty_database",
        "fingerprint_is_stable_and_changes_on_a_new_column",
    ],
    6: [
        "adoption_checksum_matches_sqlx",
        "adoption_commit_then_interruption_resumes_each_publication_boundary",
        "adoption_row_matches_sqlx_created_table",
        "app_role_is_denied_sqlx_control_table",
        "backup_reader_can_dump_and_restore_sqlx_history_but_cannot_write_it",
        "baseline_refuses_wrong_head_fingerprint_or_foreign_failed_history",
        "deploy_owned_requires_exact_state_nonce_sha_and_both_locks",
        "exact_adoption_resume_is_read_only_and_idempotent",
        "migrate_and_app_request_do_not_half_apply",
        "migrate_status_is_read_only_and_reports_pending",
        "production_literal_and_allowlist_move_together",
        "run_to_exact_approved_target_is_idempotent",
        "sqlx_control_acl_converges_twice_with_migrator_only_writes",
        "unapproved_transition_or_pending_version_refuses_before_mutation",
    ],
    7: [
        "acl_convergence_runs_between_migrate_and_restart",
        "deploy_owned_migration_accepts_only_its_blocked_state_and_held_locks",
        "deploy_script_has_no_python_or_pip_reference",
        "dry_run_real_run_and_postcheck_bind_the_same_exact_transition",
        "failure_after_migrate_leaves_durable_blocked_state",
        "interruption_at_each_state_identity_revision_and_binary_publication_fails_closed",
        "main_and_restart_locks_precede_authorization",
        "migrate_runs_before_any_restart",
        "readiness_requires_the_exact_four_key_body",
        "staged_binary_is_the_only_artifact_executed_before_install",
    ],
    8: [
        "artifact_verifier_rejects_tampering",
        "duplicate_nonregular_link_and_device_members_are_refused",
        "extra_member_and_path_traversal_are_refused",
        "failure_leaves_the_output_directory_empty",
        "metadata_policy_allowlist_and_each_member_digest_are_bound",
        "oversize_member_and_member_count_are_refused",
        "packaged_attested_switch_artifact_passes_without_candidate_execution",
        "shellcheck_job_pins_version_digest_and_checks_every_reviewed_script",
        "symlink_output_and_extraction_substitution_are_refused",
        "tampered_archive_bit_flip_is_refused",
        "valid_fixture_extracts_only_allowlisted_members",
        "wrong_public_key_and_wrong_source_sha_are_refused",
        "wrong_repository_workflow_path_ref_source_ref_and_target_are_refused",
    ],
    9: [
        "auth_and_reminder_envs_have_mail_but_no_jwt_signing_key",
        "budget_matches_configured_authority_not_measured_load",
        "migrator_url_is_only_in_the_migrate_invocation",
        "news_and_prune_envs_have_no_mail_jwt_or_migrator_key",
        "preflight_rejects_wrong_database_role_and_environment",
        "runtime_files_are_regular_0600_and_never_symlinks",
        "serve_env_contains_only_reviewed_http_credentials",
        "serve_env_has_no_migrator_or_job_only_key",
        "serve_env_requires_auth_keys_and_issues_and_verifies_sessions",
    ],
    10: [
        "every_unit_exec_start_is_the_release_binary_or_a_shell_guard",
        "installer_dispatcher_effective_acl_survives_producer_restart_and_boot",
        "installer_enforces_per_check_handoff_acl_and_identity_boundary",
        "installer_refuses_a_manifest_digest_mismatch",
        "installer_restart_and_revert_preserve_admitted_alert_state",
        "job_units_load_only_their_purpose_env",
        "manifest_counts_are_16_canonical_11_local_8_external",
        "monitor_units_exec_the_digest_pinned_host_copy",
        "no_unit_contains_python_or_a_venv_path",
        "old_unit_can_start_the_new_binary",
        "timer_catch_up_after_restart_runs_once",
    ],
    11: [
        "news_falls_back_to_rss_when_webz_is_absent",
        "news_inserts_only_new_urls",
        "news_lock_busy_is_a_successful_noop",
        "news_overlap_inserts_once",
        "news_rejects_oversize_provider_body_and_feed",
    ],
    12: [
        "approval_mail_claim_is_transition_scoped",
        "approval_mail_failure_defers_one_hour",
        "approval_mail_never_holds_a_user_lock_across_provider_io",
        "approval_mail_queue_is_ordered_due_and_bounded",
        "cleanup_and_approval_claim_do_not_double_send",
        "cleanup_deletes_expired_refresh_sessions_without_successor",
        "cleanup_does_not_clear_a_token_renewed_under_lock",
    ],
    13: [
        "prune_is_idempotent_on_the_same_day",
        "prune_keeps_exactly_800_utc_dates",
        "prune_never_deletes_status_transitions",
        "prune_refuses_wrong_environment_or_sha",
    ],
    14: [
        "expiry_batch_is_at_most_twenty",
        "expiry_mail_discards_changed_owner_and_past_deadline",
        "expiry_mail_lease_is_exclusive",
        "expiry_payload_preserves_all_order_and_provider_keys",
        "expiry_scan_and_claim_yield_one_send",
        "reminders_refuse_nonproduction_before_pool",
        "reminders_scan_is_bounded_and_locks_the_order",
    ],
    15: [
        "kyc_claim_is_permanent_after_crash",
        "kyc_claim_is_permanent_and_not_retried",
        "kyc_gate_disabled_exits_before_pool",
        "kyc_live_send_requires_separate_founder_authorization",
        "kyc_notice_keeps_approved_unbound_and_skips_approved_bound_without_dependency",
        "kyc_notice_pages_are_bounded",
        "kyc_notice_sends_to_the_grace_user_not_org_admins",
        "kyc_notice_skips_changed_organization",
        "kyc_notice_skips_expired_grace",
        "kyc_notice_skips_rejected_and_keeps_eligible_bound_users",
    ],
    16: [
        "onboarding_bootstrap_is_silent",
        "onboarding_handover_drains_and_fences_unlocked_python",
        "onboarding_reads_python_v1_state_without_alerting",
        "onboarding_repeated_crashes_can_repeat_delivery",
        "onboarding_second_process_exits_without_alert",
        "onboarding_stable_lock_busy_exits_without_send",
        "onboarding_state_is_atomic_0600_and_schema_v1",
        "onboarding_suppresses_same_stage_and_alerts_on_change",
        "onboarding_three_processes_remain_exclusive_across_state_rename",
    ],
    17: [
        "demo_coverage_and_order_write_do_not_deadlock",
        "demo_coverage_preserves_order_identity_and_updates_only_managed_fields",
        "demo_never_creates_kyc_or_synthetic_lng_prices",
        "demo_phases_are_four_separate_transactions",
        "demo_prune_respects_customer_pins_and_history",
        "demo_requires_the_exact_pair_and_one_canonical_match",
        "demo_tick_is_idempotent",
    ],
    18: [
        "health_503_body_has_no_provenance",
        "health_routes_keep_the_exact_four_key_body",
        "root_does_not_touch_the_database",
        "root_health_snippet_has_one_argument_and_exact_paths",
        "root_is_get_only",
        "root_returns_the_exact_python_body",
    ],
    19: [
        "canary_cli_requires_apply_and_the_route_uses_the_same_function",
        "canary_concurrent_cleanup_deletes_once",
        "canary_deletes_the_organization_only_when_it_has_no_members_and_matches_the_domain",
        "canary_deletes_the_user_before_the_organization",
        "canary_rejects_a_malformed_or_non_canary_email_with_422",
        "canary_repeat_returns_zeros",
        "canary_retained_kyc_documents_block_both_deletions_with_409",
        "canary_token_unset_returns_404_without_opening_a_transaction",
        "canary_wrong_token_returns_401_and_the_token_never_appears_in_output",
        "serve_env_carries_the_monitor_token_only_when_configured",
    ],
    20: [
        "phase7_decision_names_exactly_one_path",
        "phase7_exact_twenty_operations_have_accepted_ownership",
        "phase7_path_b_exact_matchers_do_not_steal_adjacent_routes",
        "phase7_path_b_preserves_all_tables_and_rows",
        "phase7_retirement_exact_operations_return_404",
        "phase7_retirement_preserves_table_and_row_identity",
    ],
    21: [
        "operator_acl_grants_only_ingestion_objects",
        "operator_role_is_exact_and_not_a_member_of_the_app_role",
        "signal_csv_headers_are_exact_per_family",
        "signal_dry_run_writes_nothing_and_reports_would_insert",
        "signal_duplicate_source_event_id_is_skipped_not_failed",
        "signal_ingest_refuses_production_and_any_unattested_target",
        "signal_ingest_uses_operator_role_and_app_role_is_denied",
        "signal_output_contains_counts_but_no_row_contents",
        "signal_row_error_rolls_back_only_that_row",
        "signal_source_pattern_and_file_size_are_bounded",
    ],
    22: [
        "gena_allow_blank_id_is_explicit_and_counted",
        "gena_blank_id_is_refused_by_default",
        "gena_concurrent_import_keeps_one_row",
        "gena_dry_run_writes_nothing",
        "gena_file_and_row_bounds_are_enforced",
        "gena_invalid_coordinate_or_missing_required_text_fails_before_any_write",
        "gena_operator_grant_is_exact_and_app_role_stays_read_only",
        "gena_same_id_replay_inserts_once",
        "gena_unknown_status_defaults_to_announced",
        "gena_zero_coordinate_is_preserved",
    ],
    23: [
        "inspect_blockers_exits_2_when_any_count_is_nonzero",
        "inspect_blockers_matches_the_python_counts_on_the_same_fixture",
        "inspect_json_is_one_object_with_stable_keys",
        "inspect_users_is_read_only_and_never_writes_a_status_file",
        "inspect_users_masks_email_by_default",
        "inspect_users_show_emails_requires_the_explicit_flag",
    ],
    24: [
        "cache_activate_revokes_then_binds_the_row_then_publishes",
        "cache_activate_uses_the_migrator_role_only",
        "cache_failure_between_steps_leaves_the_record_revoked",
        "cache_lease_and_fence_match_the_reader",
        "cache_record_is_bounded_atomic_and_0600",
        "cache_revision_is_monotonic_and_rejects_out_of_range_values",
        "cache_revoke_publishes_revoked_before_the_drain_wait",
        "cache_status_never_prints_a_full_identity",
    ],
    25: [
        "fleet_disposition_record_is_complete_and_bound_to_green_w0",
        "fleet_disposition_requires_installed_evidence",
        "fleet_installed_dependency_is_classified",
        "fleet_offhost_is_not_installed_or_ported",
        "fleet_retained_tool_has_no_deleted_package_dependency",
    ],
    26: [
        "no_unit_timer_cron_or_deploy_references_a_retired_tool",
        "offhost_rows_name_a_tool_a_reason_and_a_location",
        "phase0_and_benchmark_tools_are_ci_or_offhost_only",
        "retired_tools_are_absent_from_the_release_artifact",
        "seed_and_remediation_tools_are_not_installed_by_any_script",
    ],
    27: [
        "backup_ignores_unlisted_future_and_duplicate_generations",
        "backup_rejects_an_oversize_uncompressed_artifact",
        "backup_status_is_private_and_key_compatible",
        "backup_streams_gzip_within_the_128_mib_unit_limit",
        "backup_verifies_metadata_and_the_newest_listed_artifacts",
        "health_quarantines_a_malformed_previous_status",
        "health_rejects_a_zero_or_short_release_sha",
        "health_status_keys_match_the_python_document",
        "health_total_timeout_and_body_bound_are_enforced",
        "health_validates_the_exact_identity_and_readiness_keys",
        "health_works_with_the_database_down",
    ],
    28: [
        "alert_acknowledgement_before_remove_survives_restart_and_revert",
        "alert_crash_resume_retains_head_and_confirmed_destination_progress",
        "alert_delivery_lock_timeout_retains_admitted_head",
        "alert_different_checks_do_not_share_a_global_lock",
        "alert_dispatch_serializes_events",
        "alert_enqueue_is_durable_idempotent_and_bounded_before_status_publication",
        "alert_exhaustion_or_uncertain_send_blocks_later_sequence_until_reconciled",
        "alert_failed_enqueue_has_failed_unit_and_no_status_publication",
        "alert_full_journal_refuses_before_admission_then_converges",
        "alert_handoff_acl_modes_and_dispatcher_read_remove_only",
        "alert_handoff_producers_cannot_read_other_check_receipts_or_credentials",
        "alert_handoff_real_identities_enqueue_and_dispatch",
        "alert_healthy_wakeup_before_failure_retry_cannot_overtake",
        "alert_journal_lock_timeout_refuses_before_admission_then_converges",
        "alert_never_prints_a_token_or_chat_id",
        "alert_producer_death_before_commit_then_changed_observation_converges",
        "alert_python_direct_invocation_is_fenced_until_native_journal_drained",
        "alert_python_rust_same_check_handover_preserves_failure_then_healthy",
        "alert_receipts_prevent_duplicate_delivery_per_destination",
        "alert_recovery_after_final_name_rename_completes_fsync_and_bookkeeping",
        "alert_reminder_respects_the_hourly_repeat",
        "alert_state_is_atomic_and_schema_compatible",
        "alert_telegram_and_healthchecks_failures_are_independent",
        "outbox_output_never_contains_the_dsn",
        "outbox_probe_exit_codes_and_thresholds_match_python",
        "outbox_query_is_bounded_read_only_and_times_out",
    ],
    29: [
        "external_checks_are_bounded_one_request_each_and_isolated",
        "external_monitor_alerts_on_producer_exit_4_with_healthy_status",
        "external_monitor_checks_route_uptime_database_outbox_and_backup",
        "external_monitor_classifies_vercel_challenges_without_an_alert",
        "external_monitor_expensive_checks_are_cached_for_thirty_minutes",
        "external_monitor_never_shells_out_with_interpolation",
        "external_monitor_redacts_bodies_secrets_and_headers",
        "external_monitor_signup_canary_creates_then_cleans_up",
        "external_monitor_state_and_status_keep_existing_keys",
        "external_monitor_telegram_disabled_suppresses_alerts",
    ],
    30: [
        "recover_attempts_once_per_fingerprint_and_cooldown",
        "recover_dry_run_changes_nothing",
        "recover_lock_is_nonblocking",
        "recover_may_start_the_host_owned_backup_unit",
        "recover_never_starts_or_stops_a_python_unit",
        "recover_output_is_redacted_and_bounded",
        "recover_refuses_an_active_deploy_or_unhealthy_database",
        "recover_runs_only_allowlisted_commands",
        "recover_validates_caddy_before_reload_or_restart",
    ],
    31: [
        "every_required_scenario_has_evidence",
        "host_inventory_covers_units_timers_dropins_cron_usr_local_backup_and_processes",
        "host_inventory_is_complete_and_signed",
        "nightly_backup_and_staging_pull_schedules_are_unchanged",
        "no_python_process_remains_after_the_rehearsal",
        "rehearsal_deletes_python_and_nothing_restarts",
        "rehearsal_rollback_is_one_change_per_group",
        "rehearsal_runs_every_installed_unit_once",
        "release_bundle_rollback_starts_the_actual_pre_m_binary",
        "rollback_interruption_at_each_publication_never_starts_a_mixed_bundle",
    ],
    32: [
        "alembic_history_is_inert_and_unreferenced",
        "ci_harness_does_not_import_the_deleted_application",
        "daily_backup_and_staging_pull_still_run",
        "host_python_cleanup_removes_only_verdaxis_owned_paths",
        "no_caddy_upstream_or_route_points_at_python",
        "no_installed_reference_to_a_deleted_python_path",
        "old_timer_and_new_job_do_not_duplicate",
        "phase7_routes_have_their_recorded_end_state",
        "previous_release_artifact_still_starts",
        "python_application_tree_is_absent",
        "python_tests_that_import_app_are_removed_or_offhost",
        "staging_is_cleaned_before_production",
    ],
    33: [
        "backup_and_staging_pull_schedules_are_unchanged",
        "deleted_paths_are_absent_on_both_hosts",
        "independent_review_findings_are_closed",
        "no_python_process_unit_timer_or_cron_on_either_host",
        "passive_harm_checks_are_clean_for_each_group",
        "removal_ledger_has_no_gate_or_ownerless_rows",
        "the_rust_backup_verify_monitor_accepts_the_nightly_artifact",
    ],
    34: [
        "measure_artifact_is_report_only",
        "measure_extracts_digest_pinned_python_memory_and_computes_absolute_and_percentage_differences",
        "measure_marks_the_below_floor_caveat",
        "measure_memory_comparison_excludes_warmup_and_ramp",
        "measure_missing_comparable_python_memory_reports_unavailable_without_a_run",
        "measure_never_changes_the_service_or_deploy_state",
        "measure_refuses_without_the_pinned_seed",
        "measure_rejects_memory_digest_metric_scope_window_and_worker_mismatch",
        "measure_replays_the_exact_schedule_and_route_mix",
        "measure_reports_5xx_503_error_rate_observations_and_peak_rss",
        "measure_reports_per_route_and_weighted_p95",
        "measurement_runs_after_cutover_only",
    ],
    35: [
        "fleet_offhost_has_no_removed_package_dependency",
        "w0_classification_does_not_create_a_schedule",
        "w0_covers_both_hosts_and_host_owned_automation",
        "w0_missing_dependency_is_refused",
    ],
    36: [
        "backup_restore_preserves_inventory_and_row_counts",
        "host_replacements_preserve_backup_and_staging_pull",
        "only_superseded_exact_cron_entries_are_removed",
        "owner_acceptance_covers_each_required_replacement",
        "required_verdaxis_python_has_no_exception",
        "staging_pull_preserves_target_identity_and_schedule",
    ],
}
REQUIRED_SCENARIOS = {name for names in REQUIRED_SCENARIOS_BY_TASK.values() for name in names}

ROUND4_REQUIREMENTS = {
    "alert_recovery_after_final_name_rename_completes_fsync_and_bookkeeping": {
        "owner_task": 28,
        "plan_requirement": "§22 R4-1; Task 28, PM acceptance",
        "required_result": "Kill before rename, after final-name rename before directory fsync, and after fsync before bookkeeping. Under the journal lock, recover any valid final-name entry by completing fsync and cursor bookkeeping before delivery. In-doubt recovery yields no entry or one ordered entry, never duplicate admission or out-of-order delivery.",
    },
    "installer_dispatcher_effective_acl_survives_producer_restart_and_boot": {
        "owner_task": 10,
        "plan_requirement": "§22 R4-2; Task 10, PM acceptance",
        "required_result": "For both producer units, prove effective ACLs after stop/start and boot ordering: dispatcher can traverse, lock, read and remove an admitted entry but cannot modify contents, cursor or status. Owning-group and other entries stay empty; no shared producer membership or unrelated access.",
    },
    "external_monitor_alerts_on_producer_exit_4_with_healthy_status": {
        "owner_task": 29,
        "plan_requirement": "§22 R4-3; Tasks 28 and 29, PM acceptance",
        "required_result": "For both producer units, journal-lock timeout on a healthy observation exits 4 with no admitted entry and unchanged healthy status. The bounded failed-unit check on the external monitor main path alerts using unit state only; it neither reads producer credentials nor enqueues the refused observation nor changes failure to success.",
    },
}


SUPPLEMENTAL_GROUPS = {
    'mounts': {
        'artifact.app/main.py': 'app/main.py',
    },
    'launchers': {
        'artifact.app/runtime_server.py': 'app/runtime_server.py',
    },
    'local-monitor': {
        'artifact.deploy/monitor/alert_dispatch.py': 'deploy/monitor/alert_dispatch.py',
        'artifact.deploy/monitor/backup_verify.py': 'deploy/monitor/backup_verify.py',
        'artifact.deploy/monitor/legacy_retirement.py': 'deploy/monitor/legacy_retirement.py',
        'artifact.deploy/monitor/local_health_check.py': 'deploy/monitor/local_health_check.py',
        'artifact.deploy/monitor/outbox_backlog_probe.py': 'deploy/monitor/outbox_backlog_probe.py',
        'artifact.deploy/monitor/status_state.py': 'deploy/monitor/status_state.py',
        'artifact.deploy/monitor/verify_runtime_identity.py': 'deploy/monitor/verify_runtime_identity.py',
    },
    'external-monitor': {
        'artifact.deploy/external_monitor/verdaxis_autodiag.py': 'deploy/external_monitor/verdaxis_autodiag.py',
        'artifact.deploy/external_monitor/verdaxis_monitor.py': 'deploy/external_monitor/verdaxis_monitor.py',
        'artifact.deploy/external_monitor/verdaxis_recover.py': 'deploy/external_monitor/verdaxis_recover.py',
    },
    'cli': {
        'cli.auth_maintenance': 'app/cli/auth_maintenance.py',
        'cli.onboarding_attention': 'app/cli/onboarding_attention.py',
        'cli.order_expiry_reminders': 'app/cli/order_expiry_reminders.py',
        'cli.refresh_news': 'app/cli/refresh_news.py',
        'cli.send_kyc_grace_notices': 'app/cli/send_kyc_grace_notices.py',
    },
}

# Freeze source identity after removal too; dispositions and result records can change.
PINNED_SOURCE_IDENTITIES = '54cb056ee78c7790a476696aee11575b7814057f25649688e221048c81d53e44'


def validate_inventory_contract(ledger, manifest):
    """Validate the immutable source and required-evidence contract, even after removal."""
    scenarios = manifest["required_scenarios"]
    assert len(scenarios) == len(REQUIRED_SCENARIOS) and set(scenarios) == REQUIRED_SCENARIOS, (
        "required scenario membership changed"
    )
    catalog = manifest.get("scenario_catalog", [])
    assert len(catalog) == len(REQUIRED_SCENARIOS) and {cell["id"] for cell in catalog} == REQUIRED_SCENARIOS, (
        "required scenario membership changed in catalog"
    )
    for cell in catalog:
        assert cell["id"] in REQUIRED_SCENARIOS_BY_TASK[cell["owner_task"]]
        assert cell["plan_requirement"].strip() and cell["required_result"].strip()
    for name, requirement in ROUND4_REQUIREMENTS.items():
        assert next(cell for cell in catalog if cell["id"] == name) == dict(id=name, **requirement)
    rows = ledger["artifacts"]
    unique_rows(rows)
    identities = sorted((row["id"], row["source"], row["source_sha256"]) for row in rows)
    actual_digest = hashlib.sha256(json.dumps(identities, separators=(",", ":")).encode()).hexdigest()
    assert actual_digest == PINNED_SOURCE_IDENTITIES, "source membership or immutable identity changed"
    for group in SUPPLEMENTAL_GROUPS.values():
        assert {row["id"]: row["source"] for row in rows if row["id"] in group} == group, (
            "source membership changed in supplemental group"
        )
    validate_dispositions(rows, set(scenarios))


def test_phase10_required_scenarios_match_the_reviewed_catalog():
    manifest = document("evidence-manifest")
    assert set(manifest["required_scenarios"]) == REQUIRED_SCENARIOS
    catalog = manifest.get("scenario_catalog", [])
    assert {cell["id"] for cell in catalog} == REQUIRED_SCENARIOS
    assert len(catalog) == len(REQUIRED_SCENARIOS)
    for cell in catalog:
        assert cell["owner_task"] in REQUIRED_SCENARIOS_BY_TASK
        assert cell["id"] in REQUIRED_SCENARIOS_BY_TASK[cell["owner_task"]]
        assert cell["plan_requirement"].strip()
        assert cell["required_result"].strip()
    for name, requirement in ROUND4_REQUIREMENTS.items():
        assert next(cell for cell in catalog if cell["id"] == name) == dict(id=name, **requirement)
    validate_inventory_contract(document("removal-ledger"), manifest)


@pytest.mark.parametrize("phase", ["pre-removal", "post-removal"])
@pytest.mark.parametrize("group", SUPPLEMENTAL_GROUPS)
@pytest.mark.parametrize("mutation", ["delete", "substitute", "extra"])
def test_phase10_supplemental_membership_is_exact(phase, group, mutation):
    ledger = copy.deepcopy(document("removal-ledger"))
    ledger["phase"] = phase
    selected = next(row for row in ledger["artifacts"] if row["id"] in SUPPLEMENTAL_GROUPS[group])
    if mutation == "delete":
        ledger["artifacts"].remove(selected)
    elif mutation == "substitute":
        selected["id"] += ".replacement"
    else:
        ledger["artifacts"].append(dict(selected, id=selected["id"] + ".extra"))
    with pytest.raises(AssertionError, match="source membership"):
        validate_inventory_contract(ledger, document("evidence-manifest"))


@pytest.mark.parametrize("scenario", sorted(REQUIRED_SCENARIOS))
@pytest.mark.parametrize("mutation", ["delete", "rename"])
def test_phase10_every_required_scenario_is_pinned(scenario, mutation):
    manifest = copy.deepcopy(document("evidence-manifest"))
    # RED uses the current manifest, which also has missing cells. Every mutation
    # must be rejected, including unreferenced command and Path B retirement cells.
    manifest["required_scenarios"] = [name for name in manifest["required_scenarios"] if name != scenario]
    if mutation == "rename":
        manifest["required_scenarios"].append(scenario + "_renamed")
    with pytest.raises(AssertionError, match="required scenario membership"):
        validate_inventory_contract(document("removal-ledger"), manifest)


def test_phase10_incidental_tokens_are_not_scenarios():
    incidental = {"db", "environment", "flock", "gone", "message", "release_sha",
                  "rust", "sent_at", "shell", "status", "version"}
    assert not incidental.intersection(document("evidence-manifest")["required_scenarios"])
