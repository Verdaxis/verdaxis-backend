import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def test_operator_preflight_imports_from_repo_root_without_pythonpath():
    completed = subprocess.run(
        [sys.executable, "-m", "scripts.security_preflight", "--check-imports"],
        cwd=ROOT,
        env={"PATH": str(Path(sys.executable).parent)},
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.strip() == "security_preflight_imports=READY"


def test_operator_runbook_is_read_only_and_holds_approval_actions():
    runbook = (ROOT / "docs" / "runbooks" / "security-v2-operator-review.md").read_text()
    assert "python -m scripts.security_preflight" in runbook
    assert "read-only" in runbook.lower()
    assert "Do not auto-approve" in runbook
