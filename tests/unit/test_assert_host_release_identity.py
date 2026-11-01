"""The Phase 0 host check accepts exactly the release metadata deploy.sh writes.

That is the three identity keys, alone or followed by exactly the two watchlist
producer keys with valid values. Any other key, order or value is refused.
"""
import pytest

from scripts.phase0.assert_host import release_identity_matches

SHA = "a" * 40
IDENTITY = f"ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"
REVISION = "p89_20261022_trade_freeze"
BASE = IDENTITY + f"MIGRATION_REVISION={REVISION}\n"
PRODUCER = "WATCHLIST_EVENT_PRODUCER=rust-market-v1\n"
INSTANT = "WATCHLIST_PRODUCER_ACTIVATED_AT=2026-11-01T10:00:00+00:00\n"


def test_release_identity_accepts_the_three_key_file():
    assert release_identity_matches(BASE, IDENTITY, REVISION)


@pytest.mark.parametrize("producer", ["rust-market-v1", "legacy-python"])
@pytest.mark.parametrize(
    "instant", ["2026-11-01T10:00:00+00:00", "2026-11-01T18:00:00.123456+08:00", "2026-11-01T10:00:00Z"]
)
def test_release_identity_accepts_the_producer_keys(producer, instant):
    text = BASE + f"WATCHLIST_EVENT_PRODUCER={producer}\nWATCHLIST_PRODUCER_ACTIVATED_AT={instant}\n"
    assert release_identity_matches(text, IDENTITY, REVISION)


@pytest.mark.parametrize(
    "text",
    [
        BASE + PRODUCER,
        BASE + INSTANT,
        BASE + INSTANT + PRODUCER,
        BASE + PRODUCER + INSTANT + "EXTRA=1\n",
        BASE + "EXTRA=1\n",
        BASE + "WATCHLIST_EVENT_PRODUCER=python\n" + INSTANT,
        BASE + PRODUCER + "WATCHLIST_PRODUCER_ACTIVATED_AT=2026-11-01T10:00:00\n",
        BASE + PRODUCER + INSTANT.rstrip("\n"),
        IDENTITY + "MIGRATION_REVISION=p5_20261020_read_indexes\n" + PRODUCER + INSTANT,
        PRODUCER + INSTANT + BASE,
    ],
)
def test_release_identity_refuses_any_other_file(text):
    assert not release_identity_matches(text, IDENTITY, REVISION)
