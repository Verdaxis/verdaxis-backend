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
