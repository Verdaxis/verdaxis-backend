"""Offline tests for the Rust artifact verifier.

Every fixture archive is built in a temporary directory; no network, database,
Docker, Cargo or child process is used, and nothing from an archive is ever
executed. The fixture "binary" is a shell script that would leave a marker file
if anything ran it, so the tests can prove the verifier only ever writes it.
"""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    NoEncryption,
    PrivateFormat,
    PublicFormat,
)

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "verify_rust_artifact", ROOT / "scripts" / "verify_rust_artifact.py"
)
artifact = importlib.util.module_from_spec(spec)
spec.loader.exec_module(artifact)

SOURCE_SHA = "a" * 40
TARGET = artifact.DEFAULT_TARGET
TOOLCHAIN = "1.95.0"
# A binary that would prove it had been executed by creating this marker.
MARKER = "EXECUTED"
BINARY_SCRIPT = f"#!/bin/sh\ntouch \"$(dirname \"$0\")/{MARKER}\"\n"


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


class Fixture:
    """A packaged archive plus the material needed to verify it."""

    def __init__(self, root: Path):
        self.root = root
        self.binary = root / "verdaxis-read"
        self.binary.write_bytes(BINARY_SCRIPT.encode())
        data_dir = root / "data"
        data_dir.mkdir()
        policy = {"source_sha": "b" * 40, "products": []}
        (data_dir / "market-policy.json").write_text(json.dumps(policy) + "\n")
        (data_dir / "market-policy.manifest.json").write_text(
            json.dumps(
                {
                    "source_sha": policy["source_sha"],
                    "policy": "market-policy.json",
                    "policy_sha256": sha256((data_dir / "market-policy.json").read_bytes()),
                }
            )
            + "\n"
        )
        (data_dir / "allowed-revisions.json").write_text(json.dumps(["kycg_20261014_staging"]) + "\n")
        (data_dir / "mcp-catalog.json").write_text("{}\n")
        lockfile = root / "Cargo.lock"
        lockfile.write_text("# fixture lockfile\n")
        self.archive_bytes = artifact.build_archive(
            binary=self.binary,
            data_dir=data_dir,
            source_sha=SOURCE_SHA,
            target=TARGET,
            toolchain=TOOLCHAIN,
            lockfile=lockfile,
        )
        self.archive = root / "release.tar"
        self.archive.write_bytes(self.archive_bytes)
        self.key = Ed25519PrivateKey.generate()
        self.key_path = root / "provenance.pem"
        self.key_path.write_bytes(
            self.key.private_bytes(Encoding.PEM, PrivateFormat.PKCS8, NoEncryption())
        )
        self.public_path = root / "provenance.pub.pem"
        self.public_path.write_bytes(
            self.key.public_key().public_bytes(Encoding.PEM, PublicFormat.SubjectPublicKeyInfo)
        )
        self.record = root / "record.json"
        self.signature = root / "record.sig"
        self.output = root / "out"
        self.write_record()

    def inspected(self):
        return artifact.inspect_archive(self.archive_bytes, SOURCE_SHA, TARGET)

    def write_record(self, **overrides) -> None:
        inspected = self.inspected()
        record = {
            "version": 1,
            "repository": artifact.REPOSITORY,
            "workflow_path": artifact.WORKFLOW_PATH,
            "workflow_ref": "refs/heads/staging",
            "source_sha": SOURCE_SHA,
            "target": TARGET,
            "archive_sha256": inspected["archive_sha256"],
            "binary_sha256": inspected["binary_sha256"],
            "attested_subjects": [artifact.BINARY_NAME, "archive"],
            "gh_attestation_verified": True,
            "attested_at": "2026-10-17T00:00:00Z",
        }
        record.update(overrides)
        record_bytes = json.dumps(record, indent=2, sort_keys=True).encode() + b"\n"
        self.record.write_bytes(record_bytes)
        self.signature.write_bytes(self.key.sign(record_bytes))

    def verify(self, **overrides):
        arguments = dict(
            archive=self.archive,
            source_sha=SOURCE_SHA,
            target=TARGET,
            output_dir=self.output,
            provenance_record=self.record,
            provenance_signature=self.signature,
            provenance_public_key=self.public_path,
            repository=artifact.REPOSITORY,
            workflow_path=artifact.WORKFLOW_PATH,
            workflow_ref="refs/heads/staging",
        )
        arguments.update(overrides)
        return artifact.main(
            [
                "--archive", str(arguments["archive"]),
                "--source-sha", arguments["source_sha"],
                "--target", arguments["target"],
                "--output-dir", str(arguments["output_dir"]),
                "--provenance-record", str(arguments["provenance_record"]),
                "--provenance-signature", str(arguments["provenance_signature"]),
                "--provenance-public-key", str(arguments["provenance_public_key"]),
                "--repository", arguments["repository"],
                "--workflow-path", arguments["workflow_path"],
                "--workflow-ref", arguments["workflow_ref"],
            ]
        )

    def assert_refused(self) -> None:
        """Refusal must happen before extraction: no output, no execution."""
        assert not self.output.exists(), "refusal must not create the output directory"
        assert not (self.root / MARKER).exists(), "nothing may execute the fixture binary"


def member(name, content, *, mode=0o644, kind=tarfile.REGTYPE, linkname=""):
    """A `(TarInfo, content-bytes-or-None)` pair for `build_tar`."""
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = 0
    info.type = kind
    info.linkname = linkname
    info.size = 0 if content is None else len(content)
    return info, content


def _read_members(archive: tarfile.TarFile):
    """`(TarInfo, content-bytes-or-None)` for each member.

    `extractfile` returns None for a non-regular member, so a directory, link or
    device round-trips as `(entry, None)` instead of raising.
    """
    members = []
    for entry in archive.getmembers():
        handle = archive.extractfile(entry) if entry.isfile() else None
        members.append((entry, None if handle is None else handle.read()))
    return members


def build_tar(members) -> bytes:
    """Build an uncompressed tar from `(TarInfo, content-bytes-or-None)` pairs."""
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
        for info, content in members:
            bundle.addfile(info, None if content is None else io.BytesIO(content))
    return out.getvalue()


def swap_members(archive_bytes: bytes, replacements) -> bytes:
    """Rebuild an archive, replacing or adding named members by hand."""
    with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:") as archive:
        members = _read_members(archive)
    rebuilt = []
    for entry, content in members:
        replacement = replacements.pop(entry.name, None)
        rebuilt.append((entry, content) if replacement is None else replacement)
    rebuilt.extend(replacements.values())
    return build_tar(rebuilt)


def append_member(archive_bytes: bytes, info, content) -> bytes:
    """Rebuild an archive with one extra member, preserving any duplicate name."""
    with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:") as archive:
        members = _read_members(archive)
    members.append((info, content))
    return build_tar(members)


class ArtifactVerificationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.fixture = Fixture(Path(self.directory.name))

    # -- the happy path ---------------------------------------------------- #
    def test_round_trip_extracts_without_executing(self):
        self.assertEqual(self.fixture.verify(), 0)
        extracted = self.fixture.output / artifact.BINARY_NAME
        self.assertTrue(extracted.is_file())
        self.assertEqual(extracted.read_bytes(), BINARY_SCRIPT.encode())
        self.assertEqual(extracted.stat().st_mode & 0o777, 0o755)
        self.assertTrue((self.fixture.output / artifact.SUMS_NAME).is_file())
        self.assertFalse((self.fixture.root / MARKER).exists(), "the binary must never run")
        self.assertEqual(
            sha256(extracted.read_bytes()), self.fixture.inspected()["binary_sha256"]
        )

    def test_output_directory_must_be_empty(self):
        self.fixture.output.mkdir()
        (self.fixture.output / "stale").write_text("x")
        self.assertEqual(self.fixture.verify(), 1)
        self.assertTrue((self.fixture.output / "stale").is_file())

    def test_symlinked_output_directory_is_refused(self):
        real = self.fixture.root / "real-out"
        real.mkdir()
        self.fixture.output.symlink_to(real)
        self.assertEqual(self.fixture.verify(), 1)
        self.assertFalse(any(real.iterdir()), "nothing may be written through a symlink")

    # -- provenance bindings ----------------------------------------------- #
    def test_wrong_digest_is_refused(self):
        tampered = swap_members(
            self.fixture.archive_bytes,
            {artifact.BINARY_NAME: member(artifact.BINARY_NAME, BINARY_SCRIPT.encode() + b"#x", mode=0o755)},
        )
        self.fixture.archive.write_bytes(tampered)
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_wrong_source_sha_is_refused(self):
        self.assertEqual(self.fixture.verify(source_sha="c" * 40), 1)
        self.fixture.assert_refused()

    def test_wrong_target_is_refused(self):
        self.assertEqual(self.fixture.verify(target="aarch64-unknown-linux-gnu"), 1)
        self.fixture.assert_refused()

    def test_wrong_workflow_is_refused(self):
        self.fixture.write_record(workflow_path=".github/workflows/other.yml")
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_wrong_workflow_ref_is_refused(self):
        self.fixture.write_record(workflow_ref="refs/heads/prod")
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_wrong_repository_is_refused(self):
        # The record must name the repository that signed the attestation, not
        # just any repository with a valid signature.
        self.fixture.write_record(repository="someone/else")
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_missing_attestation_verification_is_refused(self):
        self.fixture.write_record(gh_attestation_verified=False)
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_record_archive_digest_mismatch_is_refused(self):
        # A record that is internally valid but names another archive digest must
        # not verify, even though the archive itself is well formed.
        self.fixture.write_record(archive_sha256="0" * 64)
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_record_binary_digest_mismatch_is_refused(self):
        self.fixture.write_record(binary_sha256="0" * 64)
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_record_attesting_only_one_subject_is_refused(self):
        self.fixture.write_record(attested_subjects=[artifact.BINARY_NAME])
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_unsigned_record_is_refused(self):
        self.fixture.signature.write_bytes(b"not a signature")
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_record_signed_by_another_key_is_refused(self):
        other = Ed25519PrivateKey.generate()
        record_bytes = self.fixture.record.read_bytes()
        self.fixture.signature.write_bytes(other.sign(record_bytes))
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_attestation_requires_gh_on_the_workstation(self):
        with mock.patch.object(artifact, "shutil_which", return_value=None):
            with self.assertRaises(artifact.ArtifactError):
                artifact.run_attest(
                    mock.Mock(
                        archive=self.fixture.archive,
                        binary=self.fixture.binary,
                        source_sha=SOURCE_SHA,
                        target=TARGET,
                    )
                )

    # -- archive structure -------------------------------------------------- #
    def test_absent_binary_is_refused(self):
        with tarfile.open(fileobj=io.BytesIO(self.fixture.archive_bytes), mode="r:") as archive:
            members = [entry for entry in archive.getmembers() if entry.name != artifact.BINARY_NAME]
            rebuilt = build_tar([(entry, archive.extractfile(entry).read()) for entry in members])
        self.fixture.archive.write_bytes(rebuilt)
        self.assertEqual(self.fixture.verify(), 1)
        self.fixture.assert_refused()

    def test_duplicate_member_is_refused(self):
        duplicate = member(artifact.BINARY_NAME, b"other")
        rebuilt = append_member(self.fixture.archive_bytes, *duplicate)
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)
        self.fixture.assert_refused()

    def test_traversing_member_is_refused(self):
        # tarfile keeps the raw header name on read, so a leading "./" arrives as
        # a "." segment, not as a normalized "data/x". Every one of these has a
        # "."/".."/empty segment, an absolute path, or a backslash.
        for name in (
            "../escape",
            "data/../../escape",
            "data/./x",
            "data//x",
            "./data/x",
            "/etc/passwd",
            "data\\x",
        ):
            rebuilt = swap_members(self.fixture.archive_bytes, {name: member(name, b"x")})
            with self.assertRaises(artifact.ArtifactError, msg=name):
                artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_symlink_member_is_refused(self):
        link = member("data/link.json", None, kind=tarfile.SYMTYPE, linkname="../../etc/passwd")
        rebuilt = swap_members(self.fixture.archive_bytes, {"data/link.json": link})
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_hardlink_member_is_refused(self):
        link = member("data/hard.json", None, kind=tarfile.LNKTYPE, linkname=artifact.BINARY_NAME)
        rebuilt = swap_members(self.fixture.archive_bytes, {"data/hard.json": link})
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_directory_and_device_members_are_refused(self):
        for kind in (tarfile.DIRTYPE, tarfile.CHRTYPE, tarfile.BLKTYPE, tarfile.FIFOTYPE):
            info = tarfile.TarInfo("data/odd")
            info.type = kind
            info.size = 0
            rebuilt = swap_members(self.fixture.archive_bytes, {"data/odd": (info, None)})
            with self.assertRaises(artifact.ArtifactError, msg=str(kind)):
                artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_unexpected_member_is_refused(self):
        rebuilt = swap_members(self.fixture.archive_bytes, {"data/extra.json": member("data/extra.json", b"{}")})
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_oversized_member_is_refused(self):
        # Pick a bound between the largest real member and the injected one, so
        # only the injected member trips the per-member limit.
        payload = b"0" * 8192
        rebuilt = swap_members(
            self.fixture.archive_bytes, {"data/big.json": member("data/big.json", payload)}
        )
        with mock.patch.object(artifact, "MAX_MEMBER_BYTES", 4096):
            with self.assertRaises(artifact.ArtifactError):
                artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_oversized_archive_is_refused(self):
        with mock.patch.object(artifact, "MAX_ARCHIVE_BYTES", 16):
            with self.assertRaises(artifact.ArtifactError):
                artifact.inspect_archive(self.fixture.archive_bytes, SOURCE_SHA, TARGET)

    def test_compressed_archive_is_refused(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as bundle:
            info, content = member(artifact.BINARY_NAME, b"x")
            bundle.addfile(info, io.BytesIO(content))
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(buffer.getvalue(), SOURCE_SHA, TARGET)

    def test_too_many_members_is_refused(self):
        with mock.patch.object(artifact, "MAX_MEMBERS", 2):
            with self.assertRaises(artifact.ArtifactError):
                artifact.inspect_archive(self.fixture.archive_bytes, SOURCE_SHA, TARGET)

    def test_sums_mismatch_is_refused(self):
        with tarfile.open(fileobj=io.BytesIO(self.fixture.archive_bytes), mode="r:") as archive:
            contents = {entry.name: archive.extractfile(entry).read() for entry in archive.getmembers()}
        bad = "".join(
            f"{'0' * 64}  {name}\n" for name in contents if name != artifact.SUMS_NAME
        ).encode()
        rebuilt = swap_members(
            self.fixture.archive_bytes, {artifact.SUMS_NAME: member(artifact.SUMS_NAME, bad)}
        )
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    def test_policy_digest_mismatch_is_refused(self):
        rebuilt = swap_members(
            self.fixture.archive_bytes,
            {"data/market-policy.json": member("data/market-policy.json", b'{"source_sha": "' + b"b" * 40 + b'"}')},
        )
        with self.assertRaises(artifact.ArtifactError):
            artifact.inspect_archive(rebuilt, SOURCE_SHA, TARGET)

    # -- metadata rules ----------------------------------------------------- #
    def test_metadata_rejects_unknown_and_missing_fields(self):
        document = json.loads(self.fixture_member(artifact.METADATA_NAME))
        artifact.validate_metadata(document)
        for mutation in (
            lambda value: value.pop("libc_min"),
            lambda value: value.update(extra=1),
            lambda value: value.__setitem__("target", "aarch64-unknown-linux-gnu"),
            lambda value: value.__setitem__("libc_min", "2.39"),
            lambda value: value.__setitem__("version", 2),
            lambda value: value.__setitem__("version", True),
            lambda value: value.__setitem__("release_sha", "a" * 39),
            lambda value: value.__setitem__("release_sha", None),
            lambda value: value.__setitem__("toolchain", 195),
            lambda value: value.__setitem__("lockfile_sha256", ["0" * 64]),
            lambda value: value["binary"].__setitem__("path", "other"),
            lambda value: value["binary"].__setitem__("sha256", None),
            lambda value: value["policy"].__setitem__("source_sha", "b" * 39),
            lambda value: value["revision_allowlist"].__setitem__("revisions", []),
            lambda value: value["revision_allowlist"].__setitem__("revisions", ["Bad-Revision"]),
            lambda value: value["revision_allowlist"].__setitem__("revisions", [1]),
            lambda value: value["files"].append({"path": "data/y.json", "sha256": "0" * 64, "extra": 1}),
            lambda value: value["files"].append({"path": "y.json", "sha256": "0" * 64}),
            lambda value: value["files"].append(dict(value["files"][0])),
            lambda value: value.__setitem__("files", []),
            lambda value: value["runtime_hosts"]["staging"].__setitem__("glibc", "nope"),
            lambda value: value["runtime_hosts"]["staging"].__setitem__("arch", "aarch64"),
            lambda value: value["runtime_hosts"].pop("production"),
        ):
            changed = json.loads(json.dumps(document))
            mutation(changed)
            with self.assertRaises(artifact.ArtifactError):
                artifact.validate_metadata(changed)

    def fixture_member(self, name: str) -> bytes:
        with tarfile.open(fileobj=io.BytesIO(self.fixture.archive_bytes), mode="r:") as archive:
            return archive.extractfile(name).read()

    def test_schema_matches_the_metadata_validator(self):
        schema = json.loads((ROOT / "deploy" / "rust" / "artifact.schema.json").read_text())
        self.assertEqual(set(schema["required"]), set(schema["properties"]))
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(schema["properties"]["version"]["const"], 1)
        self.assertEqual(schema["properties"]["target"]["const"], artifact.DEFAULT_TARGET)
        self.assertEqual(schema["properties"]["libc_min"]["const"], artifact.LIBC_MIN)
        self.assertEqual(
            schema["properties"]["workflow_path"]["const"], artifact.WORKFLOW_PATH
        )
        self.assertEqual(
            schema["properties"]["binary"]["properties"]["path"]["const"], artifact.BINARY_NAME
        )
        document = json.loads(self.fixture_member(artifact.METADATA_NAME))
        self.assertEqual(set(document), set(schema["required"]))
        self.assertEqual(document["libc_min"], schema["properties"]["libc_min"]["const"])
        self.assertEqual(document["workflow_path"], schema["properties"]["workflow_path"]["const"])

    # -- glibc floor -------------------------------------------------------- #
    def test_glibc_floor_accepts_the_host_and_refuses_newer(self):
        accepted = "0000 DF *UND* 0000 GLIBC_2.2.5 memcpy\n0000 DF *UND* 0000 GLIBC_2.35 strlcpy\n"
        self.assertEqual(artifact.check_glibc_max(accepted, "2.35"), "2.35")
        newer = accepted + "0000 DF *UND* 0000 GLIBC_2.38 statx\n"
        with self.assertRaises(artifact.ArtifactError):
            artifact.check_glibc_max(newer, "2.35")
        with self.assertRaises(artifact.ArtifactError):
            artifact.check_glibc_max("no dynamic symbols here\n", "2.35")

    def test_glibc_check_cli_reads_objdump_output(self):
        path = self.fixture.root / "objdump.txt"
        path.write_text("0000000000000000  DF *UND* 0000 GLIBC_2.17 clock_gettime\n")
        self.assertEqual(
            artifact.main(
                ["--glibc-check", "--objdump", str(path), "--max", "2.35", "--source-sha", SOURCE_SHA]
            ),
            0,
        )

    # -- packaging ---------------------------------------------------------- #
    def test_package_writes_a_verifiable_archive(self):
        out = self.fixture.root / "packaged.tar"
        code = artifact.main(
            [
                "--package",
                "--binary", str(self.fixture.binary),
                "--data-dir", str(self.fixture.root / "data"),
                "--lockfile", str(self.fixture.root / "Cargo.lock"),
                "--toolchain", TOOLCHAIN,
                "--source-sha", SOURCE_SHA,
                "--target", TARGET,
                "--output", str(out),
            ]
        )
        self.assertEqual(code, 0)
        inspected = artifact.inspect_archive(out.read_bytes(), SOURCE_SHA, TARGET)
        self.assertEqual(inspected["metadata"]["toolchain"], TOOLCHAIN)
        self.assertEqual(inspected["metadata"]["libc_min"], artifact.LIBC_MIN)
        self.assertIsInstance(inspected["metadata"]["runtime_hosts"], dict)

    def test_package_requires_the_reviewed_data_files(self):
        empty = self.fixture.root / "empty"
        empty.mkdir()
        with self.assertRaises(artifact.ArtifactError):
            artifact.build_archive(
                binary=self.fixture.binary,
                data_dir=empty,
                source_sha=SOURCE_SHA,
                target=TARGET,
                toolchain=TOOLCHAIN,
                lockfile=self.fixture.root / "Cargo.lock",
            )


if __name__ == "__main__":
    unittest.main()
