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
PARENT = "prc_20261018_cache"
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
    # The cache checkpoint stays a reviewed pause for a one-change rollback.
    assert f"{PRODUCTION}\t{PARENT}" in rows
    assert f"{PARENT}\t{PARENT}" in rows

    module = _checkpoint_module()
    policy = module.parse_checkpoint_policy(text)
    _validate(module, policy, PARENT, REVISION)
    _validate(module, policy, REVISION, REVISION)
    # One reviewed step at a time: production does not jump past the cache checkpoint.
    with pytest.raises(module.MigrationCheckpointError):
        _validate(module, policy, PRODUCTION, REVISION)


def test_phase5_runtime_accepts_both_read_checkpoints():
    allowlist = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert allowlist == ["kycg_20261014_staging", PRODUCTION, PARENT, REVISION]
