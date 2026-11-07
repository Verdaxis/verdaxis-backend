"""The deploy enables the stream listener at the stream checkpoint and every later one."""

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STREAM_CHECKPOINT = "sse_20261021_replay_floor"


def test_stream_listener_covers_every_checkpoint_from_the_stream_release_on():
    # M admits only the adopted baseline; retained stream checkpoints still
    # need the listener when replaying the reviewed deployment history.
    order = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert order == ["sqlx_1"]
    expected = [STREAM_CHECKPOINT, "p5_20261020_read_indexes",
                "p89_20261022_watchlist_producer", "p89_20261022_trade_freeze",
                "ob_20261031_offboarding", *order]
    source = (ROOT / "scripts/deploy.sh").read_text()
    match = re.search(
        r'case "\$MIGRATION_TARGET_REVISION" in\n\s*([^)\n]+)\) RUST_STREAM_LISTENER="1" ;;',
        source,
    )
    assert match is not None
    assert match.group(1).split("|") == expected
