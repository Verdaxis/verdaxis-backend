from __future__ import annotations

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/verify_runtime_identity.py"
SHA = "a" * 40


def load_module():
    assert MODULE_PATH.exists(), "runtime identity verifier has not been implemented"
    spec = importlib.util.spec_from_file_location("verify_runtime_identity", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_runtime_identity_requires_environment_release_and_checked_out_head(tmp_path):
    verifier = load_module()
    environ = {"ENVIRONMENT": "production", "RELEASE_SHA": SHA}

    assert verifier.verify_identity(
        tmp_path, "production", SHA, environ, resolve_head=lambda _path: SHA
    )
    assert not verifier.verify_identity(
        tmp_path, "staging", SHA, environ, resolve_head=lambda _path: SHA
    )
    assert not verifier.verify_identity(
        tmp_path, "production", "b" * 40, environ, resolve_head=lambda _path: SHA
    )
    assert not verifier.verify_identity(
        tmp_path, "production", SHA, environ, resolve_head=lambda _path: "b" * 40
    )


def _git_repository(path: Path) -> str:
    subprocess.run(["/usr/bin/git", "init", "-q", str(path)], check=True)
    subprocess.run(
        ["/usr/bin/git", "-C", str(path), "config", "user.email", "demo@test.invalid"],
        check=True,
    )
    subprocess.run(
        ["/usr/bin/git", "-C", str(path), "config", "user.name", "Demo Test"],
        check=True,
    )
    (path / "tracked.txt").write_text("approved\n")
    subprocess.run(["/usr/bin/git", "-C", str(path), "add", "tracked.txt"], check=True)
    subprocess.run(
        ["/usr/bin/git", "-C", str(path), "commit", "-qm", "approved"],
        check=True,
    )
    return subprocess.check_output(
        ["/usr/bin/git", "-C", str(path), "rev-parse", "HEAD"], text=True
    ).strip()


def _demo_repository(path: Path) -> str:
    subprocess.run(["/usr/bin/git", "init", "-q", str(path)], check=True)
    subprocess.run(
        ["/usr/bin/git", "-C", str(path), "config", "user.email", "demo@test.invalid"],
        check=True,
    )
    subprocess.run(
        ["/usr/bin/git", "-C", str(path), "config", "user.name", "Demo Test"],
        check=True,
    )
    package = path / "app"
    package.mkdir()
    (package / "__init__.py").write_text("")
    (package / "value.py").write_text("VALUE = 'attested'\n")
    script = path / "scripts/run_demo_activity.py"
    script.parent.mkdir()
    script.write_text(
        "import os\nfrom pathlib import Path\nfrom app.value import VALUE\n"
        "Path(os.environ['DEMO_OUTPUT']).write_text(VALUE)\n"
    )
    subprocess.run(["/usr/bin/git", "-C", str(path), "add", "."], check=True)
    subprocess.run(
        ["/usr/bin/git", "-C", str(path), "commit", "-qm", "approved demo"],
        check=True,
    )
    return subprocess.check_output(
        ["/usr/bin/git", "-C", str(path), "rev-parse", "HEAD"], text=True
    ).strip()


def test_runtime_identity_rejects_a_subdirectory_instead_of_exact_repo_root(tmp_path):
    verifier = load_module()
    sha = _git_repository(tmp_path)
    nested = tmp_path / "nested"
    nested.mkdir()

    assert not verifier.verify_identity(
        nested,
        "production",
        sha,
        {"ENVIRONMENT": "production", "RELEASE_SHA": sha},
    )


def _run_real_verifier(
    repository: Path, environment: str, expected_sha: str
) -> subprocess.CompletedProcess:
    return subprocess.run(
        [
            "/usr/bin/env",
            f"ENVIRONMENT={environment}",
            f"RELEASE_SHA={expected_sha}",
            "/usr/bin/python3",
            str(MODULE_PATH),
            "--source-directory",
            str(repository),
            "--expected-environment",
            environment,
            "--expected-release-sha",
            expected_sha,
        ],
        check=False,
        capture_output=True,
        text=True,
    )


def test_disposable_dual_targets_are_verified_independently_and_never_mutated(
    tmp_path,
):
    production = tmp_path / "production"
    staging = tmp_path / "staging"
    production.mkdir()
    staging.mkdir()
    production_sha = _git_repository(production)
    staging_sha = _git_repository(staging)
    wrong_staging_sha = "b" * 40
    if wrong_staging_sha == staging_sha:
        wrong_staging_sha = "c" * 40

    failed_staging = _run_real_verifier(
        staging, "staging", wrong_staging_sha
    )
    healthy_production = _run_real_verifier(
        production, "production", production_sha
    )

    assert failed_staging.returncode == 1
    assert healthy_production.returncode == 0
    assert healthy_production.stdout.strip() == "Verdaxis runtime identity: ok"
    assert subprocess.check_output(
        ["/usr/bin/git", "-C", str(production), "status", "--porcelain"],
        text=True,
    ) == ""
    assert subprocess.check_output(
        ["/usr/bin/git", "-C", str(staging), "status", "--porcelain"],
        text=True,
    ) == ""
    assert production.joinpath("tracked.txt").read_text() == "approved\n"
    assert staging.joinpath("tracked.txt").read_text() == "approved\n"


def test_demo_job_executes_attested_commit_not_dirty_mutable_checkout(tmp_path):
    verifier = load_module()
    repository = tmp_path / "repository"
    repository.mkdir()
    sha = _demo_repository(repository)
    output = tmp_path / "result.txt"
    runtime_directory = tmp_path / "runtime"
    runtime_directory.mkdir()
    (repository / "scripts/run_demo_activity.py").write_text(
        "import os\nfrom pathlib import Path\n"
        "Path(os.environ['DEMO_OUTPUT']).write_text('dirty')\n"
    )
    (repository / "app/value.py").write_text("VALUE = 'dirty'\n")
    environ = {
        "ENVIRONMENT": "production",
        "RELEASE_SHA": sha,
        "DEMO_OUTPUT": str(output),
    }

    result = verifier.run_attested_job(
        repository,
        "production",
        sha,
        environ,
        python_executable=Path(sys.executable),
        script="scripts/run_demo_activity.py",
        runtime_directory=runtime_directory,
    )

    assert result == 0
    assert output.read_text() == "attested"
    assert not list(runtime_directory.iterdir())


def test_runtime_identity_real_git_succeeds_under_a_different_uid_with_fixed_safe_directory():
    if os.geteuid() == 0 or shutil.which("sudo") is None:
        pytest.skip("different-UID boundary requires non-root sudo")
    probe = subprocess.run(
        ["sudo", "-n", "-u", "nobody", "/usr/bin/true"],
        capture_output=True,
        check=False,
    )
    if probe.returncode != 0:
        pytest.skip("passwordless nobody execution is unavailable")

    temporary = Path(tempfile.mkdtemp(prefix="verdaxis-demo-git-", dir="/tmp"))
    try:
        temporary.chmod(0o755)
        sha = _git_repository(temporary)
        verifier_copy = temporary / "verify_runtime_identity.py"
        shutil.copy2(MODULE_PATH, verifier_copy)
        verifier_copy.chmod(0o755)
        for directory in [temporary / ".git", *(temporary / ".git").rglob("*")]:
            if directory.is_dir():
                directory.chmod(0o755)
            elif directory.is_file():
                directory.chmod(0o644)
        (temporary / "tracked.txt").chmod(0o644)

        completed = subprocess.run(
            [
                "sudo",
                "-n",
                "-u",
                "nobody",
                "/usr/bin/env",
                "ENVIRONMENT=production",
                f"RELEASE_SHA={sha}",
                "/usr/bin/python3",
                str(verifier_copy),
                "--source-directory",
                str(temporary),
                "--expected-environment",
                "production",
                "--expected-release-sha",
                sha,
            ],
            capture_output=True,
            text=True,
            check=False,
        )

        assert completed.returncode == 0, completed.stderr + completed.stdout
        assert completed.stdout.strip() == "Verdaxis runtime identity: ok"
    finally:
        shutil.rmtree(temporary)
