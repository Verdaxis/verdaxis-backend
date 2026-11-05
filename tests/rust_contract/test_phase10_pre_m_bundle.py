"""Off-host contracts for the private native release transaction."""
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "deploy/scripts/pre-m-bundle.sh"


class PreMReleaseBundle(unittest.TestCase):
    def test_native_entrypoint_exists(self):
        self.assertTrue(SCRIPT.is_file(), "native private bundle entrypoint is required")

    def test_cli_refuses_incomplete_capture_without_python(self):
        self.assertTrue(SCRIPT.is_file(), "native private bundle entrypoint is required")
        result = subprocess.run(["bash", str(SCRIPT), "capture"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("NO_GO", result.stderr)

    def test_production_identity_and_trusted_tool_routing(self):
        self.assertTrue(SCRIPT.is_file(), "native private bundle entrypoint is required")
        source = SCRIPT.read_text()
        self.assertIn("== verdaxis_prod", source)
        self.assertIn("/verdaxis_prod'", source)
        self.assertIn("export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", source)
        self.assertIn('--workflow-ref "$(jq -r .workflow_ref', source)
        self.assertNotIn('printf prod; else printf staging', source)

    def test_root_git_trust_is_limited_to_the_canonical_checkout(self):
        source = SCRIPT.read_text()
        self.assertIn('safe.directory=$checkout', source)
        self.assertIn('GIT_CONFIG_GLOBAL=/dev/null', source)
        self.assertIn('core.hooksPath=/dev/null', source)

    def test_capture_checks_absent_state_before_restart_lock(self):
        self.assertTrue(SCRIPT.is_file(), "native private bundle entrypoint is required")
        source = SCRIPT.read_text()
        locks = source[source.index("locks() {"):source.index("# Test fixtures")]
        self.assertLess(locks.index("capture requires absent deploy state"),
                        locks.index('lock_one "$state_dir/$environment.restart.lock"'))

    def test_runtime_is_not_a_callback_runner(self):
        self.assertTrue(SCRIPT.is_file(), "native private bundle entrypoint is required")
        source = SCRIPT.read_text()
        for forbidden in ("python3", "python -", "eval ", "bash -c", "sh -c"):
            self.assertNotIn(forbidden, source)
        for required in ("restart.lock", "DEPLOY_NONCE", "restart-authorized", "--version-json",
                         "catalog-fingerprint.sql", "MainPID", "InvocationID", "auth", "session",
                         "monitor", "passive", "migrator_credential"):
            self.assertIn(required, source)


if __name__ == "__main__":
    unittest.main()
