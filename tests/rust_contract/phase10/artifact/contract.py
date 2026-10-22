"""Disposable artifact corpus. Keys generated here are TEST MATERIAL ONLY."""
from __future__ import annotations

import copy
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    NoEncryption,
    PrivateFormat,
    PublicFormat,
)

ROOT = Path(__file__).resolve().parents[4]
SPEC = importlib.util.spec_from_file_location("artifact", ROOT / "scripts/verify_rust_artifact.py")
artifact = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(artifact)
SHA = "a" * 40
DIGEST = "0" * 64
VERIFIER = ROOT / "deploy/scripts/verify-artifact.sh"


def digest(data):
    return hashlib.sha256(data).hexdigest()


class ArtifactContract(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="artifact-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.key = Ed25519PrivateKey.generate()
        self.private = self.root / "TEST-ONLY.pem"
        self.private.write_bytes(self.key.private_bytes(Encoding.PEM, PrivateFormat.PKCS8, NoEncryption()))
        self.public = self.root / "TEST-ONLY.pub.pem"
        self.public.write_bytes(self.key.public_key().public_bytes(Encoding.PEM, PublicFormat.SubjectPublicKeyInfo))
        self.archive = self.root / "release.tar"
        self.record = self.root / "record.json"
        self.signature = self.root / "record.sig"
        self.output = self.root / "out"
        self.marker = self.root / "CANDIDATE_EXECUTED"
        self.binary = self.root / "verdaxis-read"
        self.binary.write_text(f"#!/bin/sh\ntouch '{self.marker}'\n")
        self.data = self.root / "data"
        self.data.mkdir()
        (self.data / "market-policy.json").write_text('{}\n')
        (self.data / "market-policy.manifest.json").write_text(json.dumps({
            "source_sha": "b" * 40,
            "policy_sha256": digest((self.data / "market-policy.json").read_bytes()),
        }))
        (self.data / "allowed-revisions.json").write_text('["sqlx_1", "sqlx_20261022"]\n')
        (self.data / "mcp-catalog.json").write_text('{}\n')
        self.lock = self.root / "Cargo.lock"
        self.lock.write_text('# test lockfile\n')
        self.archive.write_bytes(artifact.build_archive(
            binary=self.binary, data_dir=self.data, source_sha=SHA,
            target=artifact.DEFAULT_TARGET, toolchain="1.95.0", lockfile=self.lock,
        ))
        with tarfile.open(self.archive, 'r:') as stream:
            self.members = [(copy.copy(m), stream.extractfile(m).read()) for m in stream]
        self.metadata = json.loads(dict((m.name, b) for m, b in self.members)[artifact.METADATA_NAME])
        self.sign()

    def sign(self, **overrides):
        record = {
            "version": 1, "repository": artifact.REPOSITORY,
            "workflow_path": artifact.WORKFLOW_PATH, "workflow_ref": "refs/heads/staging",
            "source_sha": SHA, "target": artifact.DEFAULT_TARGET,
            "archive_sha256": digest(self.archive.read_bytes()),
            "binary_sha256": self.metadata['binary']['sha256'],
            "attested_subjects": ['verdaxis-read', 'archive'],
            "gh_attestation_verified": True, "attested_at": "2026-10-22T00:00:00Z",
        }
        record.update(overrides)
        self.record.write_bytes((json.dumps(record, sort_keys=True) + '\n').encode())
        self.signature.write_bytes(self.key.sign(self.record.read_bytes()))

    def rebuild(self, changes=None, extra=(), remove=(), metadata=None, refresh_sums=False):
        entries = [(copy.copy(m), b) for m, b in self.members if m.name not in remove]
        changes = dict(changes or {})
        if metadata is not None:
            changes[artifact.METADATA_NAME] = json.dumps(metadata).encode()
        entries = [(m, changes.get(m.name, b)) for m, b in entries]
        if refresh_sums:
            sums = ''.join(f'{digest(b)}  {m.name}\n' for m, b in entries if m.name != 'SHA256SUMS').encode()
            entries = [(m, sums if m.name == 'SHA256SUMS' else b) for m, b in entries]
        entries.extend(extra)
        with tarfile.open(self.archive, 'w', format=tarfile.USTAR_FORMAT) as stream:
            for member, content in entries:
                if member.isfile():
                    member.size = len(content)
                stream.addfile(member, io.BytesIO(content) if member.isfile() else None)
        self.sign()

    def invoke(self, *extra, env=None):
        return subprocess.run([
            'bash', str(VERIFIER), '--archive', str(self.archive), '--source-sha', SHA,
            '--output-dir', str(self.output), '--provenance-record', str(self.record),
            '--provenance-signature', str(self.signature), '--provenance-public-key', str(self.public),
            *extra,
        ], cwd=ROOT, capture_output=True, text=True, timeout=30, env=env)

    def refused(self, *extra, env=None):
        result = self.invoke(*extra, env=env)
        self.assertNotEqual(result.returncode, 0, 'verifier accepted invalid artifact')
        self.assertFalse(self.marker.exists(), 'candidate executed')
        if self.output.is_dir() and not self.output.is_symlink():
            self.assertEqual(list(self.output.iterdir()), [], 'failure published files')

    def test_00_wrong_repository_workflow_path_ref_source_ref_and_target_are_refused(self):
        for field, value in {
            'repository': 'other/repo', 'workflow_path': '.github/workflows/other.yml',
            'workflow_ref': 'refs/heads/other', 'source_sha': 'c' * 40, 'target': 'aarch64-unknown-linux-gnu',
        }.items():
            with self.subTest(field=field):
                self.sign(**{field: value})
                self.refused()

    def test_valid_fixture_extracts_only_allowlisted_members(self):
        self.output.mkdir()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        actual = {str(p.relative_to(self.output)) for p in self.output.rglob('*') if p.is_file()}
        self.assertEqual(actual, {m.name for m, _ in self.members})
        for member, content in self.members:
            self.assertEqual((self.output / member.name).read_bytes(), content)
        self.assertEqual((self.output / 'verdaxis-read').stat().st_mode & 0o777, 0o755)
        self.assertFalse(self.marker.exists())

    def test_tampered_archive_bit_flip_is_refused(self):
        data = bytearray(self.archive.read_bytes())
        data[600] ^= 1
        self.archive.write_bytes(data)
        self.refused()

    def test_record_and_signature_variants_are_refused(self):
        for overrides in ({'version': 2}, {'extra': True}, {'gh_attestation_verified': False},
                          {'attested_subjects': ['archive']}, {'archive_sha256': DIGEST},
                          {'binary_sha256': DIGEST}):
            with self.subTest(overrides=overrides):
                self.sign(**overrides)
                self.refused()
        self.sign()
        self.record.write_bytes(self.record.read_bytes() + b' ')
        self.refused()
        self.sign()
        self.signature.write_bytes(bytes(64))
        self.refused()
        self.signature.write_bytes(b'')
        self.refused()

    def test_wrong_public_key_and_wrong_source_sha_are_refused(self):
        other = self.root / 'other.pub.pem'
        other.write_bytes(Ed25519PrivateKey.generate().public_key().public_bytes(Encoding.PEM, PublicFormat.SubjectPublicKeyInfo))
        self.refused('--provenance-public-key', str(other))
        self.refused('--source-sha', 'c' * 40)

    def test_extra_member_and_path_traversal_are_refused(self):
        for name in ('extra', '../escape', '/absolute', 'data/../escape', 'data//bad', 'data/./bad',
                     'data/a\\b', 'data/new\nline', ' data/bad', 'data/trailing '):
            with self.subTest(name=name):
                member = tarfile.TarInfo(name)
                self.rebuild(extra=[(member, b'bad')])
                self.refused()

    def test_duplicate_nonregular_link_and_device_members_are_refused(self):
        self.rebuild(extra=[self.members[0]])
        self.refused()
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.DIRTYPE, tarfile.CHRTYPE, tarfile.BLKTYPE, tarfile.FIFOTYPE):
            with self.subTest(kind=kind):
                member = tarfile.TarInfo('data/other')
                member.type = kind
                member.linkname = '../outside'
                self.rebuild(extra=[(member, b'')])
                self.refused()

    def test_oversize_member_and_member_count_are_refused(self):
        extra = [(tarfile.TarInfo(f'data/extra{i}'), b'') for i in range(65)]
        self.rebuild(extra=extra)
        self.refused()
        # A complete sparse-on-disk archive stays cheap while its header declares an oversized member.
        member = tarfile.TarInfo('verdaxis-read')
        member.size = artifact.MAX_MEMBER_BYTES + 1
        with self.archive.open('wb') as stream:
            stream.write(member.tobuf(format=tarfile.USTAR_FORMAT))
            stream.seek(512 + ((member.size + 511) // 512) * 512)
            stream.write(bytes(1024))
        self.sign()
        self.refused()
        with self.archive.open('r+b') as stream:
            stream.truncate(artifact.MAX_ARCHIVE_BYTES + 1)
        self.sign()
        self.refused()

    def test_metadata_policy_allowlist_and_each_member_digest_are_bound(self):
        mutations = [
            lambda m: m.update(extra=True), lambda m: m.pop('toolchain'),
            lambda m: m.update(version=2), lambda m: m.update(libc_min='2.39'),
            lambda m: m.update(toolchain='bad'), lambda m: m.update(lockfile_sha256='bad'),
            lambda m: m.update(workflow_path='other'), lambda m: m.update(target='other'),
            lambda m: m.update(release_sha='c' * 40),
            lambda m: m['binary'].update(sha256=DIGEST),
            lambda m: m['policy'].update(source_sha='c' * 40),
            lambda m: m['policy'].update(policy_sha256=DIGEST),
            lambda m: m['policy'].update(manifest_sha256=DIGEST),
            lambda m: m['revision_allowlist'].update(sha256=DIGEST),
            lambda m: m['revision_allowlist'].update(revisions=['other']),
            lambda m: m['files'][0].update(sha256=DIGEST),
            lambda m: m['files'].append(m['files'][0]),
            lambda m: m['runtime_hosts']['staging'].update(arch='other'),
        ]
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                metadata = copy.deepcopy(self.metadata)
                mutate(metadata)
                self.rebuild(metadata=metadata, refresh_sums=True)
                self.refused()
        for member, _ in self.members:
            with self.subTest(member=member.name):
                self.rebuild(changes={member.name: b'tampered'})
                self.refused()
        self.rebuild(changes={'SHA256SUMS': b''}, refresh_sums=False)
        self.refused()

    def test_missing_file_and_compressed_archive_are_refused(self):
        for member, _ in self.members:
            with self.subTest(member=member.name):
                self.rebuild(remove=[member.name])
                self.refused()
        import gzip
        self.archive.write_bytes(gzip.compress(self.archive.read_bytes()))
        self.sign()
        self.refused()
        self.archive.unlink()
        self.refused()

    def test_symlink_output_and_extraction_substitution_are_refused(self):
        target = self.root / 'untouched'
        target.mkdir()
        sentinel = target / 'sentinel'
        sentinel.write_text('keep')
        self.output.symlink_to(target, target_is_directory=True)
        self.refused()
        self.assertEqual(sentinel.read_text(), 'keep')
        self.assertEqual(list(target.iterdir()), [sentinel])
        self.output.unlink()
        parent = self.root / 'link'
        parent.symlink_to(target, target_is_directory=True)
        self.output = parent / 'out'
        self.refused()
        self.output = self.root / 'out'
        # Substitute the extraction tree after GNU tar returns. The verifier must recheck it.
        tools = self.root / 'tools'
        tools.mkdir()
        import shutil
        real_tar = shutil.which('tar')
        wrapper = tools / 'tar'
        wrapper.write_text(f'''#!/bin/bash
{real_tar} "$@" || exit $?
for arg in "$@"; do
    if [[ "$arg" == --extract ]]; then
        rm -f data/allowed-revisions.json
        ln -s '{sentinel}' data/allowed-revisions.json
    fi
done
''')
        wrapper.chmod(0o755)
        env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'])
        self.refused(env=env)
        self.assertEqual(sentinel.read_text(), 'keep')

    def test_failure_leaves_the_output_directory_empty(self):
        self.output.mkdir()
        self.sign(workflow_ref='refs/heads/other')
        self.refused()
        self.assertEqual(list(self.output.iterdir()), [])

    def test_artifact_verifier_rejects_tampering(self):
        self.rebuild(changes={'verdaxis-read': b'changed'})
        self.refused()

    def test_packaged_attested_switch_artifact_passes_without_candidate_execution(self):
        tools = self.root / 'attestor-tools'
        tools.mkdir()
        gh = tools / 'gh'
        gh.write_text('#!/bin/sh\n[ "$1 $2" = "attestation verify" ] || exit 1\n')
        gh.chmod(0o755)
        env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'])
        result = subprocess.run([
            os.sys.executable, str(ROOT / 'scripts/verify_rust_artifact.py'), '--attest',
            '--archive', str(self.archive), '--binary', str(self.binary), '--source-sha', SHA,
            '--signing-key', str(self.private), '--record-out', str(self.record),
            '--signature-out', str(self.signature), '--attested-at', '2026-10-22T00:00:00Z',
        ], env=env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads((self.output / 'data/allowed-revisions.json').read_text()), ['sqlx_1', 'sqlx_20261022'])
        self.assertFalse(self.marker.exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
