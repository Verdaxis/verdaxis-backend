"""M deploy wiring; executable transaction fixtures live in deploy/tests."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "scripts/deploy.sh").read_text()


def test_m_deploy_never_restarts_the_retained_backend():
    assert 'systemctl restart "$SERVICE_NAME"' not in SOURCE
    assert 'systemctl stop "$SERVICE_NAME"' in SOURCE
    assert 'systemctl disable "$SERVICE_NAME"' in SOURCE


def test_m_deploy_is_exact_sqlx_target_and_classifies_later_features():
    assert 'MIGRATION_TARGET_REVISION" != "sqlx_1"' in SOURCE
    assert SOURCE.count("|sqlx_1)") >= 2
    assert "latest" in SOURCE


def test_migration_and_acls_precede_runtime_identity_publication():
    start = SOURCE.index('# The one approved transition')
    migration = SOURCE.index('migrate run', start)
    acl = SOURCE.index('"$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH"', migration)
    publication = SOURCE.index('write_release_artifact "$CURRENT_SHA"', acl)
    restart = SOURCE.index('systemctl restart "$RUST_UNIT_NAME"', publication)
    assert migration < acl < publication < restart


def test_dry_run_can_bind_exact_current_target_without_executing_candidate():
    dry_run = SOURCE[SOURCE.index('if [[ "$DRY_RUN" == "1" ]]; then'):SOURCE.index('assert_clean_tree "Initial deploy preflight"')]
    assert '"$MIGRATION_EXPECTED_CURRENT_REVISION" "$MIGRATION_TARGET_REVISION"' in dry_run
    assert 'APPROVED_MIGRATION_CURRENT=' in dry_run
    assert 'APPROVED_MIGRATION_TARGET=' in dry_run
    assert 'run_release_binary' not in dry_run


def test_acceptance_is_signed_bound_and_follows_readiness_before_clear():
    assert 'verify_window_acceptance()' in SOURCE
    assert 'openssl pkeyutl -verify -pubin -rawin' in SOURCE
    for field in ('deploy_nonce', 'binary_sha256', 'auth', 'session', 'monitor', 'passive_harm'):
        assert field in SOURCE
    acceptance = SOURCE.rindex('verify_window_acceptance')
    assert SOURCE.rindex('readiness_exact') < acceptance < SOURCE.rindex('clear_deploy_state')


def test_publications_have_before_after_kill_boundaries():
    assert 'publication_boundary()' in SOURCE
    for name in ('source', 'release', 'monitor', 'budget', 'revision', 'runtime', 'binary', 'policy'):
        assert f'publication_boundary before {name}' in SOURCE
        assert f'publication_boundary after {name}' in SOURCE
    assert 'sync -f' in SOURCE


def test_verified_digest_is_carried_to_every_execution_and_promotion():
    assert 'VERIFIED_BINARY_SHA256=' in SOURCE
    assert 'verify_binary_digest "$binary"' in SOURCE
    assert 'verify_binary_digest "$ACTIVE_BINARY.tmp.$$"' in SOURCE
    assert 'verify_binary_digest "$ACTIVE_BINARY"' in SOURCE
    assert 'length == 1 and' in SOURCE


def test_cache_authority_revoke_drain_and_bind_surround_the_transition():
    revoke = SOURCE.index('ops cache-authority revoke')
    source = SOURCE.index('publication_boundary before source')
    migration = SOURCE.index('migrate run', SOURCE.index('# The one approved transition'))
    activate = SOURCE.index('ops cache-authority activate')
    restart = SOURCE.index('systemctl restart "$RUST_UNIT_NAME"', activate)
    assert revoke < source < migration < activate < restart


def test_failed_m_deploy_has_no_serving_python_fallback():
    failure = SOURCE[SOURCE.index('preserve_failed_release()'):SOURCE.index('trap preserve_failed_release EXIT')]
    assert 'exact-ready backend service remains serving' not in failure
    assert 'systemctl stop "$RUST_UNIT_NAME"' in failure
    assert 'write_deploy_state "blocked"' in failure
