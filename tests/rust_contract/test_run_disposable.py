"""Unit tests for the disposable runner's migration verification.

The runner must not reduce verification: the shared verifier
(`scripts/verify_migrations.sh`) checks that exactly one standalone Alembic head
exists, upgrades to it, then checks heads and model/schema drift. A bare
`alembic upgrade head` in the runner skips all three checks.
"""

from pathlib import Path

from scripts.rust_contract import run_disposable

ROOT = Path(__file__).resolve().parents[2]
VERIFIER = str(ROOT / "scripts" / "verify_migrations.sh")
ALEMBIC_BIN = str(ROOT / "venv" / "bin" / "alembic")
LEGACY_REVISION = "miq_20260720_market_quarantine"


class _Result:
    returncode = 0
    stdout = ""
    stderr = ""


def _runner(monkeypatch, *, seed: bool, seed_mode: str = "corpus"):
    """Build a Runner whose subprocess calls are captured, not executed."""
    calls: list[list] = []

    def fake_run(args, **kwargs):
        calls.append(list(args))
        return _Result()

    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)
    runner = run_disposable.Runner(
        "health", seed=seed, keep=False, seed_mode=seed_mode
    )
    # Drop the constructor's own `git rev-parse HEAD` probe.
    calls.clear()
    monkeypatch.setattr(runner, "adopt_database", lambda env: None)
    monkeypatch.setattr(run_disposable, "docker", lambda *args, **kwargs: "")
    return runner, calls


def _env() -> dict:
    return {"DATABASE_URL": "postgresql+asyncpg://rust_owner@127.0.0.1/verdaxis_rust_test"}


def test_runner_verifies_migrations_with_the_shared_verifier(monkeypatch):
    runner, calls = _runner(monkeypatch, seed=False)
    runner.migrate_and_seed(_env())
    # The verifier owns the head upgrade, so the runner issues no bare one.
    assert calls == [[VERIFIER]]


def test_shared_verifier_uses_the_selected_venv_alembic(monkeypatch):
    runner, _ = _runner(monkeypatch, seed=False)
    captured = {}

    def fake_run(args, **kwargs):
        captured["env"] = kwargs["env"]
        return _Result()

    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)
    runner.migrate_and_seed(_env())
    assert captured["env"]["ALEMBIC_BIN"] == ALEMBIC_BIN


def test_corpus_legacy_stage_precedes_verification_and_corpus_seed(monkeypatch):
    runner, calls = _runner(monkeypatch, seed=True)
    runner.migrate_and_seed(_env())
    assert calls == [
        [ALEMBIC_BIN, "upgrade", LEGACY_REVISION],
        [runner.python_bin, "-m", "scripts.rust_contract.seed_corpus"],
        [VERIFIER],
        [runner.python_bin, "-m", "scripts.rust_contract.seed_corpus"],
    ]
    assert not any(
        command[:3] == [ALEMBIC_BIN, "upgrade", "head"] for command in calls
    )


def test_legacy_seed_is_staged_before_migrations_run(monkeypatch):
    runner, calls = _runner(monkeypatch, seed=True)
    captured = {}

    def fake_run(args, **kwargs):
        calls.append(list(args))
        if args[0] == runner.python_bin and len(calls) == 2:
            captured["env"] = kwargs["env"]
        return _Result()

    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)
    runner.migrate_and_seed(_env())
    assert captured["env"]["RUST_CORPUS_STAGE"] == "legacy"


def test_combined_gate_gives_each_target_a_fresh_environment(monkeypatch):
    """The `--rust-target all` publication gate isolates every group.

    Extended-target probes mutate the corpus (invalid-terms fixtures, price and
    window rewrites), so a shared database changes later groups' oracle
    answers. The combined gate must start one fresh database and server pair
    per group, in group order, and stop each before the next starts.
    """
    started: list[str] = []
    stopped = 0

    class _FakeProc:
        def terminate(self):
            pass

        def wait(self, timeout=None):
            return 0

    runner, _ = _runner(monkeypatch, seed=False)
    runner.rust_target = "all"
    runner.group = "all"
    runner.container = "container-id"
    monkeypatch.setattr(runner, "target_groups", lambda: ["health", "echo"])
    monkeypatch.setattr(
        runner,
        "start_environment",
        lambda group: started.append(group)
        or {
            "base_env": {},
            "python_port": 1,
            "rust_port": 2,
            "python_url": "http://python",
            "rust_url": "http://rust",
        },
    )

    def fake_stop():
        nonlocal stopped
        stopped += 1
        runner.container = ""

    monkeypatch.setattr(runner, "stop_environment", fake_stop)
    target_groups: list[list[str]] = []

    def fake_targets(env, python_url, rust_url, groups=None):
        target_groups.append(list(groups or []))
        return {groups[0]: True}

    monkeypatch.setattr(runner, "run_targets", fake_targets)
    monkeypatch.setattr(runner, "finalize", lambda results: None)
    monkeypatch.setattr(
        run_disposable.subprocess, "run", lambda *a, **k: _Result()
    )
    runner.run()
    assert started == ["health", "echo"]
    assert target_groups == [["health"], ["echo"]]
    assert stopped == 2


def test_mixed_run_seeds_identities_before_the_suite(monkeypatch, tmp_path):
    """The mixed run must provision the accounts the suite logs in as.

    The corpus seed does not create the itest-* users, so the integration suite
    failed every authenticated test (GitHub run 38993964840). The mixed run
    seeds them with the same password it passes to pytest.
    """
    import sys

    calls: list[dict] = []

    def fake_run(args, **kwargs):
        calls.append({"args": list(args), "env": kwargs.get("env", {})})
        return _Result()

    runner, _ = _runner(monkeypatch, seed=False)
    runner.token = "-disposable-token-with-leading-hyphen-000000000"
    monkeypatch.setenv("CADDY_BIN", sys.executable)
    monkeypatch.setattr(run_disposable, "ephemeral_port", lambda: 41000)
    monkeypatch.setattr(run_disposable, "wait_for_port", lambda port: None)
    monkeypatch.setattr(run_disposable, "attest", lambda url, token: None)
    monkeypatch.setattr(runner, "spawn", lambda args, env: None)
    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)

    runner.run_mixed(_env(), python_port=1, rust_port=2)

    commands = [call["args"] for call in calls]
    seed = [c for c in commands if c[:3] == [
        runner.python_bin, "-m", "scripts.rust_contract.seed_integration"]]
    assert len(seed) == 1
    pytest_call = next(c for c in commands if c[1:3] == ["-m", "pytest"])
    assert f"--disposable-target-token={runner.token}" in pytest_call
    assert "--disposable-target-token" not in pytest_call
    # Seed first, then the suite; both use the one shared password.
    assert commands.index(seed[0]) < commands.index(pytest_call)
    for call in calls:
        if call["args"] in (seed[0], pytest_call):
            assert (
                call["env"]["DISPOSABLE_ITEST_PASSWORD"]
                == run_disposable.ITEST_PASSWORD
            )


def test_build_profile_selection_keeps_correctness_on_dev():
    """Correctness runs build dev; only an explicit release asks for --release.

    The dev profile panics on integer overflow and so catches more defects,
    which is why every correctness invocation must keep it (PM decision).
    Latency measurement runners construct the Runner with release=True and
    then must build and start the release binary instead.
    """
    dev = run_disposable.Runner(
        "orderbook_lists", seed=False, keep=False, seed_mode="corpus"
    )
    assert "--release" not in dev.build_command()
    assert dev.server_path().parts[-3] == "debug"
    release = run_disposable.Runner(
        "orderbook_lists", seed=False, keep=False, seed_mode="corpus",
        release=True,
    )
    assert release.build_command()[-1] == "--release"
    assert release.server_path().parts[-3] == "release"


def test_explicit_contract_server_path_is_used_without_changing_profile(tmp_path):
    binary = tmp_path / "shared-target/debug/examples/contract_server"
    runner = run_disposable.Runner("health", seed=False, keep=False, seed_mode="corpus", server_binary=binary)
    assert runner.server_path() == binary
    assert "--release" not in runner.build_command()


def test_shared_target_path_is_passed_to_every_contract_subprocess(monkeypatch, tmp_path):
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "shared-target"))
    runner, _ = _runner(monkeypatch, seed=False)
    captured = []

    def fake_run(args, **kwargs):
        captured.append(kwargs["env"])
        return _Result()

    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)
    runner.run_targets({}, "http://127.0.0.1:1", "http://127.0.0.1:2", ["orderbook_snapshot", "orderbook_map"])
    expected = tmp_path / "shared-target/debug/examples/contract_server"
    assert runner.server_path() == expected
    assert len(captured) == 2
    assert all(env["RUST_CONTRACT_SERVER_BINARY"] == str(expected) for env in captured)
    for name in ("orderbook_snapshot_oracle.py", "orderbook_map_oracle.py"):
        source = (ROOT / "rs/read/tests" / name).read_text()
        assert 'os.environ["RUST_CONTRACT_SERVER_BINARY"]' in source
        assert "rs/target/debug/examples/contract_server" not in source


def test_runtime_revision_comes_from_the_reviewed_allowlist():
    import json

    allowed = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert run_disposable.MIGRATION_REVISION == allowed[0]


def test_disposable_adoption_uses_the_existing_rust_adopter(monkeypatch):
    runner, calls = _runner(monkeypatch, seed=False)
    environment = {"MIGRATOR_DATABASE_URL":
                   "postgresql+asyncpg://verdaxis_migrator_staging:pw@127.0.0.1:49160/verdaxis_rust_test"}
    captured = {}

    def fake_run(args, **kwargs):
        calls.append(list(args))
        captured.update(kwargs)
        return _Result()

    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)
    run_disposable.Runner.adopt_database(runner, environment)
    assert calls == [["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
                      "-p", "verdaxis-read", "--features", "contract-tests",
                      "--test", "phase10_migrate_postgres",
                      "runner_database_adopts_to_the_baseline", "--", "--ignored"]]
    assert captured["check"] is True
    assert captured["env"]["RUST_RUNNER_ENVIRONMENT"] == "staging"
    assert captured["env"]["RUST_RUNNER_MIGRATOR_DATABASE_URL"] == environment["MIGRATOR_DATABASE_URL"].replace("postgresql+asyncpg:", "postgresql:")


def test_adoption_precedes_corpus_seed(monkeypatch):
    runner, calls = _runner(monkeypatch, seed=True)
    monkeypatch.setattr(runner, "adopt_database", lambda env: calls.append(["adopt"]))
    runner.migrate_and_seed(_env())
    assert calls.index([VERIFIER]) < calls.index(["adopt"]) < len(calls) - 1


def test_migration_failure_never_adopts_or_seeds(monkeypatch):
    import subprocess

    import pytest

    runner, calls = _runner(monkeypatch, seed=False)
    monkeypatch.setattr(runner, "adopt_database", lambda env: calls.append(["adopt"]))

    def fail_verification(args, **kwargs):
        raise subprocess.CalledProcessError(1, args)

    monkeypatch.setattr(run_disposable.subprocess, "run", fail_verification)
    with pytest.raises(subprocess.CalledProcessError):
        runner.migrate_and_seed(_env())
    assert calls == []


def test_migration_uses_the_disposable_migrator_not_the_runtime_role(monkeypatch):
    runner, _ = _runner(monkeypatch, seed=False)
    environment = runner.urls(49160)
    captured = {}

    def fake_run(args, **kwargs):
        captured.update(kwargs["env"])
        return _Result()

    monkeypatch.setattr(run_disposable.subprocess, "run", fake_run)
    runner.migrate_and_seed(environment)
    assert captured["DATABASE_URL"] == environment["MIGRATOR_DATABASE_URL"]
    assert captured["DATABASE_URL"] != environment["DATABASE_URL"]


def test_statistics_extension_follows_adoption_for_every_runner_consumer(monkeypatch):
    runner, _ = _runner(monkeypatch, seed=False)
    events = []
    monkeypatch.setattr(runner, "adopt_database", lambda env: events.append("adopt"))
    monkeypatch.setattr(run_disposable, "docker", lambda *args, **kwargs: events.append(args[-1]))
    runner.migrate_and_seed(_env())
    assert events == ["adopt", "CREATE EXTENSION IF NOT EXISTS pg_stat_statements;"]


def test_postgres_baseline_group_leaves_adoption_to_explicit_runner_calls():
    import shlex

    source = (ROOT / "rs/read/tests/run_postgres.sh").read_text()
    commands = [
        shlex.split(line.split("cargo test ", 1)[1])
        for line in source.replace("\\\n", " ").splitlines()
        if "cargo test " in line and not line.lstrip().startswith("#")
    ]
    baseline = next(command for command in commands if "phase10_baseline_postgres" in command)
    arguments = baseline[baseline.index("--") + 1:]
    assert "--include-ignored" in arguments
    skip = arguments.index("--skip")
    assert arguments[skip + 1] == "runner_database_adopts_to_the_baseline"
    explicit = [command for command in commands
                if "runner_database_adopts_to_the_baseline" in command[:command.index("--")]]
    assert explicit
    assert all(command[command.index("--") + 1] == "--ignored" for command in explicit)
    assert 'RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://' in source
