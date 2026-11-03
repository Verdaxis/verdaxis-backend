"""The release metadata keeps the watchlist producer activation across deploys.

`scripts/deploy.sh` rewrites `.runtime-release.env` on every deploy. The two
producer keys are written only by the deploy itself: carried forward byte for
byte unless the operator supplies a value, never with a changed activation
instant, and dropped below the producer checkpoint, where the schema has no
producer. Each case stops the deploy at the first command after the metadata
is published (the fake preflight), so nothing after it runs.
"""

import json
import subprocess
from pathlib import Path

from tests.unit.test_deploy_health import (
    MIGRATION_TARGET,
    _approved_deploy_environment,
    _deployment_checkout,
    _git,
)

ROOT = Path(__file__).resolve().parents[2]
PRODUCER = "p89_20261022_watchlist_producer"
FREEZE = "p89_20261022_trade_freeze"
INSTANT = "2026-11-01T10:00:00+00:00"
PRODUCER_KEYS = ("WATCHLIST_EVENT_PRODUCER", "WATCHLIST_PRODUCER_ACTIVATED_AT")


def _release_with_producer_checkpoints(tmp_path: Path) -> tuple[Path, str]:
    """A deploy checkout whose next release allowlists the producer steps."""
    source, checkout, _old_sha = _deployment_checkout(tmp_path)
    policy = source / "deploy/migration-checkpoints.tsv"
    policy.write_text(
        policy.read_text()
        + f"{MIGRATION_TARGET}\t{PRODUCER}\n"
        + f"{PRODUCER}\t{PRODUCER}\n"
        + f"{PRODUCER}\t{FREEZE}\n"
        + f"{FREEZE}\t{FREEZE}\n"
    )
    _git(source, "add", "deploy/migration-checkpoints.tsv")
    _git(source, "commit", "-qm", "producer checkpoints")
    _git(source, "push", "-q", "origin", "staging")
    return checkout, _git(source, "rev-parse", "HEAD").stdout.strip()


def _record(checkout: Path, *lines: str) -> None:
    (checkout / ".runtime-release.env").write_text(
        "ENVIRONMENT=staging\nRELEASE_SHA=" + "b" * 40 + "\n"
        f"MIGRATION_REVISION={PRODUCER}\n" + "".join(f"{line}\n" for line in lines)
    )


def _deploy(
    tmp_path: Path,
    checkout: Path,
    release_sha: str,
    *,
    current: str = PRODUCER,
    target: str = FREEZE,
    supplied: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    environment = _approved_deploy_environment(
        tmp_path / "commands.log", release_sha, fail_phase="preflight"
    )
    environment.update(
        {
            "MIGRATION_EXPECTED_CURRENT_REVISION": current,
            "MIGRATION_TARGET_REVISION": target,
        }
    )
    for key in PRODUCER_KEYS:
        environment.pop(key, None)
    environment.update(supplied or {})
    return subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def _published(checkout: Path) -> str:
    return (checkout / ".runtime-release.env").read_text()


def _expected(release_sha: str, target: str, *lines: str) -> str:
    return (
        f"ENVIRONMENT=staging\nRELEASE_SHA={release_sha}\nMIGRATION_REVISION={target}\n"
        + "".join(f"{line}\n" for line in lines)
    )


def test_producer_checkpoint_list_matches_the_allowed_revisions():
    """deploy.sh names the producer checkpoint and every later revision."""
    allowed = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    revisions = allowed["revisions"] if isinstance(allowed, dict) else allowed
    later = revisions[revisions.index(PRODUCER):]
    source = (ROOT / "scripts/deploy.sh").read_text()
    start = source.index("revision_has_watchlist_producer() {")
    case_line = next(
        line for line in source[start:].splitlines() if ") return 0 ;;" in line
    )
    listed = case_line.strip().split(")")[0].split("|")
    assert listed == later
