#!/usr/bin/env python3
"""Verify systemd unit bytes come from one exact clean release checkout."""

from __future__ import annotations

import argparse
import hashlib
import io
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile


_FULL_SHA = re.compile(r"[0-9a-f]{40}")
_ENVIRONMENTS = {"production", "staging"}
_UNIT_NAME = re.compile(r"verdaxis-[a-z0-9-]+\.(?:service|timer)")
UNIT_MANIFEST_PATH = "deploy/systemd/runtime-units.manifest"


class SourceProvenanceError(RuntimeError):
    """The proposed systemd source is not the exact approved Git artifact."""


def _git(
    source_root: Path,
    *args: str,
    text: bool = True,
    honor_replacement_refs: bool = False,
):
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("GIT_")
    }
    environment["GIT_CONFIG_GLOBAL"] = "/dev/null"
    environment["GIT_CONFIG_NOSYSTEM"] = "1"
    if honor_replacement_refs:
        environment.pop("GIT_NO_REPLACE_OBJECTS", None)
    else:
        environment["GIT_NO_REPLACE_OBJECTS"] = "1"
    completed = subprocess.run(
        ["/usr/bin/git", "-C", str(source_root), *args],
        capture_output=True,
        check=False,
        env=environment,
        text=text,
    )
    if completed.returncode != 0:
        raise SourceProvenanceError("unable to attest source ref in Git checkout")
    return completed.stdout


def _read_working_unit(path: Path) -> bytes:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as exc:
        raise SourceProvenanceError(
            "approved artifact must be a regular file"
        ) from exc
    with os.fdopen(descriptor, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise SourceProvenanceError(
                "approved artifact must be a regular file"
            )
        return stream.read()


def _read_committed_unit(source_root: Path, source_ref: str, relative: str) -> bytes:
    return _git(source_root, "show", f"{source_ref}:{relative}", text=False)


def _require_regular_committed_blob(
    source_root: Path, source_ref: str, relative: str
) -> None:
    entry = _git(
        source_root, "ls-tree", source_ref, "--", relative
    ).rstrip("\n")
    try:
        metadata, found_path = entry.split("\t", 1)
        mode, object_type, _object_id = metadata.split(" ", 2)
    except ValueError as exc:
        raise SourceProvenanceError(
            "approved artifact must be a regular committed file"
        ) from exc
    if (
        found_path != relative
        or object_type != "blob"
        or mode not in {"100644", "100755"}
    ):
        raise SourceProvenanceError(
            "approved artifact must be a regular committed file"
        )


def _parse_unit_manifest(content: bytes, environment: str) -> tuple[str, ...]:
    selected: list[str] = []
    seen: set[str] = set()
    try:
        lines = content.decode("utf-8").splitlines()
    except UnicodeDecodeError as exc:
        raise SourceProvenanceError("runtime unit manifest is not UTF-8") from exc
    for line_number, raw_line in enumerate(lines, start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 2:
            raise SourceProvenanceError(
                f"invalid runtime unit manifest line {line_number}"
            )
        manifest_environment, unit_name = fields
        wrong_environment_name = (
            manifest_environment == "production" and "-staging." in unit_name
        ) or (
            manifest_environment == "staging" and "-staging." not in unit_name
        )
        if (
            manifest_environment not in _ENVIRONMENTS
            or _UNIT_NAME.fullmatch(unit_name) is None
            or wrong_environment_name
            or unit_name in seen
        ):
            raise SourceProvenanceError(
                "invalid, cross-environment, or duplicate runtime unit entry"
            )
        seen.add(unit_name)
        if manifest_environment == environment:
            selected.append(f"deploy/systemd/{unit_name}")
    if not selected:
        raise SourceProvenanceError(
            "runtime unit manifest has no entries for selected environment"
        )
    return tuple(selected)


def _verified_committed_units(
    *,
    source_root: Path,
    source_ref: str,
    environment: str,
) -> dict[str, bytes]:
    """Attest a clean checkout and return only immutable committed unit bytes."""
    if environment not in _ENVIRONMENTS:
        raise SourceProvenanceError("environment must be production or staging")
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

    _require_regular_committed_blob(root, source_ref, UNIT_MANIFEST_PATH)
    committed_manifest = _read_committed_unit(
        root, source_ref, UNIT_MANIFEST_PATH
    )
    working_manifest = _read_working_unit(root / UNIT_MANIFEST_PATH)
    if hashlib.sha256(working_manifest).digest() != hashlib.sha256(
        committed_manifest
    ).digest():
        raise SourceProvenanceError("runtime unit manifest differs from source ref")

    committed_units: dict[str, bytes] = {
        UNIT_MANIFEST_PATH: committed_manifest,
    }
    for relative in _parse_unit_manifest(committed_manifest, environment):
        _require_regular_committed_blob(root, source_ref, relative)
        working_path = root / relative
        committed_bytes = _read_committed_unit(root, source_ref, relative)
        working_bytes = _read_working_unit(working_path)
        if hashlib.sha256(working_bytes).digest() != hashlib.sha256(
            committed_bytes
        ).digest():
            raise SourceProvenanceError("systemd unit digest differs from source ref")
        committed_units[relative] = committed_bytes
    return committed_units


def verify_source_provenance(
    *,
    source_root: Path,
    source_ref: str,
    environment: str,
) -> dict[str, str]:
    """Return SHA-256 unit digests after exact release provenance checks."""
    committed_units = _verified_committed_units(
        source_root=source_root,
        source_ref=source_ref,
        environment=environment,
    )
    return {
        relative: hashlib.sha256(content).hexdigest()
        for relative, content in committed_units.items()
        if relative != UNIT_MANIFEST_PATH
    }


def _archive_member(
    name: str, content: bytes, *, mode: int
) -> tuple[tarfile.TarInfo, io.BytesIO]:
    member = tarfile.TarInfo(name)
    member.size = len(content)
    member.mode = mode
    member.uid = 0
    member.gid = 0
    member.uname = "root"
    member.gname = "root"
    member.mtime = 0
    return member, io.BytesIO(content)


def build_verified_unit_archive(
    *,
    source_root: Path,
    source_ref: str,
    environment: str,
) -> bytes:
    """Build a deterministic archive exclusively from the approved Git commit."""
    committed_units = _verified_committed_units(
        source_root=source_root,
        source_ref=source_ref,
        environment=environment,
    )
    manifest = "".join(
        f"{hashlib.sha256(content).hexdigest()}  {relative}\n"
        for relative, content in committed_units.items()
    ).encode()
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
        for relative, content in committed_units.items():
            member, stream = _archive_member(relative, content, mode=0o644)
            bundle.addfile(member, stream)
        member, stream = _archive_member("SHA256SUMS", manifest, mode=0o600)
        bundle.addfile(member, stream)
    return archive.getvalue()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-ref", required=True)
    parser.add_argument(
        "--environment", choices=sorted(_ENVIRONMENTS), required=True
    )
    parser.add_argument("--archive", action="store_true")
    args = parser.parse_args()
    try:
        if args.archive:
            archive = build_verified_unit_archive(
                source_root=args.source_root,
                source_ref=args.source_ref,
                environment=args.environment,
            )
            sys.stdout.buffer.write(archive)
            return 0
        digests = verify_source_provenance(
            source_root=args.source_root,
            source_ref=args.source_ref,
            environment=args.environment,
        )
    except (OSError, SourceProvenanceError) as exc:
        print(f"systemd source provenance failed: {exc}", file=sys.stderr)
        return 1
    for relative, digest in digests.items():
        print(f"{relative} sha256={digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
