"""Offline tests for the Rust runtime budget preparation boundary."""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("prepare_rust_budget", ROOT / "scripts/prepare_rust_budget.py")
budget = importlib.util.module_from_spec(spec)
spec.loader.exec_module(budget)


class RustBudgetTests(unittest.TestCase):
    def settings(self, **changes):
        values = dict(DB_SERVICE_COUNT=2, UVICORN_WORKERS=4, DB_POOL_SIZE=2,
                      DB_MAX_OVERFLOW=1, DB_RESERVED_CONNECTIONS=20, DB_MAX_CONNECTIONS=100)
        values.update(changes)
        return SimpleNamespace(**values)

    def test_rust_pool_is_not_pinned_to_python_admission(self):
        for slots in (1, 8, 16, 24, 47):
            self.assertEqual(budget.derive_budget(self.settings(), slots + 1)["total"], 53 + slots)
        for rust_pool in (0, 1, 49, 2**32, True):
            with self.assertRaises(ValueError):
                budget.derive_budget(self.settings(), rust_pool)

    def test_effective_budget_and_overflow(self):
        self.assertEqual(budget.derive_budget(self.settings(), 9)["total"], 61)
        self.assertEqual(budget.derive_budget(self.settings(DB_POOL_SIZE=8, DB_MAX_CONNECTIONS=109), 9)["total"], 109)
        for changes in [dict(DB_MAX_CONNECTIONS=60), dict(DB_POOL_SIZE=8), dict(UVICORN_WORKERS=2**32)]:
            with self.assertRaises(ValueError):
                budget.derive_budget(self.settings(**changes), 9)
        for changes, expected in [(dict(UVICORN_WORKERS=5), 69), (dict(DB_MAX_OVERFLOW=2), 69), (dict(DB_RESERVED_CONNECTIONS=21), 62)]:
            self.assertEqual(budget.derive_budget(self.settings(**changes), 9)["total"], expected)

    def test_stream_listener_keeps_python_and_cache_connections(self):
        result = budget.derive_budget(self.settings(), 17, cache_listener=1, stream_listener=1)
        self.assertEqual(result["python_budget"], 52)
        self.assertEqual(result["workers"], 4)
        self.assertEqual(result["total"], 71)
        self.assertEqual(result["stream_listener"], 1)
        for bad in (2, -1, True, "1", None):
            with self.assertRaises(ValueError):
                budget.derive_budget(self.settings(), 17, stream_listener=bad)
        with self.assertRaises(ValueError):
            budget.derive_budget(self.settings(DB_MAX_CONNECTIONS=70), 17, 1, 1)
        with self.assertRaises(ValueError):
            budget.derive_budget(self.settings(UVICORN_WORKERS=1), 17, stream_listener=1)

    def test_stream_listener_artifact_is_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = root / "runtime.env"
            env.write_text("ENVIRONMENT=test\nRELEASE_SHA=test\nDATABASE_URL=sqlite+aiosqlite:///:memory:\n")
            artifact = root / "budget.json"
            budget.prepare(env, "a" * 40, artifact, stream_listener=1)
            self.assertEqual(json.loads(artifact.read_text())["stream_listener"], 1)
            budget.verify(env, "a" * 40, artifact)
            recorded = json.loads(artifact.read_text())
            recorded["stream_listener"] = 0
            artifact.write_text(json.dumps(recorded))
            with self.assertRaises(ValueError):
                budget.verify(env, "a" * 40, artifact)

    def test_cache_listener_is_one_budgeted_connection_or_absent(self):
        plain = budget.derive_budget(self.settings(), 17)
        self.assertNotIn("cache_listener", plain)
        self.assertEqual(plain, budget.derive_budget(self.settings(), 17, 0))
        listener = budget.derive_budget(self.settings(), 17, 1)
        self.assertEqual(listener["cache_listener"], 1)
        self.assertEqual(listener["total"], plain["total"] + 1)
        for bad in (2, -1, True, "1", None):
            with self.assertRaises(ValueError):
                budget.derive_budget(self.settings(), 17, bad)
        # The listener must fit inside the configured maximum on its own.
        exact = self.settings(DB_MAX_CONNECTIONS=plain["total"])
        self.assertEqual(budget.derive_budget(exact, 17, 0)["total"], plain["total"])
        with self.assertRaises(ValueError):
            budget.derive_budget(exact, 17, 1)

    def test_listener_artifact_is_prepared_and_verified_with_its_recorded_choice(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = root / "runtime.env"
            env.write_text("ENVIRONMENT=test\nRELEASE_SHA=test\nDATABASE_URL=sqlite+aiosqlite:///:memory:\n")
            sha = "a" * 40
            plain, with_listener = root / "plain.json", root / "listener.json"
            budget.prepare(env, sha, plain)
            budget.prepare(env, sha, with_listener, 1)
            self.assertNotIn("cache_listener", json.loads(plain.read_text()))
            recorded = json.loads(with_listener.read_text())
            self.assertEqual(recorded["cache_listener"], 1)
            self.assertEqual(recorded["total"], json.loads(plain.read_text())["total"] + 1)
            budget.verify(env, sha, plain)
            budget.verify(env, sha, with_listener)
            # Editing the recorded choice cannot make the totals agree.
            recorded["cache_listener"] = 0
            with_listener.write_text(json.dumps(recorded))
            with self.assertRaises(ValueError):
                budget.verify(env, sha, with_listener)
            plain.write_text("[]")
            with self.assertRaises(ValueError):
                budget.verify(env, sha, plain)

    def test_digest_drift_and_private_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = root / "runtime.env"
            env.write_text("ENVIRONMENT=test\nRELEASE_SHA=test\nDATABASE_URL=sqlite+aiosqlite:///:memory:\n")
            artifact = root / "budget.json"
            sha = "a" * 40
            budget.prepare(env, sha, artifact)
            data = json.loads(artifact.read_text())
            self.assertEqual(data["total"], 69)
            self.assertEqual(data["release_sha"], sha)
            self.assertEqual(artifact.stat().st_mode & 0o777, 0o600)
            self.assertNotIn("DATABASE_URL", artifact.read_text())
            budget.verify(env, sha, artifact)
            with self.assertRaises(ValueError):
                budget.verify(env, "b" * 40, artifact)
            env.write_text(env.read_text() + "DB_POOL_SIZE=3\n")
            with self.assertRaises(ValueError):
                budget.verify(env, sha, artifact)

    def test_second_preparation_and_retry_replace_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = root / "runtime.env"
            env.write_text("ENVIRONMENT=test\nRELEASE_SHA=test\nDATABASE_URL=sqlite+aiosqlite:///:memory:\n")
            artifact = root / "budget.json"
            for sha in ("a" * 40, "b" * 40, "b" * 40):
                budget.prepare(env, sha, artifact)
                budget.verify(env, sha, artifact)
            self.assertEqual(artifact.stat().st_mode & 0o777, 0o600)
            saved = artifact.read_bytes()
            artifact.unlink()
            victim = root / "victim"
            victim.write_bytes(saved)
            artifact.symlink_to(victim)
            with self.assertRaises((ValueError, OSError)):
                budget.prepare(env, "c" * 40, artifact)
            self.assertEqual(victim.read_bytes(), saved)

    def test_environment_symlink_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            real = root / "real.env"
            real.write_text("ENVIRONMENT=test\n")
            link = root / "link.env"
            link.symlink_to(real)
            with self.assertRaises((ValueError, OSError)):
                budget.prepare(link, "a" * 40, root / "budget.json")


if __name__ == "__main__":
    unittest.main()
