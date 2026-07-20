#!/usr/bin/env python3
"""Verify systemd unit bytes come from one exact clean release checkout."""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
from typing import Iterable


_FULL_SHA = re.compile(r"[0-9a-f]{40}")
_MODES = {"dry-run", "apply"}


class SourceProvenanceError(RuntimeError):
    """The proposed systemd source is not the exact approved Git artifact."""


def _git(
    source_root: Path,
    *args: str,
    text: bool = True,
    honor_replacement_refs: bool = False,
):
    environment = os.environ.copy()
    if honor_replacement_refs:
        environment.pop("GIT_NO_REPLACE_OBJECTS", None)
    else:
        environment["GIT_NO_REPLACE_OBJECTS"] = "1"
    completed = subprocess.run(
        ["git", "-C", str(source_root), *args],
        capture_output=True,
        check=False,
        env=environment,
        text=text,
    )
    if completed.returncode != 0:
        raise SourceProvenanceError("unable to attest source ref in Git checkout")
    return completed.stdout


def _validated_unit_paths(unit_paths: Iterable[str]) -> tuple[str, ...]:
    validated: list[str] = []
    for raw_path in unit_paths:
        path = PurePosixPath(raw_path)
        if (
            path.is_absolute()
            or ".." in path.parts
            or len(path.parts) != 3
            or path.parts[:2] != ("deploy", "systemd")
            or path.suffix not in {".service", ".timer"}
        ):
            raise SourceProvenanceError("unit path is outside deploy/systemd")
        normalized = path.as_posix()
        if normalized in validated:
            raise SourceProvenanceError("unit path is duplicated")
        validated.append(normalized)
    if not validated:
        raise SourceProvenanceError("at least one unit path is required")
    return tuple(validated)


def verify_source_provenance(
    *,
    source_root: Path,
    source_ref: str,
    unit_paths: Iterable[str],
    mode: str,
) -> dict[str, str]:
    """Return SHA-256 unit digests after exact release provenance checks."""
    if mode not in _MODES:
        raise SourceProvenanceError("installation mode must be dry-run or apply")
    if _FULL_SHA.fullmatch(source_ref) is None:
        raise SourceProvenanceError("source ref must be a full lowercase commit SHA")

    root = source_root.resolve(strict=True)
    top_level = Path(_git(root, "rev-parse", "--show-toplevel").strip()).resolve()
    if top_level != root:
        raise SourceProvenanceError("source root must be the Git checkout root")

    replacement_refs = _git(
        root, "replace", "-l", honor_replacement_refs=True
    ).strip()
    if replacement_refs:
        raise SourceProvenanceError("source checkout contains Git replacement refs")

    head = _git(root, "rev-parse", "HEAD").strip()
    if head != source_ref:
        raise SourceProvenanceError("source ref does not match checkout HEAD")
    tracked_flags = _git(root, "ls-files", "-v", "--").splitlines()
    if any(line and (line[0].islower() or line[0] == "S") for line in tracked_flags):
        raise SourceProvenanceError(
            "source checkout contains hidden tracked-file index flags"
        )
    dirty = _git(root, "status", "--porcelain", "--untracked-files=all")
    if dirty:
        raise SourceProvenanceError("source checkout is dirty")

    digests: dict[str, str] = {}
    for relative in _validated_unit_paths(unit_paths):
        working_path = root / relative
        if not working_path.is_file():
            raise SourceProvenanceError("approved release is missing a systemd unit")
        committed_bytes = _git(root, "show", f"{source_ref}:{relative}", text=False)
        working_bytes = working_path.read_bytes()
        committed_digest = hashlib.sha256(committed_bytes).hexdigest()
        working_digest = hashlib.sha256(working_bytes).hexdigest()
        if working_digest != committed_digest:
            raise SourceProvenanceError("systemd unit digest differs from source ref")
        digests[relative] = working_digest
    return digests


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-ref", required=True)
    parser.add_argument("--mode", choices=sorted(_MODES), required=True)
    parser.add_argument("--unit", action="append", dest="units", required=True)
    args = parser.parse_args()
    try:
        digests = verify_source_provenance(
            source_root=args.source_root,
            source_ref=args.source_ref,
            unit_paths=args.units,
            mode=args.mode,
        )
    except (OSError, SourceProvenanceError) as exc:
        print(f"systemd source provenance failed: {exc}", file=sys.stderr)
        return 1
    for relative, digest in digests.items():
        print(f"{relative} sha256={digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
