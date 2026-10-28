#!/usr/bin/env python3
"""Verify a Rust read artifact before anything extracts or executes it.

Four modes share one definition of what the artifact is:

* the default mode verifies a release archive on the deploy host, then extracts
  it into an empty output directory;
* ``--attest`` runs on the PM workstation, calls the ``gh`` CLI to check GitHub
  artifact attestation, and writes a signed provenance record;
* ``--package`` runs in CI and builds the archive plus its metadata from the
  built binary and the reviewed ``deploy/rust`` data files;
* ``--glibc-check`` reads ``objdump -T`` output and refuses a binary that needs a
  glibc symbol newer than the recorded floor.

Provenance is split so the deploy host needs no GitHub tooling and no network
trust of its own. The workstation holds ``gh`` and the signing key; the host
verifies the workstation's signature over the record and then checks that the
record, the metadata inside the archive, and the extracted bytes all agree. A
digest that came from the same download as the archive never stands alone:
the archive digest is bound by the GitHub attestation the workstation verified,
and every inner digest is then bound to that archive digest.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile

REPOSITORY = "Verdaxis/verdaxis-backend"
WORKFLOW_PATH = ".github/workflows/rust-read-ci.yml"
DEFAULT_TARGET = "x86_64-unknown-linux-gnu"
LIBC_MIN = "2.35"
BINARY_NAME = "verdaxis-read"
METADATA_NAME = "artifact-metadata.json"
SUMS_NAME = "SHA256SUMS"
DATA_PREFIX = "data/"
SCHEMA_PATH = Path(__file__).resolve().parents[1] / "deploy" / "rust" / "artifact.schema.json"

# Bounds. The archive is read whole into memory so the bytes that are hashed are
# the bytes that are parsed, and every inner read is bounded by these limits.
MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
MAX_MEMBER_BYTES = 128 * 1024 * 1024
MAX_MEMBERS = 64

# Operator-checked host facts (PM, 16 Oct). The target is chosen for the oldest
# of these, so the build runs on both.
RUNTIME_HOSTS = {
    "staging": {"arch": "x86_64", "glibc": "2.35"},
    "production": {"arch": "x86_64", "glibc": "2.39"},
}

_FULL_SHA = re.compile(r"[0-9a-f]{40}")
_SHA256 = re.compile(r"[0-9a-f]{64}")
_VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
_REVISION = re.compile(r"[a-z0-9_]+")
_DATA_MEMBER = re.compile(r"data/[A-Za-z0-9._-]+")
_GLIBC_TAG = re.compile(r"GLIBC_([0-9]+)\.([0-9]+)")
_HOST_GLIBC = re.compile(r"[0-9]+\.[0-9]+")

# Members every archive must carry, plus the data members the metadata lists.
FIXED_MEMBERS = (BINARY_NAME, METADATA_NAME, SUMS_NAME)


class ArtifactError(RuntimeError):
    """The artifact, its provenance, or its metadata is not acceptable."""


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ArtifactError(message)


def _matches(pattern: re.Pattern, value: object) -> bool:
    """A text field matches its pattern. Non-strings never match, and never
    raise: a malformed document must be refused, not crash the verifier."""
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def _is_int(value: object) -> bool:
    """A real integer, never a bool."""
    return isinstance(value, int) and not isinstance(value, bool)


# --------------------------------------------------------------------------- #
# Metadata validation
# --------------------------------------------------------------------------- #
def validate_metadata(document: object) -> dict:
    """Validate artifact-metadata.json against deploy/rust/artifact.schema.json.

    The schema file is the written contract; this function is its executable
    form. tests/unit/test_rust_artifact.py asserts the two agree, so the schema
    cannot silently drift from the enforced rules.
    """
    _require(isinstance(document, dict), "artifact metadata must be a JSON object")
    expected = {
        "version",
        "release_sha",
        "target",
        "libc_min",
        "toolchain",
        "lockfile_sha256",
        "workflow_path",
        "binary",
        "policy",
        "revision_allowlist",
        "files",
        "runtime_hosts",
    }
    _require(set(document) == expected, "artifact metadata has unexpected or missing fields")
    _require(_is_int(document["version"]) and document["version"] == 1, "artifact metadata version must be 1")
    _require(
        _matches(_FULL_SHA, document["release_sha"]),
        "release_sha must be a full lowercase commit SHA",
    )
    _require(document["target"] == DEFAULT_TARGET, "unexpected build target")
    _require(document["libc_min"] == LIBC_MIN, "unexpected libc floor")
    _require(
        _matches(_VERSION, document["toolchain"]),
        "toolchain must be a three-part version",
    )
    _require(
        _matches(_SHA256, document["lockfile_sha256"]),
        "lockfile_sha256 must be a SHA-256 digest",
    )
    _require(
        document["workflow_path"] == WORKFLOW_PATH,
        "artifact must be published only by the reviewed workflow",
    )

    binary = document["binary"]
    _require(isinstance(binary, dict) and set(binary) == {"path", "sha256"}, "invalid binary block")
    _require(binary["path"] == BINARY_NAME, "binary member has an unexpected name")
    _require(_matches(_SHA256, binary["sha256"]), "binary digest is invalid")

    policy = document["policy"]
    _require(
        isinstance(policy, dict)
        and set(policy)
        == {"source_sha", "policy_path", "policy_sha256", "manifest_path", "manifest_sha256"},
        "invalid policy block",
    )
    _require(_matches(_FULL_SHA, policy["source_sha"]), "policy source SHA is invalid")
    _require(policy["policy_path"] == "data/market-policy.json", "policy path is unexpected")
    _require(
        policy["manifest_path"] == "data/market-policy.manifest.json",
        "policy manifest path is unexpected",
    )
    for field in ("policy_sha256", "manifest_sha256"):
        _require(_matches(_SHA256, policy[field]), f"{field} is invalid")

    allowlist = document["revision_allowlist"]
    _require(
        isinstance(allowlist, dict) and set(allowlist) == {"path", "sha256", "revisions"},
        "invalid revision allowlist block",
    )
    _require(allowlist["path"] == "data/allowed-revisions.json", "allowlist path is unexpected")
    _require(_matches(_SHA256, allowlist["sha256"]), "allowlist digest is invalid")
    _require(
        isinstance(allowlist["revisions"], list) and allowlist["revisions"],
        "revision allowlist must be a non-empty list",
    )
    for revision in allowlist["revisions"]:
        _require(_matches(_REVISION, revision), "revision allowlist holds an invalid entry")

    files = document["files"]
    _require(isinstance(files, list) and files, "files must be a non-empty list")
    seen: set[str] = set()
    for entry in files:
        _require(
            isinstance(entry, dict) and set(entry) == {"path", "sha256"},
            "invalid file entry",
        )
        _require(_matches(_DATA_MEMBER, entry["path"]), "file entry path is invalid")
        _require(_matches(_SHA256, entry["sha256"]), "file entry digest is invalid")
        _require(entry["path"] not in seen, "duplicate file entry")
        seen.add(entry["path"])

    hosts = document["runtime_hosts"]
    _require(
        isinstance(hosts, dict) and set(hosts) == {"staging", "production"},
        "runtime_hosts must name staging and production",
    )
    for name, host in hosts.items():
        _require(
            isinstance(host, dict) and set(host) == {"arch", "glibc"},
            f"{name} host record is invalid",
        )
        _require(host["arch"] == "x86_64", f"{name} host arch must be x86_64")
        _require(
            _matches(_HOST_GLIBC, host["glibc"]),
            f"{name} host glibc is invalid",
        )
    return document


def _glibc_tuple(text: str) -> tuple[int, int]:
    tags = [tuple(int(part) for part in match.groups()) for match in _GLIBC_TAG.finditer(text)]
    return max(tags) if tags else (0, 0)


def check_glibc_max(objdump_text: str, maximum: str) -> str:
    """Return the highest glibc version tag and refuse one above ``maximum``."""
    observed = _glibc_tuple(objdump_text)
    _require(observed != (0, 0), "no glibc symbol version tags found; not a glibc build")
    ceiling = tuple(int(part) for part in maximum.split("."))
    _require(
        observed <= ceiling,
        f"binary needs glibc {observed[0]}.{observed[1]}, above the {maximum} floor",
    )
    return f"{observed[0]}.{observed[1]}"


# --------------------------------------------------------------------------- #
# Archive inspection
# --------------------------------------------------------------------------- #
def _archive_members(archive: tarfile.TarFile) -> dict[str, tarfile.TarInfo]:
    members: dict[str, tarfile.TarInfo] = {}
    entries = archive.getmembers()
    _require(len(entries) <= MAX_MEMBERS, "archive has too many members")
    total = 0
    for member in entries:
        name = member.name
        _require(bool(name), "archive has an unnamed member")
        _require(not member.isdir(), f"archive member {name!r} is a directory")
        _require(member.isfile(), f"archive member {name!r} is not a regular file")
        _require(name == name.strip(), f"archive member {name!r} has surrounding whitespace")
        _require("\\" not in name, f"archive member {name!r} contains a backslash")
        _require("\x00" not in name, "archive member name contains a NUL byte")
        _require(not name.startswith("/"), f"archive member {name!r} is an absolute path")
        parts = name.split("/")
        _require(
            all(part not in ("", ".", "..") for part in parts),
            f"archive member {name!r} traverses or is not a plain relative path",
        )
        _require(name not in members, f"archive member {name!r} is duplicated")
        _require(member.size <= MAX_MEMBER_BYTES, f"archive member {name!r} is oversized")
        total += member.size
        _require(total <= MAX_ARCHIVE_BYTES, "archive members exceed the total size bound")
        members[name] = member
    return members


def _read_members(archive: tarfile.TarFile, names: set[str]) -> dict[str, bytes]:
    contents: dict[str, bytes] = {}
    for name in names:
        handle = archive.extractfile(name)
        _require(handle is not None, f"archive member {name!r} cannot be read")
        contents[name] = handle.read(MAX_MEMBER_BYTES + 1)
        _require(
            len(contents[name]) <= MAX_MEMBER_BYTES,
            f"archive member {name!r} exceeds the per-member size bound",
        )
    return contents


def _parse_sums(sums: bytes) -> dict[str, str]:
    try:
        text = sums.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ArtifactError("SHA256SUMS is not UTF-8") from exc
    entries: dict[str, str] = {}
    for line in text.splitlines():
        if not line:
            continue
        fields = line.split("  ")
        _require(len(fields) == 2, "SHA256SUMS has a malformed line")
        digest, name = fields
        _require(_SHA256.fullmatch(digest or "") is not None, "SHA256SUMS has a malformed digest")
        _require(name not in entries, "SHA256SUMS repeats a member")
        entries[name] = digest
    return entries


def inspect_archive(data: bytes, source_sha: str, target: str) -> dict:
    """Validate one archive in full. Returns the trusted facts about it.

    Nothing is written to disk here: every member is read into bounded memory.
    """
    _require(len(data) <= MAX_ARCHIVE_BYTES, "archive exceeds the size bound")
    try:
        archive = tarfile.open(fileobj=io.BytesIO(data), mode="r:")
    except tarfile.TarError as exc:
        raise ArtifactError("archive is not an uncompressed tar") from exc
    with archive:
        members = _archive_members(archive)
        _require(BINARY_NAME in members, "archive is missing the release binary")
        _require(METADATA_NAME in members, "archive is missing artifact metadata")
        contents = _read_members(archive, set(members))

    try:
        document = json.loads(contents[METADATA_NAME])
    except (ValueError, UnicodeDecodeError) as exc:
        raise ArtifactError("artifact metadata is not valid JSON") from exc
    metadata = validate_metadata(document)
    _require(metadata["release_sha"] == source_sha, "metadata source SHA does not match --source-sha")
    _require(metadata["target"] == target, "metadata target does not match --target")

    expected = {BINARY_NAME, METADATA_NAME, SUMS_NAME} | {
        entry["path"] for entry in metadata["files"]
    }
    _require(
        set(members) == expected,
        "archive member set does not match the metadata "
        f"(extra={sorted(set(members) - expected)}, missing={sorted(expected - set(members))})",
    )

    binary_digest = sha256_bytes(contents[BINARY_NAME])
    _require(
        binary_digest == metadata["binary"]["sha256"],
        "binary digest does not match the metadata",
    )

    sums = _parse_sums(contents[SUMS_NAME])
    _require(
        set(sums) == expected - {SUMS_NAME},
        "SHA256SUMS does not cover exactly the archive members",
    )
    for name, digest in sums.items():
        _require(sha256_bytes(contents[name]) == digest, f"SHA256SUMS digest mismatch for {name!r}")

    for entry in metadata["files"]:
        _require(entry["path"] in contents, f"metadata names an absent member {entry['path']!r}")
        _require(
            sha256_bytes(contents[entry["path"]]) == entry["sha256"],
            f"metadata digest mismatch for {entry['path']!r}",
        )

    policy = metadata["policy"]
    _require(
        sha256_bytes(contents[policy["policy_path"]]) == policy["policy_sha256"],
        "market policy digest does not match the metadata",
    )
    _require(
        sha256_bytes(contents[policy["manifest_path"]]) == policy["manifest_sha256"],
        "market policy manifest digest does not match the metadata",
    )
    try:
        manifest = json.loads(contents[policy["manifest_path"]])
    except (ValueError, UnicodeDecodeError) as exc:
        raise ArtifactError("market policy manifest is not valid JSON") from exc
    _require(
        isinstance(manifest, dict) and manifest.get("source_sha") == policy["source_sha"],
        "policy manifest and metadata disagree on the oracle source SHA",
    )
    _require(
        manifest.get("policy_sha256") == policy["policy_sha256"],
        "policy manifest and metadata disagree on the policy digest",
    )

    allowlist = metadata["revision_allowlist"]
    _require(
        sha256_bytes(contents[allowlist["path"]]) == allowlist["sha256"],
        "revision allowlist digest does not match the metadata",
    )
    try:
        revisions = json.loads(contents[allowlist["path"]])
    except (ValueError, UnicodeDecodeError) as exc:
        raise ArtifactError("revision allowlist is not valid JSON") from exc
    _require(revisions == allowlist["revisions"], "revision allowlist content does not match")

    return {
        "archive_sha256": sha256_bytes(data),
        "binary_sha256": binary_digest,
        "binary": contents[BINARY_NAME],
        "metadata": metadata,
        "members": {name: contents[name] for name in expected},
    }


# --------------------------------------------------------------------------- #
# Provenance
# --------------------------------------------------------------------------- #
def _load_public_key(path: Path):
    try:
        from cryptography.hazmat.primitives.serialization import load_pem_public_key
    except ImportError as exc:  # pragma: no cover - cryptography is a runtime dependency
        raise ArtifactError("cryptography is required to verify provenance") from exc
    try:
        return load_pem_public_key(_read_regular(path))
    except (ValueError, OSError) as exc:
        raise ArtifactError("provenance public key is not a readable PEM public key") from exc


def _read_regular(path: Path) -> bytes:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as exc:
        raise ArtifactError(f"{path} must be a regular file") from exc
    with os.fdopen(descriptor, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ArtifactError(f"{path} must be a regular file")
        return stream.read()


def verify_provenance(
    record_bytes: bytes,
    signature: bytes,
    public_key_path: Path,
    *,
    expected: dict,
) -> dict:
    """Verify the workstation's signed record and every binding it carries."""
    try:
        from cryptography.exceptions import InvalidSignature
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    except ImportError as exc:  # pragma: no cover
        raise ArtifactError("cryptography is required to verify provenance") from exc
    key = _load_public_key(public_key_path)
    _require(isinstance(key, Ed25519PublicKey), "provenance key must be an Ed25519 public key")
    try:
        key.verify(signature, record_bytes)
    except InvalidSignature as exc:
        raise ArtifactError("provenance record signature is not valid") from exc

    try:
        record = json.loads(record_bytes)
    except (ValueError, UnicodeDecodeError) as exc:
        raise ArtifactError("provenance record is not valid JSON") from exc
    _require(isinstance(record, dict), "provenance record must be a JSON object")
    required = {
        "version",
        "repository",
        "workflow_path",
        "workflow_ref",
        "source_sha",
        "target",
        "archive_sha256",
        "binary_sha256",
        "attested_subjects",
        "gh_attestation_verified",
        "attested_at",
    }
    _require(set(record) == required, "provenance record has unexpected or missing fields")
    _require(record["version"] == 1, "provenance record version must be 1")
    _require(record["gh_attestation_verified"] is True, "attestation was not verified")
    _require(
        record["attested_subjects"] == [BINARY_NAME, "archive"],
        "provenance record must attest both the binary and the archive",
    )
    for field, value in expected.items():
        _require(record[field] == value, f"provenance record {field} does not match")
    return record


def run_attest(args: argparse.Namespace) -> int:
    """Workstation mode: check GitHub attestation and write a signed record."""
    if shutil_which("gh") is None:
        raise ArtifactError(
            "gh is required to check attestation; install it on the workstation, never on the host"
        )
    archive_path = args.archive
    archive_bytes = _read_regular(archive_path)
    inspected = inspect_archive(archive_bytes, args.source_sha, args.target)
    binary_bytes = _read_regular(args.binary)
    _require(
        sha256_bytes(binary_bytes) == inspected["binary_sha256"],
        "the loose binary does not match the binary inside the archive",
    )
    for subject in (args.binary, archive_path):
        completed = subprocess.run(
            [
                "gh",
                "attestation",
                "verify",
                str(subject),
                "--repo",
                args.repository,
                "--signer-workflow",
                f"{args.repository}/{args.workflow_path}",
                "--source-ref",
                args.workflow_ref,
                "--source-digest",
                args.source_sha,
            ],
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            raise ArtifactError(
                f"gh attestation verify failed for {subject}: {completed.stderr.strip()}"
            )
    record = {
        "version": 1,
        "repository": args.repository,
        "workflow_path": args.workflow_path,
        "workflow_ref": args.workflow_ref,
        "source_sha": args.source_sha,
        "target": args.target,
        "archive_sha256": inspected["archive_sha256"],
        "binary_sha256": inspected["binary_sha256"],
        "attested_subjects": [BINARY_NAME, "archive"],
        "gh_attestation_verified": True,
        "attested_at": args.attested_at,
    }
    record_bytes = json.dumps(record, indent=2, sort_keys=True).encode() + b"\n"
    _sign_record(record_bytes, args.signing_key, args.record_out, args.signature_out)
    print(f"provenance record written to {args.record_out}")
    return 0


def _sign_record(record_bytes: bytes, signing_key: Path, record_out: Path, signature_out: Path) -> None:
    try:
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        from cryptography.hazmat.primitives.serialization import load_pem_private_key
    except ImportError as exc:  # pragma: no cover
        raise ArtifactError("cryptography is required to sign provenance") from exc
    key = load_pem_private_key(_read_regular(signing_key), password=None)
    _require(isinstance(key, Ed25519PrivateKey), "signing key must be an Ed25519 private key")
    record_out.write_bytes(record_bytes)
    signature_out.write_bytes(key.sign(record_bytes))


def shutil_which(name: str) -> str | None:
    from shutil import which

    return which(name)


def run_verify(args: argparse.Namespace) -> int:
    """Host mode: verify provenance, then extract into an empty directory."""
    archive_bytes = _read_regular(args.archive)
    inspected = inspect_archive(archive_bytes, args.source_sha, args.target)
    record_bytes = _read_regular(args.provenance_record)
    verify_provenance(
        record_bytes,
        _read_regular(args.provenance_signature),
        args.provenance_public_key,
        expected={
            "repository": args.repository,
            "workflow_path": args.workflow_path,
            "workflow_ref": args.workflow_ref,
            "source_sha": args.source_sha,
            "target": args.target,
            "archive_sha256": inspected["archive_sha256"],
            "binary_sha256": inspected["binary_sha256"],
        },
    )
    _extract(inspected["members"], args.output_dir, inspected["binary_sha256"])
    print(
        f"verified {BINARY_NAME} sha256={inspected['binary_sha256']} "
        f"from release {args.source_sha} into {args.output_dir}"
    )
    return 0


def _extract(members: dict[str, bytes], output_dir: Path, binary_sha256: str) -> None:
    """Write validated members into an empty directory.

    Every member name was already proven to be a plain relative path with no
    traversal, and every file is created relative to one open directory
    descriptor with O_NOFOLLOW, so nothing swapped in after the check can
    redirect a write. The directory must not itself be a symlink and must be
    absent or empty.
    """
    if output_dir.exists() or output_dir.is_symlink():
        _require(not output_dir.is_symlink(), "output directory must not be a symlink")
        _require(output_dir.is_dir(), "output directory must be a directory")
    else:
        output_dir.mkdir(parents=True)
    descriptor = os.open(output_dir, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        _require(not os.listdir(descriptor), "output directory must be empty")
        for name, content in members.items():
            parent = os.path.dirname(name)
            _require(
                "/" not in parent, f"archive member {name!r} nests deeper than one directory"
            )
            if parent and parent not in os.listdir(descriptor):
                os.mkdir(parent, dir_fd=descriptor)
            mode = 0o755 if name == BINARY_NAME else 0o644
            handle = os.open(
                name,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                mode,
                dir_fd=descriptor,
            )
            with os.fdopen(handle, "wb") as stream:
                stream.write(content)
            # os.open applies the process umask, so set the mode explicitly.
            os.chmod(name, mode, dir_fd=descriptor)
        binary = os.open(BINARY_NAME, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=descriptor)
        with os.fdopen(binary, "rb") as stream:
            _require(
                sha256_bytes(stream.read()) == binary_sha256,
                "extracted binary digest changed during extraction",
            )
    finally:
        os.close(descriptor)


# --------------------------------------------------------------------------- #
# Packaging
# --------------------------------------------------------------------------- #
def _archive_member(name: str, content: bytes, mode: int) -> tuple[tarfile.TarInfo, io.BytesIO]:
    member = tarfile.TarInfo(name)
    member.size = len(content)
    member.mode = mode
    member.uid = 0
    member.gid = 0
    member.uname = "root"
    member.gname = "root"
    member.mtime = 0
    return member, io.BytesIO(content)


def build_archive(
    *,
    binary: Path,
    data_dir: Path,
    source_sha: str,
    target: str,
    toolchain: str,
    lockfile: Path,
) -> bytes:
    """Build a deterministic archive from the built binary and reviewed data."""
    _require(_FULL_SHA.fullmatch(source_sha) is not None, "source SHA must be a full commit SHA")
    _require(target == DEFAULT_TARGET, "unexpected build target")
    binary_bytes = _read_regular(binary)
    data_files = sorted(
        path
        for path in data_dir.iterdir()
        if path.is_file() and path.suffix == ".json" and path.name != "artifact.schema.json"
    )
    _require(bool(data_files), "no data files to package")
    entries: dict[str, bytes] = {}
    for path in data_files:
        entries[f"{DATA_PREFIX}{path.name}"] = _read_regular(path)
    policy_path = f"{DATA_PREFIX}market-policy.json"
    manifest_path = f"{DATA_PREFIX}market-policy.manifest.json"
    allowlist_path = f"{DATA_PREFIX}allowed-revisions.json"
    for required in (policy_path, manifest_path, allowlist_path):
        _require(required in entries, f"required data file {required!r} is absent")
    manifest = json.loads(entries[manifest_path])
    revisions = json.loads(entries[allowlist_path])
    metadata = {
        "version": 1,
        "release_sha": source_sha,
        "target": target,
        "libc_min": LIBC_MIN,
        "toolchain": toolchain,
        "lockfile_sha256": sha256_bytes(_read_regular(lockfile)),
        "workflow_path": WORKFLOW_PATH,
        "binary": {"path": BINARY_NAME, "sha256": sha256_bytes(binary_bytes)},
        "policy": {
            "source_sha": manifest["source_sha"],
            "policy_path": policy_path,
            "policy_sha256": sha256_bytes(entries[policy_path]),
            "manifest_path": manifest_path,
            "manifest_sha256": sha256_bytes(entries[manifest_path]),
        },
        "revision_allowlist": {
            "path": allowlist_path,
            "sha256": sha256_bytes(entries[allowlist_path]),
            "revisions": revisions,
        },
        "files": [
            {"path": name, "sha256": sha256_bytes(content)}
            for name, content in sorted(entries.items())
        ],
        "runtime_hosts": RUNTIME_HOSTS,
    }
    validate_metadata(metadata)
    metadata_bytes = json.dumps(metadata, indent=2, sort_keys=True).encode() + b"\n"
    sums = "".join(
        f"{sha256_bytes(content)}  {name}\n"
        for name, content in sorted({**entries, BINARY_NAME: binary_bytes, METADATA_NAME: metadata_bytes}.items())
    ).encode()
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
        for name, content in sorted({**entries, BINARY_NAME: binary_bytes, METADATA_NAME: metadata_bytes}.items()):
            member, stream = _archive_member(
                name, content, 0o755 if name == BINARY_NAME else 0o644
            )
            bundle.addfile(member, stream)
        member, stream = _archive_member(SUMS_NAME, sums, 0o644)
        bundle.addfile(member, stream)
    return archive.getvalue()


def run_package(args: argparse.Namespace) -> int:
    data = build_archive(
        binary=args.binary,
        data_dir=args.data_dir,
        source_sha=args.source_sha,
        target=args.target,
        toolchain=args.toolchain,
        lockfile=args.lockfile,
    )
    args.output.write_bytes(data)
    if args.metadata_out is not None:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            handle = archive.extractfile(METADATA_NAME)
            _require(handle is not None, "packaged archive is missing its metadata")
            args.metadata_out.write_bytes(handle.read())
    print(f"wrote {args.output} sha256={sha256_bytes(data)}")
    return 0


def run_glibc_check(args: argparse.Namespace) -> int:
    text = args.objdump.read_text() if args.objdump else sys.stdin.read()
    observed = check_glibc_max(text, args.maximum)
    print(f"glibc_max={observed}")
    return 0


# --------------------------------------------------------------------------- #
# Entry point
# --------------------------------------------------------------------------- #
def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.set_defaults(mode="verify")
    parser.add_argument("--archive", type=Path, help="release archive to verify")
    parser.add_argument("--source-sha", help="approved full release SHA")
    parser.add_argument("--target", default=DEFAULT_TARGET)
    parser.add_argument("--output-dir", type=Path, help="empty directory to extract into")

    parser.add_argument("--repository", default=REPOSITORY)
    parser.add_argument("--workflow-path", default=WORKFLOW_PATH)
    parser.add_argument("--workflow-ref", default="refs/heads/staging")

    parser.add_argument("--provenance-record", type=Path)
    parser.add_argument("--provenance-signature", type=Path)
    parser.add_argument("--provenance-public-key", type=Path)
    parser.add_argument("--attested-at", default="")

    parser.add_argument("--attest", action="store_true", help="workstation attestation mode")
    parser.add_argument("--binary", type=Path, help="built binary or loose attestation subject")
    parser.add_argument("--signing-key", type=Path, help="Ed25519 private key (workstation)")
    parser.add_argument("--record-out", type=Path)
    parser.add_argument("--signature-out", type=Path)

    parser.add_argument("--package", action="store_true", help="package mode (CI)")
    parser.add_argument("--data-dir", type=Path, default=Path("deploy/rust"))
    parser.add_argument("--lockfile", type=Path, default=Path("rs/Cargo.lock"))
    parser.add_argument("--toolchain")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--metadata-out", type=Path, help="also write the packaged metadata here")

    parser.add_argument("--glibc-check", action="store_true", help="read objdump -T output")
    parser.add_argument("--objdump", type=Path, help="file holding objdump -T output")
    parser.add_argument("--max", dest="maximum", default=LIBC_MIN)
    return parser


def dispatch(args: argparse.Namespace) -> int:
    if args.glibc_check:
        return run_glibc_check(args)
    _require(args.source_sha is not None, "--source-sha is required")
    _require(
        _FULL_SHA.fullmatch(args.source_sha) is not None,
        "--source-sha must be a full lowercase commit SHA",
    )
    if args.package:
        for required in ("binary", "toolchain", "output"):
            _require(getattr(args, required) is not None, f"--{required.replace('_', '-')} is required")
        return run_package(args)
    if args.attest:
        for required in ("archive", "binary", "signing_key", "record_out", "signature_out"):
            _require(getattr(args, required) is not None, f"--{required.replace('_', '-')} is required")
        return run_attest(args)
    _require(args.archive is not None, "--archive is required")
    for required in ("output_dir", "provenance_record", "provenance_signature", "provenance_public_key"):
        _require(
            getattr(args, required) is not None,
            f"--{required.replace('_', '-')} is required to verify an artifact",
        )
    return run_verify(args)


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return dispatch(args)
    except ArtifactError as exc:
        print(f"artifact verification failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
