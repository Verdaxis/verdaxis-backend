#!/usr/bin/env python3
"""Verify a demo job runs against its configured checked-out release."""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path
from typing import Callable, Mapping


SHA_PATTERN = re.compile(r"[0-9a-f]{40}")
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024


def _git_environment() -> dict[str, str]:
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": "/nonexistent",
        "LC_ALL": "C",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_COUNT": "0",
    }


def _git_head(source_directory: Path) -> str:
    exact_root = source_directory.resolve(strict=True)
    result = subprocess.run(
        [
            "/usr/bin/git",
            "-c",
            f"safe.directory={exact_root}",
            "-C",
            str(exact_root),
            "rev-parse",
            "--show-toplevel",
            "--verify",
            "HEAD",
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=5,
        env=_git_environment(),
    )
    lines = result.stdout.splitlines()
    if len(lines) != 2:
        raise ValueError("Git identity output is invalid")
    reported_root = Path(lines[0]).resolve(strict=True)
    if reported_root != exact_root:
        raise ValueError("source directory is not the exact Git root")
    return lines[1]


def verify_identity(
    source_directory: Path,
    expected_environment: str,
    expected_release_sha: str,
    environ: Mapping[str, str],
    *,
    resolve_head: Callable[[Path], str] = _git_head,
) -> bool:
    if expected_environment not in {"production", "staging"}:
        return False
    if SHA_PATTERN.fullmatch(expected_release_sha) is None or expected_release_sha == "0" * 40:
        return False
    if environ.get("ENVIRONMENT") != expected_environment:
        return False
    if environ.get("RELEASE_SHA") != expected_release_sha:
        return False
    try:
        return resolve_head(source_directory) == expected_release_sha
    except (OSError, ValueError, subprocess.SubprocessError):
        return False


def _extract_attested_commit(
    source_directory: Path, release_sha: str, destination: Path
) -> None:
    exact_root = source_directory.resolve(strict=True)
    archive = destination.parent / f".{destination.name}.tar"
    with archive.open("wb") as handle:
        subprocess.run(
            [
                "/usr/bin/git",
                "-c",
                f"safe.directory={exact_root}",
                "-C",
                str(exact_root),
                "archive",
                "--format=tar",
                release_sha,
            ],
            check=True,
            stdout=handle,
            close_fds=True,
            timeout=60,
            env=_git_environment(),
        )
    try:
        expanded = 0
        with tarfile.open(archive, mode="r:") as bundle:
            members = bundle.getmembers()
            for member in members:
                parts = Path(member.name).parts
                if (
                    not parts
                    or Path(member.name).is_absolute()
                    or ".." in parts
                    or not (member.isdir() or member.isfile())
                ):
                    raise ValueError("attested archive contains an unsafe member")
                expanded += member.size
                if expanded > MAX_ARCHIVE_BYTES:
                    raise ValueError("attested archive is too large")
            for member in members:
                target = destination.joinpath(*Path(member.name).parts)
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True, mode=0o700)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                source = bundle.extractfile(member)
                if source is None:
                    raise ValueError("attested archive member is unreadable")
                with source, target.open("xb") as output:
                    shutil.copyfileobj(source, output, length=1024 * 1024)
        for path in sorted(destination.rglob("*"), reverse=True):
            path.chmod(0o555 if path.is_dir() else 0o444)
        destination.chmod(0o555)
    finally:
        archive.unlink(missing_ok=True)


def _restore_writable(directory: Path) -> None:
    if not directory.exists():
        return
    directory.chmod(0o700)
    for path in directory.rglob("*"):
        path.chmod(0o700 if path.is_dir() else 0o600)


def run_attested_job(
    source_directory: Path,
    expected_environment: str,
    expected_release_sha: str,
    environ: Mapping[str, str],
    *,
    python_executable: Path,
    script: str,
    runtime_directory: Path,
) -> int:
    if not verify_identity(
        source_directory,
        expected_environment,
        expected_release_sha,
        environ,
    ):
        raise ValueError("runtime identity verification failed")
    script_path = Path(script)
    if script_path.is_absolute() or ".." in script_path.parts or not script_path.parts:
        raise ValueError("demo script path is invalid")
    exact_runtime = runtime_directory.resolve(strict=True)
    snapshot = Path(tempfile.mkdtemp(prefix="attested-", dir=exact_runtime))
    try:
        _extract_attested_commit(source_directory, expected_release_sha, snapshot)
        executable_script = snapshot / script_path
        if not executable_script.is_file() or executable_script.is_symlink():
            raise ValueError("demo script is absent from attested commit")
        child_environment = dict(environ)
        child_environment["PYTHONPATH"] = str(snapshot)
        return subprocess.run(
            [str(python_executable), str(executable_script)],
            cwd=snapshot,
            env=child_environment,
            check=False,
            close_fds=True,
        ).returncode
    finally:
        _restore_writable(snapshot)
        if snapshot.exists():
            shutil.rmtree(snapshot, ignore_errors=False)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-directory", type=Path, required=True)
    parser.add_argument("--expected-environment", required=True)
    parser.add_argument("--expected-release-sha", required=True)
    parser.add_argument("--python-executable", type=Path)
    parser.add_argument("--script")
    parser.add_argument("--runtime-directory", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    job_options = (args.python_executable, args.script, args.runtime_directory)
    try:
        if any(job_options):
            if not all(job_options):
                raise ValueError("attested job options are incomplete")
            result = run_attested_job(
                args.source_directory,
                args.expected_environment,
                args.expected_release_sha,
                os.environ,
                python_executable=args.python_executable,
                script=args.script,
                runtime_directory=args.runtime_directory,
            )
            healthy = result == 0
        else:
            healthy = verify_identity(
                args.source_directory,
                args.expected_environment,
                args.expected_release_sha,
                os.environ,
            )
    except (OSError, ValueError, subprocess.SubprocessError, tarfile.TarError):
        healthy = False
    print(f"Verdaxis runtime identity: {'ok' if healthy else 'failure'}")
    return 0 if healthy else 1


if __name__ == "__main__":
    raise SystemExit(main())
