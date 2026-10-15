"""Task 4b fixture inventory and fail-closed writer boundary."""

import pytest
from scripts.rust_contract import seed_corpus as corpus

EXPECTED_COUNTS = {
    "ports": 3,
    "port_intelligence": 3,
    "news": 6,
    "market_orders": 46,
    "market_trades": 6,
    "small_decimal_orders": 4,
    "signals": 2,
    "ingestion_runs": 1,
    "legacy_trades": 2,
    "legacy_orders": 1,
}


def test_every_fixture_family_has_exact_count():
    assert corpus.EXPECTED_COUNTS == EXPECTED_COUNTS
    assert set(corpus.CORPUS_FIXTURES) <= set(corpus.FAMILY_CHECKS)


@pytest.mark.parametrize(
    "change",
    [
        {"environment": "staging"},
        {"is_superuser": True},
        {"connected_database": "verdaxis"},
        {"expected_database": "verdaxis"},
        {"host": "localhost"},
        {"host": "194.233.68.86"},
        {"connected_user": "runtime_app"},
    ],
)
def test_fixture_target_refuses_unsafe_authority(change):
    values = dict(
        environment="test",
        expected_database="corpus_test",
        connected_database="corpus_test",
        connected_user="rust_owner",
        is_superuser=False,
        host="127.0.0.1",
    )
    values.update(change)
    with pytest.raises(corpus.CorpusSeedError):
        corpus.attest_target(**values)


def test_fixture_target_accepts_only_local_writer():
    corpus.attest_target(
        environment="test",
        expected_database="corpus_test",
        connected_database="corpus_test",
        connected_user="rust_owner",
        is_superuser=False,
        host="127.0.0.1",
    )


def test_missing_or_wrong_family_count_fails_closed():
    corpus.validate_counts(EXPECTED_COUNTS)
    for name in EXPECTED_COUNTS:
        missing = dict(EXPECTED_COUNTS)
        missing.pop(name)
        with pytest.raises(corpus.CorpusSeedError):
            corpus.validate_counts(missing)
        wrong = {**EXPECTED_COUNTS, name: EXPECTED_COUNTS[name] + 1}
        with pytest.raises(corpus.CorpusSeedError):
            corpus.validate_counts(wrong)
