"""Run root-owned authority primitives in an offline, disposable Linux runtime.

This is not production execution acceptance. It uses only a committed source
archive, no checkout bind mounts, Docker socket, .env, or host credentials.
"""
from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path
from uuid import uuid4

ROOT = Path(__file__).resolve().parents[2]
from tests.phase0.runtime_images import runtime_image

IMAGE, PLATFORM = runtime_image('amd64')
TESTS = (
    'tests/unit/test_phase0_host_authority.py',
    'tests/unit/test_phase0_report_publication.py',
    'tests/unit/test_phase0_tool_interfaces.py',
)


def main():
    name = 'phase0-hosttools-linux-' + uuid4().hex[:12]
    work = ROOT / 'work'
    work.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='linux-authority-', dir=work) as temporary:
        archive = Path(temporary) / 'source.tar'
        with archive.open('wb') as stream:
            subprocess.run(['git', '-C', str(ROOT), 'archive', 'HEAD'], stdout=stream, check=True)
        created = False
        try:
            subprocess.run([
                'docker', 'create', '--name', name, '--platform', PLATFORM,
                '--network', 'none', '--cap-drop', 'ALL',
                '--security-opt', 'no-new-privileges',
                '-e', 'ENVIRONMENT=test', '-e', 'RELEASE_SHA=test',
                '-e', 'JWT_SECRET=test-secret-key-for-testing-minimum-32-chars',
                '-e', 'DATABASE_URL=sqlite+aiosqlite:///:memory:',
                IMAGE, '/bin/sh', '-ec',
                'tar -xf /source.tar -C /app; '
                'mkdir -m 700 /root/phase0-proof; '
                'python -c "from pathlib import Path; '
                'from scripts.phase0.host_authority import read_root_private; '
                "p=Path('/root/phase0-proof/authority.json'); "
                "p.write_text('{}'); p.chmod(0o600); "
                "assert read_root_private(p)==b'{}'" + '"; '
                'python -m pytest -q ' + ' '.join(TESTS),
            ], check=True, stdout=subprocess.DEVNULL)
            created = True
            subprocess.run(['docker', 'cp', str(archive), name + ':/source.tar'], check=True)
            result = subprocess.run(['docker', 'start', '-a', name])
            status = subprocess.run(['docker', 'inspect', '--format', '{{.State.ExitCode}}', name],
                                    check=True, capture_output=True, text=True)
            return result.returncode or int(status.stdout.strip())
        finally:
            if created:
                subprocess.run(['docker', 'rm', '-f', '-v', name], check=True)


if __name__ == '__main__':
    raise SystemExit(main())
