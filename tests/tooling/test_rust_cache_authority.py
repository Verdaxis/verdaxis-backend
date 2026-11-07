"""Offline proofs for the cache authority owner script.

The PostgreSQL binding is proved in tests/postgres/test_public_read_cache_failover.py;
these tests cover the record contract shared with Rust and the owner's ordering.
"""
import asyncio
import importlib.util
import json
import os
import re
import stat
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
VECTORS = ROOT / "rs/read/tests/data/cache_authority_records.json"
RUST_AUTHORITY = ROOT / "rs/read/src/runtime/cache_authority.rs"


def _load():
    spec = importlib.util.spec_from_file_location(
        "rust_cache_authority", ROOT / "scripts/rust_cache_authority.py"
    )
    module = importlib.util.module_from_spec(spec)
    # dataclasses resolve string annotations through the module registry.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


owner = _load()


def _cases():
    return json.loads(VECTORS.read_text())["cases"]


def _text(case):
    return case["raw"] if "raw" in case else json.dumps(case["record"])


@pytest.mark.parametrize("case", _cases(), ids=lambda case: case["name"])
def test_record_vectors_match_the_rust_reader(case):
    if not case["accept"]:
        with pytest.raises(owner.AuthorityError):
            owner.parse_record(_text(case))
        return
    record = owner.parse_record(_text(case))
    expected = case["authority"]
    if expected is None:
        assert record.authority is None
    else:
        assert record.authority == owner.Authority(
            expected["database_epoch"], expected["primary_identity"], expected["revision"]
        )


def test_rendered_records_round_trip_and_stay_small():
    authority = owner.Authority(owner.new_identity(), owner.new_identity(), 7)
    for record in (owner.Record(7, authority), owner.Record(8, None)):
        text = owner.render_record(record)
        assert owner.parse_record(text) == record
        assert len(text.encode()) < owner.MAX_RECORD_BYTES // 4


def test_new_identities_are_random_lowercase_hex_that_rust_accepts():
    first, second = owner.new_identity(), owner.new_identity()
    assert first != second
    assert re.fullmatch(r"[0-9a-f]{64}", first)


def test_lease_and_drain_follow_the_rust_lease():
    source = RUST_AUTHORITY.read_text()
    match = re.search(r"AUTHORITY_LEASE: Duration = Duration::from_secs\((\d+)\)", source)
    assert match is not None
    lease = int(match.group(1))
    assert owner.LEASE_SECONDS == lease
    assert owner.FENCE_DRAIN_SECONDS >= 2 * lease


def test_publication_is_private_atomic_and_leaves_no_temporary_file(tmp_path):
    record = owner.Record(1, owner.Authority(owner.new_identity(), owner.new_identity(), 1))
    target = owner.publish_record(tmp_path, record)
    assert stat.S_IMODE(target.stat().st_mode) == 0o600
    assert owner.read_record(tmp_path) == record
    assert [path.name for path in tmp_path.iterdir()] == [owner.RECORD_NAME]
    owner.publish_record(tmp_path, owner.Record(2, None))
    assert owner.read_record(tmp_path).authority is None


def test_absent_record_is_none_and_unsafe_records_are_refused(tmp_path):
    assert owner.read_record(tmp_path) is None
    real = tmp_path / "real.json"
    real.write_text(owner.render_record(owner.Record(1, None)))
    (tmp_path / owner.RECORD_NAME).symlink_to(real)
    with pytest.raises(owner.AuthorityError):
        owner.read_record(tmp_path)
    (tmp_path / owner.RECORD_NAME).unlink()
    (tmp_path / owner.RECORD_NAME).write_text(" " * (owner.MAX_RECORD_BYTES + 1))
    with pytest.raises(owner.AuthorityError):
        owner.read_record(tmp_path)


def test_publication_refuses_a_symlinked_runtime_directory(tmp_path):
    real = tmp_path / "real"
    real.mkdir()
    link = tmp_path / "link"
    link.symlink_to(real)
    with pytest.raises(owner.AuthorityError):
        owner.publish_record(link, owner.Record(1, None))


class FakeStore:
    """A database stand-in that logs the order of owner steps."""

    def __init__(self, log, runtime_dir, *, revision=0, in_recovery=False, fail_bind=False):
        self.log = log
        self.runtime_dir = runtime_dir
        self.row = owner.IncarnationRow(
            owner.new_identity(), owner.new_identity(), revision, True, in_recovery
        )
        self.fail_bind = fail_bind
        self.bound = None

    async def read(self):
        return self.row

    async def bind(self, authority):
        record = owner.read_record(self.runtime_dir)
        self.log.append(("bind", record.authority is None, authority.revision))
        if self.fail_bind:
            raise owner.AuthorityError("bind failed")
        self.bound = authority


def _run(coroutine):
    return asyncio.run(coroutine)


def _sleeper(log, runtime_dir):
    """A drain wait that records whether the record was revoked while waiting."""

    async def sleep(seconds):
        record = owner.read_record(runtime_dir)
        log.append(("sleep", seconds, record.authority is None))

    return sleep


def test_activation_revokes_drains_binds_then_publishes(tmp_path):
    log = []
    store = FakeStore(log, tmp_path, revision=4)
    owner.publish_record(
        tmp_path, owner.Record(6, owner.Authority(owner.new_identity(), owner.new_identity(), 6))
    )
    authority = _run(owner.activate(tmp_path, store, sleep=_sleeper(log, tmp_path)))
    # Revoked at 7 (above both the row and the old record), waited a full drain
    # while revoked, bound while still revoked, and only then published at 8.
    assert log == [
        ("sleep", owner.FENCE_DRAIN_SECONDS, True),
        ("bind", True, 8),
    ]
    assert authority.revision == 8
    assert store.bound == authority
    published = owner.read_record(tmp_path)
    assert published == owner.Record(8, authority)
    assert authority.database_epoch != store.row.database_epoch
    assert authority.primary_identity != store.row.primary_identity


def test_failed_binding_leaves_the_record_revoked(tmp_path):
    log = []
    store = FakeStore(log, tmp_path, fail_bind=True)
    with pytest.raises(owner.AuthorityError):
        _run(owner.activate(tmp_path, store, sleep=_sleeper(log, tmp_path)))
    assert owner.read_record(tmp_path).authority is None


def test_failed_publication_after_binding_leaves_the_record_revoked(tmp_path, monkeypatch):
    log = []
    store = FakeStore(log, tmp_path)
    real_publish = owner.publish_record

    def publish(runtime_dir, record):
        if record.authority is not None:
            raise OSError("disk full")
        return real_publish(runtime_dir, record)

    monkeypatch.setattr(owner, "publish_record", publish)
    with pytest.raises(OSError):
        _run(owner.activate(tmp_path, store, sleep=_sleeper(log, tmp_path)))
    assert owner.read_record(tmp_path).authority is None
    assert store.bound is not None


def test_a_replica_is_refused_before_anything_changes(tmp_path):
    log = []
    store = FakeStore(log, tmp_path, in_recovery=True)
    with pytest.raises(owner.AuthorityError):
        _run(owner.activate(tmp_path, store, sleep=_sleeper(log, tmp_path)))
    assert log == []
    assert owner.read_record(tmp_path) is None


def test_revisions_only_increase_across_repeated_activations(tmp_path):
    log = []
    store = FakeStore(log, tmp_path)
    first = _run(owner.activate(tmp_path, store, sleep=_sleeper(log, tmp_path)))
    store.row = owner.IncarnationRow(
        first.database_epoch, first.primary_identity, first.revision, True, False
    )
    second = _run(owner.activate(tmp_path, store, sleep=_sleeper(log, tmp_path)))
    assert second.revision > first.revision
    assert second.database_epoch != first.database_epoch
    assert second.primary_identity != first.primary_identity


def test_revocation_never_lowers_the_revision(tmp_path):
    owner.publish_record(tmp_path, owner.Record(40, None))
    revoked = _run(owner.revoke_and_drain(tmp_path, floor=3, sleep=_sleeper([], tmp_path)))
    assert revoked == 41
    assert owner.read_record(tmp_path) == owner.Record(41, None)


def test_status_never_prints_a_full_identity(tmp_path, capsys):
    authority = owner.Authority(owner.new_identity(), owner.new_identity(), 3)
    owner.publish_record(tmp_path, owner.Record(3, authority))
    assert owner.main(["status", "--runtime-dir", str(tmp_path)]) == 0
    printed = capsys.readouterr().out
    assert authority.database_epoch not in printed
    assert authority.primary_identity not in printed
    assert json.loads(printed)["state"] == "active"


def test_ordinary_deploy_scripts_neither_create_nor_rotate_authority():
    """Rotation is an explicit recovery step; a deploy must never do it."""
    for name in ("scripts/deploy.sh", "scripts/prepare_rust_runtime.py"):
        assert "rust_cache_authority" not in (ROOT / name).read_text()
    assert os.access(ROOT / "scripts/rust_cache_authority.py", os.R_OK)
