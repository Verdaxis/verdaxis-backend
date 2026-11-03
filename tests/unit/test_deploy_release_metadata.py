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
    _deploy_environment,
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


def test_release_metadata_carries_the_producer_keys_forward(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    recorded = (
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )
    _record(checkout, *recorded)

    result = _deploy(tmp_path, checkout, release_sha)

    # The fake preflight stops the deploy right after the publication.
    assert result.returncode != 0
    assert "+ write immutable release metadata" in result.stdout
    assert _published(checkout) == _expected(release_sha, FREEZE, *recorded)


def test_release_metadata_records_a_supplied_activation(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    _record(checkout)

    result = _deploy(
        tmp_path,
        checkout,
        release_sha,
        supplied={
            "WATCHLIST_EVENT_PRODUCER": "rust-market-v1",
            "WATCHLIST_PRODUCER_ACTIVATED_AT": INSTANT,
        },
    )

    assert "+ write immutable release metadata" in result.stdout
    assert _published(checkout) == _expected(
        release_sha,
        FREEZE,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )


def test_release_metadata_refuses_a_changed_activation_instant(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    _record(
        checkout,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )
    before = _published(checkout)

    result = _deploy(
        tmp_path,
        checkout,
        release_sha,
        supplied={"WATCHLIST_PRODUCER_ACTIVATED_AT": "2026-11-02T10:00:00+00:00"},
    )

    assert result.returncode != 0
    assert "activation instant never changes" in result.stderr
    assert _published(checkout) == before
    assert "+ write immutable release metadata" not in result.stdout


def test_release_metadata_allows_pause_and_reactivation_with_the_same_instant(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    _record(
        checkout,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )

    paused = _deploy(
        tmp_path,
        checkout,
        release_sha,
        supplied={"WATCHLIST_EVENT_PRODUCER": "legacy-python"},
    )
    assert "+ write immutable release metadata" in paused.stdout
    assert _published(checkout) == _expected(
        release_sha,
        FREEZE,
        "WATCHLIST_EVENT_PRODUCER=legacy-python",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )

    # The failed deploy left its blocked state; a new attempt recovers it.
    (tmp_path / "runtime-deploy-state/staging.state").unlink()
    resumed = _deploy(
        tmp_path,
        checkout,
        release_sha,
        current=FREEZE,
        supplied={
            "WATCHLIST_EVENT_PRODUCER": "rust-market-v1",
            "WATCHLIST_PRODUCER_ACTIVATED_AT": INSTANT,
        },
    )
    assert "+ write immutable release metadata" in resumed.stdout
    assert _published(checkout) == _expected(
        release_sha,
        FREEZE,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )


def test_release_metadata_refuses_a_producer_without_an_instant(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    for supplied, reason in (
        ({"WATCHLIST_EVENT_PRODUCER": "rust-market-v1"}, "requires WATCHLIST_PRODUCER_ACTIVATED_AT"),
        ({"WATCHLIST_PRODUCER_ACTIVATED_AT": INSTANT}, "requires WATCHLIST_EVENT_PRODUCER"),
        (
            {"WATCHLIST_EVENT_PRODUCER": "python", "WATCHLIST_PRODUCER_ACTIVATED_AT": INSTANT},
            "legacy-python or rust-market-v1",
        ),
        (
            {"WATCHLIST_EVENT_PRODUCER": "rust-market-v1", "WATCHLIST_PRODUCER_ACTIVATED_AT": "2026-11-01T10:00:00"},
            "RFC 3339",
        ),
    ):
        _record(checkout)
        before = _published(checkout)
        result = _deploy(tmp_path, checkout, release_sha, supplied=supplied)
        assert result.returncode != 0, supplied
        assert reason in result.stderr, (supplied, result.stderr)
        assert _published(checkout) == before


def test_release_metadata_drops_producer_keys_below_the_producer_checkpoint(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    _record(
        checkout,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )

    result = _deploy(
        tmp_path, checkout, release_sha, current=MIGRATION_TARGET, target=MIGRATION_TARGET
    )
    assert "+ write immutable release metadata" in result.stdout
    assert _published(checkout) == _expected(release_sha, MIGRATION_TARGET)

    (tmp_path / "runtime-deploy-state/staging.state").unlink()
    refused = _deploy(
        tmp_path,
        checkout,
        release_sha,
        current=MIGRATION_TARGET,
        target=MIGRATION_TARGET,
        supplied={
            "WATCHLIST_EVENT_PRODUCER": "rust-market-v1",
            "WATCHLIST_PRODUCER_ACTIVATED_AT": INSTANT,
        },
    )
    assert refused.returncode != 0
    assert "require the producer checkpoint" in refused.stderr


def test_release_metadata_refuses_an_instant_that_is_not_a_real_utc_time(tmp_path):
    """The recorded instant never changes later, so only a real UTC time
    (a calendar date and clock time, offset Z or +00:00) is ever recorded."""
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    for instant in (
        "2026-02-30T10:00:00+00:00",
        "2026-11-01T25:00:00Z",
        "2026-11-01T10:61:00Z",
        "2026-11-01T10:00:61+00:00",
        "2026-11-01T18:00:00+08:00",
        "2026-11-01T10:00:00-00:00",
        "2026-11-01T10:00:00+24:00",
    ):
        _record(checkout)
        before = _published(checkout)
        result = _deploy(
            tmp_path,
            checkout,
            release_sha,
            supplied={
                "WATCHLIST_EVENT_PRODUCER": "rust-market-v1",
                "WATCHLIST_PRODUCER_ACTIVATED_AT": instant,
            },
        )
        assert result.returncode != 0, instant
        assert "real UTC time" in result.stderr, (instant, result.stderr)
        assert _published(checkout) == before
        assert "+ write immutable release metadata" not in result.stdout


def test_release_metadata_records_a_utc_instant_with_fraction_and_z(tmp_path):
    checkout, release_sha = _release_with_producer_checkpoints(tmp_path)
    _record(checkout)
    instant = "2028-02-29T23:59:59.123456Z"
    result = _deploy(
        tmp_path,
        checkout,
        release_sha,
        supplied={
            "WATCHLIST_EVENT_PRODUCER": "rust-market-v1",
            "WATCHLIST_PRODUCER_ACTIVATED_AT": instant,
        },
    )
    assert "+ write immutable release metadata" in result.stdout, result.stderr
    assert _published(checkout) == _expected(
        release_sha,
        FREEZE,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={instant}",
    )


def test_dry_run_prints_the_producer_keys(tmp_path):
    checkout, _release_sha = _release_with_producer_checkpoints(tmp_path)
    _record(
        checkout,
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1",
        f"WATCHLIST_PRODUCER_ACTIVATED_AT={INSTANT}",
    )
    environment = _deploy_environment(tmp_path / "commands.log")
    environment["WATCHLIST_EVENT_PRODUCER"] = "legacy-python"
    environment.pop("WATCHLIST_PRODUCER_ACTIVATED_AT", None)

    result = subprocess.run(
        ["bash", "scripts/deploy.sh", "--dry-run"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert (
        "  WATCHLIST_EVENT_PRODUCER recorded=rust-market-v1 supplied=legacy-python"
        in result.stdout
    )
    assert (
        f"  WATCHLIST_PRODUCER_ACTIVATED_AT recorded={INSTANT} supplied=<absent>"
        in result.stdout
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
