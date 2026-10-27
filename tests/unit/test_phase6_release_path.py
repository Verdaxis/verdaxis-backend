"""The stream release follows each reviewed migration checkpoint in order."""
import json
from pathlib import Path

from alembic.config import Config
from alembic.script import ScriptDirectory

ROOT = Path(__file__).resolve().parents[2]


def test_stream_release_path_has_no_skipped_checkpoint():
    chain = ["prc_20261026_changes", "p3_20261020_auth_tokens", "sse_20261021_replay_floor"]
    script = ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))
    assert script.get_heads() == [chain[-1]]
    for parent, child in zip(chain, chain[1:]):
        assert script.get_revision(child).down_revision == parent
    rows = {tuple(line.split("\t")) for line in (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines() if line and not line.startswith("#")}
    assert (chain[0], chain[1]) in rows and (chain[1], chain[2]) in rows
    for source, target in rows:
        if target == chain[2]:
            assert source in (chain[1], chain[2])
        if target == chain[1]:
            assert source in (chain[0], chain[1])
    allowed = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert set(chain) <= set(allowed)


def test_stream_checkpoint_is_an_explicit_production_pin():
    for path in ("rs/read/src/runtime/config.rs", "scripts/prepare_rust_runtime.py"):
        assert '"sse_20261021_replay_floor"' in (ROOT / path).read_text()
