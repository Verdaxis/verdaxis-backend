"""Off-host contracts for the private native release transaction."""
import os
import subprocess
import tempfile
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
        self.assertIn('core.fsmonitor=false', source)
        self.assertIn('canonical Git directory must be a real directory', source)
        self.assertIn('Git configuration outside the inert recovery allowlist', source)

    def test_capture_checks_absent_state_before_restart_lock(self):
        self.assertTrue(SCRIPT.is_file(), "native private bundle entrypoint is required")
        source = SCRIPT.read_text()
        locks = source[source.index("locks() {"):source.index("# Test fixtures")]
        self.assertLess(locks.index("capture requires absent deploy state"),
                        locks.index('lock_one "$state_dir/$environment.restart.lock"'))

    def test_readiness_requires_one_complete_matching_object(self):
        source = SCRIPT.read_text()
        check = source[source.index("check_ready() {"):source.index("# /openapi.json")]
        payload = '{"db":"ok","environment":"staging","release_sha":"' + "a" * 40 + '","status":"ok"}'
        cases = (("", False), (payload + payload, False), (payload, True),
                 (payload.replace('"ok"', '"bad"', 1), False),
                 (payload.replace('staging', 'production'), False),
                 (payload.replace("a" * 40, "b" * 40), False),
                 (payload[:-1] + ',"extra":true}', False))
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            curl = work / "curl"
            curl.write_text('#!/bin/sh\nprintf "%s" "$PAYLOAD"\n')
            curl.chmod(0o700)
            shell = 'fail() { exit 1; }; timeout() { shift; "$@"; }; '
            shell += 'work=$WORK; environment=staging; release=' + "a" * 40 + '\n'
            shell += check + '\ncheck_ready http://127.0.0.1/health/ready\n'
            for body, expected in cases:
                with self.subTest(body=body):
                    result = subprocess.run(["bash", "-c", shell], capture_output=True, text=True,
                                            env={**os.environ, "PATH": str(work) + os.pathsep + os.environ["PATH"],
                                                 "WORK": str(work), "PAYLOAD": body})
                    self.assertEqual(result.returncode == 0, expected, result.stderr)

    def test_python_route_requires_the_python_schema(self):
        source = SCRIPT.read_text()
        check = source[source.index("check_python_route() {"):source.index("verify_signed_artifact() {")]
        payload = '{"openapi":"3.1.0","info":{"title":"Verdaxis Intelligence Cockpit"},"paths":{"/api/auth/login":{"post":{}}}}'
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            curl = work / "curl"
            curl.write_text('#!/bin/sh\nprintf "%s" "$PAYLOAD"\n')
            curl.chmod(0o700)
            shell = 'fail() { exit 1; }; timeout() { shift; "$@"; }; work=$WORK; ready=https://api-staging.verdaxis.exchange/health/ready\n'
            shell += check + '\ncheck_python_route\n'
            for body, expected in (("", False), (payload + payload, False), ("{}", False),
                                   (payload.replace("Verdaxis Intelligence Cockpit", "native"), False),
                                   (payload, True)):
                with self.subTest(body=body):
                    result = subprocess.run(["bash", "-c", shell], capture_output=True, text=True,
                                            env={**os.environ, "PATH": str(work) + os.pathsep + os.environ["PATH"],
                                                 "WORK": str(work), "PAYLOAD": body})
                    self.assertEqual(result.returncode == 0, expected, result.stderr)

    def test_restore_starts_both_services_before_caddy_reload(self):
        source = SCRIPT.read_text()
        restore = source[source.index('    check_runtime_credentials\n    systemctl daemon-reload'):source.index('# Accept is a new lock-held attempt.')]
        authorize = restore.index('write_state restart-authorized')
        python_restart = restore.index('restart_service "$python_backend"')
        rust_restart = restore.index('restart_service "$backend"')
        python_ready = restore.index('check_ready "$python_ready"')
        rust_ready = restore.index('check_ready "$rust_ready"')
        reload = restore.index('caddy reload')
        public_ready = restore.index('check_ready "$ready"')
        self.assertLess(authorize, python_restart)
        self.assertLess(authorize, rust_restart)
        self.assertLess(python_restart, python_ready)
        self.assertLess(rust_restart, rust_ready)
        self.assertLess(python_ready, reload)
        self.assertLess(rust_ready, reload)
        self.assertLess(reload, public_ready)

    def test_restore_tail_stops_at_each_failed_serving_gate(self):
        source = SCRIPT.read_text()
        restore = source[source.index('    check_runtime_credentials\n    systemctl daemon-reload'):source.index('# Accept is a new lock-held attempt.')]
        tail = restore[restore.index('    write_state restart-authorized'):restore.index("    printf 'RESTORED")]
        expected = ["authorize", "restart:python", "restart:rust", "ready:python",
                    "ready:rust", "reload", "ready:public", "python-route"]
        fixture = '''set -e
fail() { exit 1; }
record() { printf '%s\\n' "$1"; [[ $STOP != "$1" ]]; }
write_state() { record authorize; }
restart_service() { record "restart:$1"; }
check_ready() { record "ready:$1"; }
runuser() { record reload; }
check_python_route() { record python-route; }
python_backend=python; backend=rust; python_ready=python; rust_ready=rust; ready=public; caddy=saved
'''
        with tempfile.TemporaryDirectory() as temporary:
            for stop in ["", *expected]:
                with self.subTest(stop=stop):
                    result = subprocess.run(["bash", "-c", fixture + tail], capture_output=True, text=True,
                                            env={**os.environ, "STOP": stop, "work": temporary})
                    calls = result.stdout.splitlines()
                    # Reload output is kept private by the production tail.
                    visible = [item for item in expected if item != "reload"]
                    if stop:
                        visible = [item for item in expected[:expected.index(stop) + 1] if item != "reload"]
                    self.assertEqual(calls, visible)
                    self.assertEqual(result.returncode == 0, not stop)

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
