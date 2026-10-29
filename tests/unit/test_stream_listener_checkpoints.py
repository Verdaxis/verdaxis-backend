"""The deploy enables the stream listener at the stream checkpoint and every later one."""

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STREAM_CHECKPOINT = "sse_20261021_replay_floor"


def test_stream_listener_covers_every_checkpoint_from_the_stream_release_on():
    # The reviewed allowlist lists the checkpoints in migration order.
    order = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    expected = order[order.index(STREAM_CHECKPOINT):]
    source = (ROOT / "scripts/deploy.sh").read_text()
    match = re.search(
        r'case "\$MIGRATION_TARGET_REVISION" in\n\s*([^)\n]+)\) RUST_STREAM_LISTENER="1" ;;',
        source,
    )
    assert match is not None
    assert match.group(1).split("|") == expected
