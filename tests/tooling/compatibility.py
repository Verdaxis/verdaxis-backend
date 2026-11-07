"""Read approved historical inputs for retained tooling compatibility checks."""

import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASELINE = "61bf722dc"


def historical_text(relative: str) -> str:
    """Load committed bytes without importing the removed application."""
    return subprocess.run(
        ["git", "show", f"{BASELINE}:{relative}"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def stage_budget_source(directory: Path) -> Path:
    """Keep the unchanged budget helper beside its approved Settings input."""
    scripts = directory / "scripts"
    scripts.mkdir(parents=True)
    (scripts / "prepare_rust_budget.py").write_bytes(
        (ROOT / "scripts/prepare_rust_budget.py").read_bytes()
    )
    application = directory / "app"
    application.mkdir()
    (application / "__init__.py").write_text("")
    (application / "config.py").write_text(historical_text("app/config.py"))
    return scripts / "prepare_rust_budget.py"
