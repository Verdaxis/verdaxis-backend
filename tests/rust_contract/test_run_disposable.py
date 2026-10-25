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
