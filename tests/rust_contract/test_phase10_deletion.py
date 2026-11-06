"""Task 32 deletion contract.

Each named check runs against a tree root so the same checks run on the
synthetic fixtures here and, after the W5 release, on the real post-deletion
tree. The fixtures are synthetic data: nothing here imports, starts or
deletes the application, and no host is contacted.

The pre tree still has ``app/``, a Python Caddy fallback and the backend
units; the post tree is the W5 end state. Every check must pass on the post
tree and report a failure on the pre tree, except the two checks that state
rules holding in both states. A check that cannot fail on the pre tree is a
broken check, so the normal test run fails in that case.
"""

import ast
import hashlib
import json
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase10/deletion-fixtures"
PRE = FIXTURES / "pre"
POST = FIXTURES / "post"

# Paths the W5 release deletes from the repository, as recorded in the
# removal ledger rows owned by Task 32. Checked against the post tree only.
DELETED_PATHS = (
    "app",
    "alembic/env.py",
    "alembic/script.py.mako",
    "alembic.ini",
    "scripts/verify_migrations.sh",
    "deploy/systemd/verdaxis-backend.service",
    "deploy/systemd/verdaxis-backend-staging.service",
    "deploy/systemd/verdaxis-order-expiry-reminders-staging.service",
    "deploy/systemd/verdaxis-order-expiry-reminders-staging.timer",
)

# Tokens that name the deleted Python application inside a runtime surface.
PYTHON_APP_TOKENS = (
    "app.main",
    "app/cli",
    "app/routers",
    "uvicorn",
    "venv/bin",
    "alembic.ini",
    "alembic/env.py",
    "scripts/verify_migrations.sh",
)

# Caddy path matchers that point at the Python upstream in the pre tree and
# must be retired (404, no upstream) in the post tree.
RETIRED_MATCHERS = {
    "retired_activity": "/api/stream/activity",
    "retired_ai_chat": "/api/ai/chat",
    "retired_mcp": "/mcp",
    "retired_admin": "/admin",
}

# Host surface files read as text for installed-reference scans.
HOST_SURFACE_SUFFIXES = (".service", ".timer", ".caddy", ".sh", ".json", ".yml")


def host_inventory(root=ROOT):
    return json.loads((root / "host/host-inventory.json").read_text())


def fixture_offhost_manifest(root):
    return json.loads((root / "offhost/bundle.manifest.json").read_text())


def caddy_files(root):
    return sorted((root / "deploy/caddy").glob("*.caddy"))


def tree_files(root, base):
    top = root / base
    if not top.is_dir():
        return []
    return [path for path in sorted(top.rglob("*"))
            if path.is_file() and "__pycache__" not in path.parts]


def surface_files(root):
    """Installed-surface files: units, timers, Caddy, deploy scripts and docs."""
    surfaces = []
    for base in ("deploy",):
        for path in tree_files(root, base):
            if path.suffix.lower() in HOST_SURFACE_SUFFIXES or path.name in {"runtime-units.manifest"}:
                surfaces.append(path)
    return surfaces


# ---------------------------------------------------------------------------
# Fixtures: one tree root per test. Task 33/35 reuse the checks on the real
# post-deletion tree by passing its root instead.
# ---------------------------------------------------------------------------

TREE_ROOTS = {"pre": PRE, "post": POST}


# These two checks describe rules that hold before and after W5, so they pass
# on both trees. Every other check must report a failure on the pre tree.
CHECKS_THAT_HOLD_ON_BOTH_TREES = frozenset({
    "check_host_python_cleanup_removes_only_verdaxis_owned_paths",
    "check_daily_backup_and_staging_pull_still_run",
})


@pytest.fixture(params=list(TREE_ROOTS.values()), ids=list(TREE_ROOTS))
def root(request):
    return request.param


def assert_check(check, root):
    """Pass on the post tree; on the pre tree, require the check to report a failure.

    A check that passes on the pre tree can no longer detect the state that
    W5 removes, so that is a test failure too.
    """
    expect_failure = root == PRE and check.__name__ not in CHECKS_THAT_HOLD_ON_BOTH_TREES
    if not expect_failure:
        check(root)
        return
    with pytest.raises(AssertionError):
        check(root)


# ---------------------------------------------------------------------------
# 1. python_application_tree_is_absent
# ---------------------------------------------------------------------------

def check_python_application_tree_is_absent(root):
    for relative in DELETED_PATHS:
        assert not (root / relative).exists(), f"deleted path remains: {relative}"
    assert not (root / "app").exists(), "the Python application tree remains"
    assert not list(root.glob("alembic/*.mako")), "the Alembic template remains"


def test_python_application_tree_is_absent(root):
    assert_check(check_python_application_tree_is_absent, root)


# ---------------------------------------------------------------------------
# 2. no_installed_reference_to_a_deleted_python_path
# ---------------------------------------------------------------------------

def check_no_installed_reference_to_a_deleted_python_path(root):
    for path in surface_files(root):
        text = path.read_text(errors="replace")
        for token in PYTHON_APP_TOKENS:
            assert token not in text, f"installed reference to {token}: {path.relative_to(root)}"


def test_no_installed_reference_to_a_deleted_python_path(root):
    assert_check(check_no_installed_reference_to_a_deleted_python_path, root)


# ---------------------------------------------------------------------------
# 3. no_caddy_upstream_or_route_points_at_python
# ---------------------------------------------------------------------------

PYTHON_UPSTREAM_PATTERN = re.compile(
    r"reverse_proxy\s+(?:[^\n]*\s)?(?:127\.0\.0\.1:800[01]|\{\$PYTHON_UPSTREAM\})")


def check_no_caddy_upstream_or_route_points_at_python(root):
    for path in caddy_files(root):
        text = path.read_text()
        assert not PYTHON_UPSTREAM_PATTERN.search(text), (
            f"Caddy upstream still points at Python: {path.relative_to(root)}")
        for handle in re.findall(r"handle\s+/mcp\b[^{]*\{", text):
            raise AssertionError(f"/mcp still routed: {path.relative_to(root)}: {handle}")


def test_no_caddy_upstream_or_route_points_at_python(root):
    assert_check(check_no_caddy_upstream_or_route_points_at_python, root)


# ---------------------------------------------------------------------------
# 4. phase7_routes_have_their_recorded_end_state
# ---------------------------------------------------------------------------

def caddy_blocks(text):
    """Split a Caddy file into named matcher blocks with their handle bodies."""
    blocks = {}
    for match in re.finditer(r"@([\w-]+)\s*\{([^{}]*)\}\s*handle\s+@\1\s*\{([^{}]*)\}", text):
        blocks[match.group(1)] = {"matcher": match.group(2), "body": match.group(3)}
    return blocks


def check_phase7_routes_have_their_recorded_end_state(root):
    text = "\n".join(path.read_text() for path in caddy_files(root))
    blocks = caddy_blocks(text)
    for matcher, path in sorted(RETIRED_MATCHERS.items()):
        block = blocks.get(matcher)
        assert block is not None, f"no retired-route block for {path}: missing {matcher}"
        assert "respond 404" in block["body"], f"{path} is not retired with 404: {matcher}"
        assert "reverse_proxy" not in block["body"], f"{path} retired block proxies: {matcher}"
    rfq = blocks.get("phase7_rfq_create")
    assert rfq is not None and "respond 404" in rfq["body"], "Phase 7 rfq retirement drifted"


def test_phase7_routes_have_their_recorded_end_state(root):
    assert_check(check_phase7_routes_have_their_recorded_end_state, root)


# ---------------------------------------------------------------------------
# 5. alembic_history_is_inert_and_unreferenced
# ---------------------------------------------------------------------------

def check_alembic_history_is_inert_and_unreferenced(root):
    versions = root / "alembic/versions"
    assert versions.is_dir(), "retained Alembic history is missing"
    retained = sorted(p for p in versions.rglob("*.py") if p.is_file())
    assert retained, "retained Alembic history is empty"
    for path in retained:
        source = ast.parse(path.read_text())
        for node in ast.walk(source):
            if isinstance(node, ast.Call) and ast.unparse(node.func) in {
                "op.execute", "sa.execute", "engine.execute",
            }:
                raise AssertionError(f"retained revision executes SQL: {path}")
    # No file outside the retained revisions runs Alembic or names its runner
    # parts: env.py, alembic.ini or a module invocation. A refusal guard that
    # merely greps for a forbidden command names none of these.
    runner_tokens = ("venv/bin/alembic", "alembic -c", "alembic.ini",
                     "alembic/env", "python -m alembic")
    scanned = [path for path in tree_files(root, ".")
               if versions not in path.parents and path != versions
               and path.suffix in {".py", ".sh", ".service", ".timer", ".caddy",
                                   ".yml", ".ini", ".mako"}]
    for path in scanned:
        text = path.read_text(errors="replace")
        for token in runner_tokens:
            assert token not in text, (
                f"Alembic runner referenced outside the retained history: "
                f"{path.relative_to(root)}: {token}")


def test_alembic_history_is_inert_and_unreferenced(root):
    assert_check(check_alembic_history_is_inert_and_unreferenced, root)


# ---------------------------------------------------------------------------
# 6. ci_harness_does_not_import_the_deleted_application
# ---------------------------------------------------------------------------

def check_ci_harness_does_not_import_the_deleted_application(root):
    for path in tree_files(root, ".github"):
        text = path.read_text()
        assert "uvicorn" not in text, f"CI starts the deleted application: {path}"
        for token in ("app.main", "from app", "import app"):
            assert token not in text, f"CI references the deleted application: {path}"
    # The retained, application-free CI still runs the data-only fixture test.
    text = "\n".join(path.read_text() for path in tree_files(root, ".github"))
    assert "test_data_fixture_only" in text, "retained CI lost the data-only fixture job"


def test_ci_harness_does_not_import_the_deleted_application(root):
    assert_check(check_ci_harness_does_not_import_the_deleted_application, root)


# ---------------------------------------------------------------------------
# 7. python_tests_that_import_app_are_removed_or_offhost
# ---------------------------------------------------------------------------

def check_python_tests_that_import_app_are_removed_or_offhost(root):
    manifest = fixture_offhost_manifest(root)
    offhost = {member["path"]: member for member in manifest["members"]}
    for path in tree_files(root, "tests"):
        if path.suffix != ".py":
            continue
        relative = path.relative_to(root).as_posix()
        if any("import app" in line or line.startswith("from app")
               for line in path.read_text().splitlines()):
            record = offhost.get(relative)
            assert record is not None, f"test imports the deleted app and is not off-host: {relative}"
            expected = hashlib.sha256(path.read_bytes()).hexdigest()
            assert record["sha256"] == expected, f"off-host digest drifted: {relative}"
        else:
            assert relative not in offhost, f"clean test recorded off-host: {relative}"
    # Every recorded off-host member resolves: the file still exists here with
    # the recorded digest, or the pre tree holds the retained audit copy.
    for relative, record in offhost.items():
        local = root / relative
        if local.is_file():
            assert hashlib.sha256(local.read_bytes()).hexdigest() == record["sha256"], (
                f"off-host digest drifted: {relative}")
        else:
            audit = PRE / relative
            assert audit.is_file(), f"off-host member has no audit copy: {relative}"
            assert hashlib.sha256(audit.read_bytes()).hexdigest() == record["sha256"], (
                f"off-host digest does not match the audit copy: {relative}")


def test_python_tests_that_import_app_are_removed_or_offhost(root):
    assert_check(check_python_tests_that_import_app_are_removed_or_offhost, root)


# ---------------------------------------------------------------------------
# 8. host_python_cleanup_removes_only_verdaxis_owned_paths
# ---------------------------------------------------------------------------

def check_host_python_cleanup_removes_only_verdaxis_owned_paths(root):
    document = host_inventory(root)
    for host, removals in document["cleanup_removals"].items():
        assert removals, f"no recorded cleanup for host: {host}"
        for removed in removals:
            assert any(removed == owned or removed.startswith(owned.rstrip("/") + "/")
                       for owned in document["owned_roots"]), (
                f"cleanup leaves the owned roots: {removed}")
    for document_path in (document["forbidden_removals"]):
        assert not any(removed == document_path for removals in document["cleanup_removals"].values()
                       for removed in removals), f"cleanup removes a forbidden path: {document_path}"


def test_host_python_cleanup_removes_only_verdaxis_owned_paths(root):
    assert_check(check_host_python_cleanup_removes_only_verdaxis_owned_paths, root)


# ---------------------------------------------------------------------------
# 9. staging_is_cleaned_before_production
# ---------------------------------------------------------------------------

def check_staging_is_cleaned_before_production(root):
    document = host_inventory(root)
    staging = document["hosts"]["staging"]["cleanup"]
    production = document["hosts"]["production"]["cleanup"]
    assert staging["status"] == "complete", "staging cleanup is not complete"
    assert production["status"] == "complete", "production cleanup is not complete"
    assert staging["completed_at"] < production["completed_at"], (
        "staging must be cleaned before production")
    python_units = document["installed_python_units"]
    assert not python_units["staging"], f"staging still has Python units: {python_units['staging']}"
    assert not python_units["production"], (
        f"production still has Python units: {python_units['production']}")


def test_staging_is_cleaned_before_production(root):
    assert_check(check_staging_is_cleaned_before_production, root)


# ---------------------------------------------------------------------------
# 10. old_timer_and_new_job_do_not_duplicate (R12)
# ---------------------------------------------------------------------------

def check_old_timer_and_new_job_do_not_duplicate(root):
    document = host_inventory(root)
    # The superseded Python timer/CLI pair must be gone from every host while
    # the replacement Rust job stays installed exactly once.
    for host, state in document["hosts"].items():
        installed = set(state["installed_units"])
        news = sorted(unit for unit in installed if "news-refresh" in unit)
        assert len(news) == 1, f"exactly one news timer must remain on {host}: {news}"
        expired = sorted(unit for unit in installed
                         if "order-expiry-reminders" in unit and "staging" in unit)
        assert not expired, f"superseded staging reminder pair still installed on {host}: {expired}"
    for unit in sorted((root / "deploy/systemd").glob("*.service")):
        if "staging" in unit.name and "order-expiry" in unit.name:
            raise AssertionError(f"superseded unit file remains in the tree: {unit.name}")
        text = unit.read_text()
        if "app.cli" in text:
            matching_timer = unit.name.replace(".service", ".timer")
            assert "staging" in unit.name, (
                f"retained production unit still launches a Python CLI: {unit.name}")
    timers = {path.name for path in (root / "deploy/systemd").glob("*.timer")}
    for timer in timers:
        service = timer.replace(".timer", ".service")
        assert (root / "deploy/systemd" / service).exists(), (
            f"installed timer has no matching unit file: {timer}")


def test_old_timer_and_new_job_do_not_duplicate(root):
    assert_check(check_old_timer_and_new_job_do_not_duplicate, root)


# ---------------------------------------------------------------------------
# 11. previous_release_artifact_still_starts
# ---------------------------------------------------------------------------

def check_previous_release_artifact_still_starts(root):
    evidence = host_inventory(root)["rollback_evidence"]
    assert evidence["pre_m_bundle"]["status"] == "accepted", (
        "no accepted pre-M rollback bundle")
    assert re.fullmatch(r"[0-9a-f]{64}", evidence["pre_m_bundle"]["sha256"]), (
        "pre-M bundle digest is not SHA-256")
    assert evidence["observed_start"], "the previous release artifact was not observed to start"


def test_previous_release_artifact_still_starts(root):
    assert_check(check_previous_release_artifact_still_starts, root)


# ---------------------------------------------------------------------------
# 12. daily_backup_and_staging_pull_still_run
# ---------------------------------------------------------------------------

RETAINED_SCHEDULES = {
    "nightly-backup": ("30 2 * * *", "production"),
    "staging-pull": ("0 3 * * *", "production"),
}


def check_daily_backup_and_staging_pull_still_run(root):
    document = host_inventory(root)
    retained = document["backup_and_pull"]
    for name, (expression, host) in RETAINED_SCHEDULES.items():
        entry = retained[name]
        assert entry["retained"], f"preserved schedule was dropped: {name}"
        assert entry["expression"] == expression, f"schedule drifted: {name}"
        assert entry["destination"] and entry["credential_references"], (
            f"schedule lost its destination or credentials: {name}")
    cron_commands = {entry["command"] for entries in document["cron"].values()
                     for entry in entries}
    for name, entry in retained.items():
        assert entry["path"] in cron_commands, f"cron no longer runs {name}"


def test_daily_backup_and_staging_pull_still_run(root):
    assert_check(check_daily_backup_and_staging_pull_still_run, root)


# ---------------------------------------------------------------------------
# Fixture contract: the two trees stay in their recorded shapes so RED and
# GREEN keep meaning what the plan says they mean.
# ---------------------------------------------------------------------------

def test_pre_fixture_still_has_the_deleted_application():
    assert (PRE / "app/main.py").is_file()
    assert (PRE / "alembic/env.py").is_file()
    assert "venv/bin/uvicorn app.main:app" in (PRE / "deploy/systemd/verdaxis-backend.service").read_text()
    assert PYTHON_UPSTREAM_PATTERN.search((PRE / "deploy/caddy/site.caddy").read_text())


def test_post_fixture_records_the_end_state():
    check_python_application_tree_is_absent(POST)
    assert (POST / "alembic/versions").is_dir()
    assert not list((POST / "alembic/versions").glob("*.mako"))
    assert not (POST / "alembic/env.py").exists()


# ---------------------------------------------------------------------------
# Deletion manifest generator: the exact W5 delete/keep partition.
# ---------------------------------------------------------------------------

MANIFEST_SCRIPT = ROOT / "scripts/rust_contract/deletion_manifest.py"


def manifest_lines():
    import subprocess
    import sys
    result = subprocess.run([sys.executable, str(MANIFEST_SCRIPT), "--ledger", str(
        ROOT / "tests/rust_contract/phase10/removal-ledger.json")],
        capture_output=True, text=True, timeout=30, check=True)
    return result.stdout.splitlines()


def manifest_delete_paths():
    return sorted(line[len("DELETE "):] for line in manifest_lines() if line.startswith("DELETE "))


def test_manifest_delete_list_matches_the_ledger_partition():
    ledger = json.loads((ROOT / "tests/rust_contract/phase10/removal-ledger.json").read_text())
    expected = sorted({row["source"] for row in ledger["artifacts"]
                       if row["end_state"] == "gone" and row.get("deleting_task") == 32})
    assert expected, "no Task 32 deletion rows in the removal ledger"
    assert manifest_delete_paths() == expected


def test_manifest_keep_list_excludes_every_delete_path():
    delete_set = set(manifest_delete_paths())
    keep_lines = [line for line in manifest_lines() if line.startswith("KEEP ")]
    assert keep_lines, "manifest lists no kept paths"
    assert any("alembic/versions" in line for line in keep_lines)
    assert any("tests/fixtures" in line for line in keep_lines)
    assert any("test_phase10_inventory.py" in line for line in keep_lines)
    assert any("offhost-tools-bundle.tar" in line for line in keep_lines)
    for line in keep_lines:
        keep = line[len("KEEP "):].split("#")[0].strip()
        assert keep not in delete_set, f"kept path is also listed for deletion: {keep}"
        assert not any(deleted.startswith(keep.rstrip("/") + "/") for deleted in delete_set), (
            f"kept path is shadowed by a deletion: {keep}")


def test_manifest_paths_exist_today():
    assert (MANIFEST_SCRIPT).is_file()
    for relative in manifest_delete_paths():
        assert (ROOT / relative).exists(), f"listed for deletion but absent today: {relative}"
    assert (ROOT / "alembic/versions").is_dir()
    assert (ROOT / "tests/fixtures").is_dir()
    assert (ROOT / "tests/rust_contract/test_phase10_inventory.py").is_file()


def test_manifest_refuses_a_missing_delete_path(tmp_path):
    import copy
    import subprocess
    import sys
    ledger = json.loads((ROOT / "tests/rust_contract/phase10/removal-ledger.json").read_text())
    broken = copy.deepcopy(ledger)
    gone = next(row for row in broken["artifacts"]
                if row["end_state"] == "gone" and row.get("deleting_task") == 32)
    gone["source"] = "app/already_absent.py"
    broken_ledger = tmp_path / "removal-ledger.json"
    broken_ledger.write_text(json.dumps(broken))
    result = subprocess.run([sys.executable, str(MANIFEST_SCRIPT), "--ledger", str(broken_ledger),
                             "--check"], capture_output=True, text=True, timeout=30)
    assert result.returncode != 0, "a delete path that does not exist must fail --check"
    assert "absent today" in result.stdout
