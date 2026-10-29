"""Deployment checkpoints for the read lookup index migration."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path

import pytest
from alembic.config import Config
from alembic.script import ScriptDirectory

ROOT = Path(__file__).resolve().parents[2]
SOURCE_SHA = "a" * 40
PRODUCTION = "p0_20261016_union"
CACHE = "prc_20261018_cache"
CHANGES = "prc_20261026_changes"
AUTH = "p3_20261020_auth_tokens"
PARENT = "sse_20261021_replay_floor"
REVISION = "p5_20261020_read_indexes"


def _checkpoint_module():
    path = ROOT / "scripts/apply_migration_checkpoint.py"
    spec = importlib.util.spec_from_file_location("apply_migration_checkpoint", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _validate(module, policy, current, target):
    module.validate_checkpoint_request(
        policy=policy,
        source_sha=SOURCE_SHA,
        approved_source_sha=SOURCE_SHA,
        expected_current=current,
        target=target,
        current_heads=(current,),
        script_directory=ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini"))),
    )


def test_phase5_checkpoint_cases():
    text = (ROOT / "deploy/migration-checkpoints.tsv").read_text()
    rows = text.splitlines()
    assert f"{PARENT}\t{REVISION}" in rows
    assert f"{REVISION}\t{REVISION}" in rows
    for current, target in ((PRODUCTION, CACHE), (CACHE, CHANGES), (CHANGES, AUTH), (AUTH, PARENT)):
        assert f"{current}\t{target}" in rows
        assert f"{target}\t{target}" in rows

    module = _checkpoint_module()
    policy = module.parse_checkpoint_policy(text)
    _validate(module, policy, PARENT, REVISION)
    _validate(module, policy, REVISION, REVISION)
    # Every predecessor stops at the stream checkpoint before Phase 5.
    for current in (PRODUCTION, CACHE, CHANGES, AUTH):
        with pytest.raises(module.MigrationCheckpointError):
            _validate(module, policy, current, REVISION)


def test_phase5_runtime_accepts_both_read_checkpoints():
    allowlist = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert allowlist == ["kycg_20261014_staging", PRODUCTION, CACHE, CHANGES, AUTH, PARENT, REVISION]
