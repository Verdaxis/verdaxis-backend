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
