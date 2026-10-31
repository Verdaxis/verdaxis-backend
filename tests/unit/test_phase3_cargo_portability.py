"""Phase 3 subprocess commands follow explicit runner configuration."""
from tests.postgres.test_phase3_cross_stack_auth import cargo


def test_cargo_without_host_configuration_is_portable(monkeypatch):
    monkeypatch.delenv("PHASE3_CARGO_LOCK_WRAPPER", raising=False)
    monkeypatch.delenv("PHASE3_CARGO_LOCK_PATH", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    assert cargo("build", "--release") == ["cargo", "build", "--release"]


def test_cargo_uses_only_an_explicit_lock(monkeypatch, tmp_path):
    monkeypatch.delenv("PHASE3_CARGO_LOCK_WRAPPER", raising=False)
    lock = str(tmp_path / "cargo.lock")
    monkeypatch.setenv("PHASE3_CARGO_LOCK_PATH", lock)
    assert cargo("test") == ["flock", "-w", "7200", lock, "cargo", "test"]
    monkeypatch.setenv("PHASE3_CARGO_LOCK_WRAPPER", "1")
    assert cargo("test") == ["cargo", "test"]
