import json
from pathlib import Path

import pytest
from scripts.rust_contract.compare import (
    ContractError,
    capture,
    compare_news_tie_groups,
)

ROOT = Path(__file__).resolve().parents[2]

# The one reviewed rule (decision 18). Nothing else may be configured.
APPROVED_RULE = {
    "rule": "news-published-at-tie-multiset",
    "request_path": "/api/news",
    "json_path": "/body",
    "sort_key": "published_at",
}


def envelope(raw):
    return capture(200, {"content-type": "application/json"}, raw)


A = b'{"id":1,"published_at":"2026-10-15"}'
B = b'{"id":2,"published_at":"2026-10-15"}'
C = b'{"id":3,"published_at":"2026-10-14"}'


def test_reordered_news_tie_passes():
    assert (
        compare_news_tie_groups(
            envelope(b"[" + A + b"," + B + b"," + C + b"]"),
            envelope(b"[" + B + b"," + A + b"," + C + b"]"),
            request_path="/api/news",
        )
        == []
    )


@pytest.mark.parametrize(
    "items",
    [
        A + b"," + C,
        A + b"," + A + b"," + C,
        C + b"," + A + b"," + B,
        A.replace(b"1", b"9", 1) + b"," + B + b"," + C,
        A.replace(b'"id":1', b'"id": 1') + b"," + B + b"," + C,
    ],
)
def test_missing_changed_or_misordered_news_fails(items):
    assert compare_news_tie_groups(
        envelope(b"[" + A + b"," + B + b"," + C + b"]"),
        envelope(b"[" + items + b"]"),
        request_path="/api/news",
    )


def test_other_paths_remain_byte_exact():
    assert compare_news_tie_groups(
        envelope(b"[" + A + b"," + B + b"]"),
        envelope(b"[" + B + b"," + A + b"]"),
        request_path="/api/orderbook",
    )


def test_ascending_published_at_order_fails():
    # Decision 18: the keys must match and be in descending published-time
    # order. An identical ascending pair used to compare equal.
    ascending = b"[" + C + b"," + A + b"," + B + b"]"
    assert compare_news_tie_groups(
        envelope(ascending), envelope(ascending), request_path="/api/news"
    ) == ["/body/published_at:order"]


def test_equal_descending_news_pair_stays_equal():
    descending = b"[" + A + b"," + B + b"," + C + b"]"
    assert (
        compare_news_tie_groups(
            envelope(descending), envelope(descending), request_path="/api/news"
        )
        == []
    )


def test_checked_in_rule_file_holds_only_the_approved_news_rule():
    rules = json.loads((ROOT / "tests/rust_contract/news-tie-paths.json").read_text())
    assert rules == [APPROVED_RULE]


@pytest.mark.parametrize(
    "document",
    [
        [],
        [APPROVED_RULE, {**APPROVED_RULE, "request_path": "/api/orderbook"}],
        [{**APPROVED_RULE, "request_path": "/api/orderbook"}],
        [{**APPROVED_RULE, "json_path": "/body/items"}],
        [{**APPROVED_RULE, "sort_key": "created_at"}],
        [{**APPROVED_RULE, "rule": "any-tie-multiset"}],
        {**APPROVED_RULE},
    ],
)
def test_unreviewed_rule_file_fails_closed(tmp_path, document):
    path = tmp_path / "news-tie-paths.json"
    path.write_text(json.dumps(document))
    with pytest.raises(ContractError):
        compare_news_tie_groups(
            envelope(b"[" + A + b"," + B + b"]"),
            envelope(b"[" + A + b"," + B + b"]"),
            request_path="/api/news",
            rules_path=path,
        )


def test_unreadable_rule_file_fails_closed(tmp_path):
    path = tmp_path / "news-tie-paths.json"
    path.write_text("{not json")
    with pytest.raises(ContractError):
        compare_news_tie_groups(
            envelope(b"[" + A + b"]"),
            envelope(b"[" + A + b"]"),
            request_path="/api/news",
            rules_path=path,
        )
