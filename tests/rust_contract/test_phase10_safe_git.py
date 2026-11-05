"""Privileged release Git calls admit inert repository configuration only."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "deploy/scripts/safe-root-git.sh"


class SafeRootGit(unittest.TestCase):
    def run_git(self, repo, *args):
        return subprocess.run(
            ["bash", "-c", 'source "$1"; shift; safe_root_git "$@"',
             "fixture", str(HELPER), str(repo), *args],
            capture_output=True, text=True,
        )

    def test_release_entrypoints_share_the_git_boundary(self):
        for relative in ("scripts/deploy.sh", "scripts/install_systemd_units.sh",
                         "scripts/rust_w2_handover.sh", "deploy/scripts/pre-m-bundle.sh"):
            with self.subTest(source=relative):
                source = (ROOT / relative).read_text()
                self.assertIn("safe-root-git.sh", source)
                self.assertIn("safe_root_git", source)
                self.assertNotIn("/usr/bin/git", source)

    def test_executable_configuration_is_refused_before_status(self):
        self.assertTrue(HELPER.is_file(), "shared privileged Git wrapper required")
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            marker = repo / "executed"
            command = repo / "runner"
            command.write_text(f'#!/bin/sh\ntouch "{marker}"\n')
            command.chmod(0o700)
            for key in ("core.fsmonitor", "core.sshCommand", "core.pager",
                        "diff.fixture.command", "filter.fixture.clean", "include.path"):
                with self.subTest(key=key):
                    subprocess.run(["git", "-C", str(repo), "config", key, str(command)], check=True)
                    result = self.run_git(repo, "status", "--porcelain")
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn("inert", result.stderr)
                    self.assertFalse(marker.exists())
                    subprocess.run(["git", "config", "--file", str(repo / ".git/config"), "--no-includes", "--unset", key], check=True)
            self.assertEqual(self.run_git(repo, "status", "--porcelain").returncode, 0)

    def test_ambient_git_configuration_cannot_execute(self):
        self.assertTrue(HELPER.is_file(), "shared privileged Git wrapper required")
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            env = dict(os.environ, GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0="core.fsmonitor",
                       GIT_CONFIG_VALUE_0="/missing-monitor")
            result = subprocess.run(
                ["bash", "-c", 'source "$1"; safe_root_git "$2" status --porcelain',
                 "fixture", str(HELPER), str(repo)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn("missing-monitor", result.stderr)


if __name__ == "__main__":
    unittest.main()
