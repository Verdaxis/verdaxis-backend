"""Real filesystem and process proof for the separate Phase 0 start fence."""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SHA = "a" * 40


def test_fence_module_exists():
    assert (ROOT / "scripts/phase0/start_fence.py").is_file()


def test_assertion_module_exists():
    assert (ROOT / "scripts/phase0/assert_host.py").is_file()


def test_outbox_drain_before_final_snapshot():
    from scripts.phase0.assert_host import require_outbox_drained

    # The first zero is not evidence: a producer has not committed yet.
    with pytest.raises(ValueError):
        require_outbox_drained(producers_stopped=False, sequencer_stopped=False,
                               pending_jobs=0, app_sessions=1, undispatched=0)
    with pytest.raises(ValueError):
        require_outbox_drained(producers_stopped=True, sequencer_stopped=False,
                               pending_jobs=0, app_sessions=1, undispatched=1)
    with pytest.raises(ValueError):
        require_outbox_drained(producers_stopped=True, sequencer_stopped=True,
                               pending_jobs=0, app_sessions=0, undispatched=1)
    require_outbox_drained(producers_stopped=True, sequencer_stopped=True,
                           pending_jobs=0, app_sessions=0, undispatched=0)


@pytest.mark.parametrize("boundary", ["tuple", "units", "restart", "readiness"])
def test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked(tmp_path, boundary):
    from scripts.phase0.start_fence import start_is_allowed

    control = tmp_path / "control"
    control.mkdir(mode=0o700)
    (control / "fence").write_text("phase0\n")
    (control / "fence").chmod(0o600)
    (control / "operator.lock").touch(mode=0o600)
    tuple_root = tmp_path / "be"
    tuple_root.mkdir()
    (tuple_root / "identity").write_text("old")
    candidate = tmp_path / "candidate"
    candidate.mkdir()
    (candidate / "identity").write_text("new")
    # Child holds the actual advisory lock and publishes a permit. Termination
    # must revoke authority even when the permit remains on disk.
    code = '''
import fcntl, json, os, pathlib, sys
control, root, candidate, boundary, sha = map(str, sys.argv[1:])
control, root, candidate = map(pathlib.Path, (control, root, candidate))
with (control / 'operator.lock').open('r+') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    st = os.fstat(lock.fileno())
    permit = dict(environment='production', release_sha=sha, database='verdaxis_prod',
                  lock_device=st.st_dev, lock_inode=st.st_ino)
    temporary = control / 'permit.tmp'
    temporary.write_text(json.dumps(permit))
    temporary.chmod(0o600)
    os.replace(temporary, control / 'permit.json')
    os.rename(root, root.with_name('rollback'))
    os.rename(candidate, root)
    if boundary != 'tuple':
        temporary = root / 'unit.tmp'
        temporary.write_text('new unit')
        os.replace(temporary, root / 'unit')
    if boundary in ('restart', 'readiness'):
        (root / 'worker-started').write_text(sha)
    if boundary == 'readiness':
        (root / 'readiness.json').write_text(json.dumps(dict(status='ok', db='ok', environment='production', release_sha=sha)))
    print('held', flush=True)
    sys.stdin.read()
'''
    process = subprocess.Popen([sys.executable, "-c", code, str(control),
                                str(tuple_root), str(candidate), boundary, SHA],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        assert process.stdout.readline().strip() == "held"
        assert start_is_allowed(control, "production", SHA, "verdaxis_prod", owner_uid=os.getuid())
        assert not start_is_allowed(control, "production", "b" * 40, "verdaxis_prod", owner_uid=os.getuid())
        assert not start_is_allowed(control, "production", SHA, "verdaxis", owner_uid=os.getuid())
    finally:
        process.kill()
        process.wait(timeout=5)
        process.stdin.close()
        process.stdout.close()
    assert (tuple_root / "identity").read_text() == "new"
    assert (tmp_path / "rollback/identity").read_text() == "old"
    assert not start_is_allowed(control, "production", SHA, "verdaxis_prod", owner_uid=os.getuid())


def test_dangling_fence_and_missing_control_fail_closed(tmp_path):
    from scripts.phase0.start_fence import start_is_allowed

    assert not start_is_allowed(tmp_path / "missing", "production", SHA, "verdaxis_prod", owner_uid=os.getuid())
    (tmp_path / "fence").symlink_to(tmp_path / "absent")
    assert not start_is_allowed(tmp_path, "production", SHA, "verdaxis_prod", owner_uid=os.getuid())


@pytest.mark.parametrize("key,value", [
    ("database", "verdaxis_staging"), ("role", "verdaxis_app"),
    ("cluster_id", "999"), ("database_oid", 999), ("port", 5433),
])
def test_wrong_database_identity_refuses(key, value):
    from scripts.phase0.assert_host import require_identity
    expected = dict(database="verdaxis_prod", role="verdaxis_backup",
                    cluster_id="123", database_oid=123, host="127.0.0.1",
                    port=5432, revision="p0_20261016_union")
    require_identity(expected, expected)
    with pytest.raises(ValueError):
        require_identity(expected, {**expected, key: value})


@pytest.mark.parametrize("output", [
    "ActiveState=inactive\nSubState=dead\nJob=12",
    "ActiveState=active\nSubState=running\nJob=",
    "ActiveState=inactive\nSubState=dead",
])
def test_pending_job_or_writer_refuses(output):
    from scripts.phase0.assert_host import require_stopped_unit
    with pytest.raises(ValueError):
        require_stopped_unit(output)


def test_stopped_unit_properties_do_not_depend_on_systemd_order():
    from scripts.phase0.assert_host import require_stopped_unit
    require_stopped_unit("Job=\nSubState=dead\nActiveState=inactive")


def test_external_fence_does_not_replace_two_lock_guard(tmp_path):
    import fcntl
    import importlib.util

    from scripts.phase0.start_fence import start_is_allowed

    root = tmp_path / "be"
    (root / "scripts").mkdir(parents=True)
    helper = root / "scripts/verify_deploy_start_guard.py"
    helper.write_bytes((ROOT / "scripts/verify_deploy_start_guard.py").read_bytes())
    spec = importlib.util.spec_from_file_location("synthetic_deploy_guard", helper)
    guard = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(guard)
    state = root / ".runtime-deploy"
    state.mkdir(mode=0o700)
    (state / "production.state").write_text(
        "DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=production\nRELEASE_SHA=" + SHA + "\n")
    control = tmp_path / "control"
    control.mkdir(mode=0o700)
    (control / "fence").write_text("phase0\n")
    (control / "fence").chmod(0o600)
    with (control / "operator.lock").open("w+") as operator, \
         (state / "production.lock").open("w+") as main, \
         (state / "production.restart.lock").open("w+") as restart:
        os.chmod(operator.name, 0o600)
        fcntl.flock(operator, fcntl.LOCK_EX)
        info = os.fstat(operator.fileno())
        permit = control / "permit.json"
        permit.write_text(json.dumps(dict(environment="production", release_sha=SHA,
            database="verdaxis_prod", lock_device=info.st_dev, lock_inode=info.st_ino)))
        permit.chmod(0o600)
        assert start_is_allowed(control, "production", SHA, "verdaxis_prod", owner_uid=os.getuid())
        assert not guard._start_is_allowed("production", SHA)
        fcntl.flock(main, fcntl.LOCK_EX)
        assert not guard._start_is_allowed("production", SHA)
        fcntl.flock(restart, fcntl.LOCK_EX)
        assert guard._start_is_allowed("production", SHA)
        fcntl.flock(operator, fcntl.LOCK_UN)
        assert not start_is_allowed(control, "production", SHA, "verdaxis_prod", owner_uid=os.getuid())
        # A new operator window must use a new inode, not revive a stale permit.
        os.unlink(operator.name)
        with open(operator.name, "w+") as replacement:
            os.chmod(operator.name, 0o600)
            fcntl.flock(replacement, fcntl.LOCK_EX)
            assert not start_is_allowed(control, "production", SHA, "verdaxis_prod", owner_uid=os.getuid())


def test_in_flight_producer_invalidates_first_zero(tmp_path):
    import sqlite3
    from scripts.phase0.assert_host import require_outbox_drained
    database = tmp_path / "synthetic-outbox.sqlite"
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TABLE outbox (stream_seq INTEGER)")
    producer = subprocess.Popen([sys.executable, "-c", """
import sqlite3, sys
with sqlite3.connect(sys.argv[1]) as connection:
    connection.execute('INSERT INTO outbox VALUES (NULL)')
    print('uncommitted', flush=True)
    sys.stdin.readline()
""", str(database)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        assert producer.stdout.readline().strip() == "uncommitted"
        with sqlite3.connect(database) as connection:
            first_zero = connection.execute("SELECT count(*) FROM outbox WHERE stream_seq IS NULL").fetchone()[0]
        assert first_zero == 0
        with pytest.raises(ValueError):
            require_outbox_drained(producers_stopped=False, sequencer_stopped=False,
                                   pending_jobs=0, app_sessions=1, undispatched=first_zero)
        producer.stdin.write("commit\n")
        producer.stdin.flush()
        assert producer.wait(timeout=5) == 0
        with sqlite3.connect(database) as connection:
            assert connection.execute("SELECT count(*) FROM outbox WHERE stream_seq IS NULL").fetchone()[0] == 1
            connection.execute("UPDATE outbox SET stream_seq=1")
        # Only a fresh post-stop read can pass the snapshot predicate.
        with sqlite3.connect(database) as connection:
            final_zero = connection.execute("SELECT count(*) FROM outbox WHERE stream_seq IS NULL").fetchone()[0]
        require_outbox_drained(producers_stopped=True, sequencer_stopped=True,
                               pending_jobs=0, app_sessions=0, undispatched=final_zero)
    finally:
        if producer.poll() is None:
            producer.kill()
            producer.wait(timeout=5)
        producer.stdin.close()
        producer.stdout.close()


def test_tuple_assertion_binds_runtime_and_monitor_bytes(tmp_path, monkeypatch):
    import hashlib
    from scripts.phase0 import assert_host
    root = tmp_path / "be"
    root.mkdir()
    sha = "a" * 40
    identity = f"ENVIRONMENT=production\nRELEASE_SHA={sha}\n"
    (root / ".runtime-monitor.env").write_text(identity)
    (root / ".runtime-release.env").write_text(identity + "MIGRATION_REVISION=p0_20261016_union\n")
    (root / ".env").write_text("synthetic")
    monkeypatch.setattr(assert_host, "ROOT", root)
    monkeypatch.setattr(assert_host, "command", lambda args: sha if args[-1] == "HEAD" else "")
    required_units = {
        "/etc/systemd/system/verdaxis-backend.service": b"synthetic unit",
        "/etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf": b"synthetic guard",
    }
    original_read = Path.read_bytes
    monkeypatch.setattr(Path, "read_bytes", lambda path: required_units[str(path)] if str(path) in required_units else original_read(path))
    artifacts = {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}
    artifacts.update({name: hashlib.sha256(value).hexdigest() for name, value in required_units.items()})
    manifest = dict(release_sha=sha, identity={"revision": "p0_20261016_union"},
                    monitor_identity_file=str(root / ".runtime-monitor.env"), artifacts=artifacts)
    assert_host.verify_tuple(manifest)
    (root / ".runtime-monitor.env").write_text(identity.replace(sha, "b" * 40))
    with pytest.raises(ValueError, match="Monitor identity"):
        assert_host.verify_tuple(manifest)
