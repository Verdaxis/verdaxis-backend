"""Keep the Rust-owned reset table limited to migration metadata."""
import ast
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_reset_token_model_has_no_application_imports():
    for path in (ROOT / "app").rglob("*.py"):
        if (ROOT / "app/models") in path.parents:
            continue
        for node in ast.walk(ast.parse(path.read_text())):
            if isinstance(node, ast.ImportFrom):
                assert "password_reset_token" not in (node.module or ""), path
                assert all(alias.name != "PasswordResetToken" for alias in node.names), path
            elif isinstance(node, ast.Import):
                assert all("password_reset_token" not in alias.name for alias in node.names), path
