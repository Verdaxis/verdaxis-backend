"""All Phase 0 database tools use one exact target-secret flag."""
import subprocess
import sys

import pytest


@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
def test_target_environment_flag_is_consistent(tool):
    result = subprocess.run([sys.executable, '-m', f'scripts.phase0.{tool}', '--help'],
                            capture_output=True, text=True)
    assert result.returncode == 0
    assert '--target-environment-file' in result.stdout
    assert '--environment-file' not in result.stdout


@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
def test_abbreviated_flags_are_refused(tool):
    args = ['--manifest', '/missing/manifest', '--target-environment', '/missing/target']
    if tool != 'bootstrap':
        args += ['--source-environment-file', '/missing/source']
    if tool == 'copy':
        args += ['--source-sha', 'a' * 40, '--target-sha', 'b' * 40]
    if tool == 'verify':
        args += ['--report', '/missing/report']
    result = subprocess.run([sys.executable, '-m', f'scripts.phase0.{tool}', *args],
                            capture_output=True, text=True)
    assert result.returncode == 2
