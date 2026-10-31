#!/usr/bin/env bash
# Linux-only disposable corpus. Never reads an installed signing key.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
python=${ARTIFACT_TEST_PYTHON:-venv/bin/python}
"$python" tests/rust_contract/phase10/artifact/contract.py

# Check the reviewed list and the job itself, including fail-closed lint behavior.
"$python" - <<'PY'
from pathlib import Path
import re
import subprocess
import tempfile
import yaml

root = Path.cwd()
paths = Path('deploy/tests/shellcheck-files.txt').read_text().splitlines()
assert paths and len(paths) == len(set(paths))
assert {'deploy/scripts/verify-artifact.sh', 'deploy/tests/artifact-contract.sh'} <= set(paths)
for path in paths:
    assert re.fullmatch(r'(deploy|scripts)/[A-Za-z0-9_./-]+\.sh', path), path
    assert Path(path).is_file() and not Path(path).is_symlink(), path
# Every active Phase 10 shell file is reviewed; future files join atomically.
for directory in ('deploy/scripts', 'deploy/tests'):
    for path in Path(directory).glob('*.sh'):
        assert str(path) in paths, f'unreviewed script: {path}'
workflow = yaml.safe_load(Path('.github/workflows/rust-read-ci.yml').read_text())
job = workflow['jobs']['phase10-shellcheck']
assert job['runs-on'] == 'ubuntu-22.04'
steps = job['steps']
for step in steps:
    if 'uses' in step:
        assert re.search(r'@[a-f0-9]{40}$', step['uses'])
install = next(s['run'] for s in steps if s.get('name') == 'Install ShellCheck 0.10.0')
assert 'https://github.com/koalaman/shellcheck/releases/download/v0.10.0/shellcheck-v0.10.0.linux.x86_64.tar.xz' in install
assert '6c881ab0698e4e6ea235245f22832860544f17ba386442fe7e9d629f8cbedf87' in install
assert 'sha256sum --check --strict' in install
lint = next(s for s in steps if s.get('name') == 'Check every reviewed shell script')
assert lint['shell'] == 'bash -euo pipefail {0}'
command = lint['run']
for line in (
    '[[ "$(shellcheck --version | grep \'^version:\')" == "version: 0.10.0" ]]',
    'mapfile -t reviewed_scripts < deploy/tests/shellcheck-files.txt',
    '(( ${#reviewed_scripts[@]} > 0 ))',
    'for script in "${reviewed_scripts[@]}"; do bash -n "$script"; done',
    'shellcheck --severity=style --external-sources "${reviewed_scripts[@]}"',
):
    assert line in command, line
assert 'continue-on-error' not in lint and '|| true' not in command
with tempfile.TemporaryDirectory(prefix='lint-contract-') as temp:
    test = Path(temp)
    (test / 'deploy/tests').mkdir(parents=True)
    tools = test / 'tools'
    tools.mkdir()
    fake = tools / 'shellcheck'
    def run(version, status, listed):
        fake.write_text(f'#!/bin/sh\nif [ "$1" = --version ]; then printf "version: {version}\\n"; else exit {status}; fi\n')
        fake.chmod(0o755)
        (test / 'deploy/tests/shellcheck-files.txt').write_text(listed)
        import os
        return subprocess.run(['bash', '-euo', 'pipefail', '-c', command], cwd=test,
                              env=dict(os.environ, PATH=str(tools) + ':' + os.environ['PATH']),
                              capture_output=True).returncode
    (test / 'test.sh').write_text('#!/bin/bash\nexit 0\n')
    assert run('0.10.0', 0, 'test.sh\n') == 0
    assert run('0.10.0', 1, 'test.sh\n') != 0, 'lint failure ignored'
    assert run('0.9.0', 0, 'test.sh\n') != 0, 'wrong version accepted'
    assert run('0.10.0', 0, '') != 0, 'empty list accepted'
    assert run('0.10.0', 0, 'absent.sh\n') != 0, 'missing script accepted'
print('PASS shellcheck_job_pins_version_digest_and_checks_every_reviewed_script')
PY
