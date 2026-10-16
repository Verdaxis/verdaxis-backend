"""Each PostgreSQL file belongs to exactly one disposable CI runner."""
from pathlib import Path
import re

from tests.phase0.runner import default_test_paths

ROOT = Path(__file__).resolve().parents[2]


def test_postgres_runner_partition_is_complete_and_disjoint():
    runner = (ROOT / "scripts/run_product_analytics_postgres_tests.sh").read_text()
    exclusions = re.findall(r'--ignore=(tests/postgres/test_[\w]+\.py)', runner)
    phase0 = {path for path in default_test_paths() if path.startswith("tests/postgres/")}
    assert set(exclusions) == phase0
    assert len(exclusions) == len(phase0)
    all_tests = {str(path.relative_to(ROOT)) for path in (ROOT / "tests/postgres").glob("test_*.py")}
    analytics = all_tests - set(exclusions)
    assert analytics and phase0
    assert not analytics & phase0
    assert analytics | phase0 == all_tests


def test_ci_runs_complete_phase0_once_with_pinned_history_and_venv():
    workflow = (ROOT / ".github/workflows/backend-ci.yml").read_text()
    assert "  postgres-phase0:" in workflow
    job = workflow.split("  postgres-phase0:", 1)[1]
    assert "fetch-depth: 0" in job
    assert "python -m venv venv" in job
    assert "venv/bin/pip install -r requirements.txt -c constraints.txt" in job
    assert "bash scripts/run_phase0_postgres_tests.sh" in job
    assert "pytest tests/phase0/test_cutover.py" not in workflow
