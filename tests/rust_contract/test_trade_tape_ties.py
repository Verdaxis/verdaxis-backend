"""Decision-68-adjacent PM rule: `/api/trade-tape` tie groups compare as a set.

Both implementations copy the same tie-unstable `ORDER BY confirmed_at DESC`, so
the order of items inside one `confirmed_at` tie group is source
nondeterminism. Only the item bytes within a tie group may permute; the sequence
of `confirmed_at` keys, their descending order, the item sets, the frame and
every other byte stay exact. A real ordering difference across tie groups still
fails.
"""

import json

import pytest
from scripts.rust_contract.compare import capture, recording_differences

# Four trades: two tie at 12:00, one at 11:00, one at 10:00.
T1 = b'{"id":"be1bd3ef","confirmed_at":"2026-10-15T12:00:00Z","price":"600.00"}'
T2 = b'{"id":"20fdbacc","confirmed_at":"2026-10-15T12:00:00Z","price":"601.00"}'
T3 = b'{"id":"2459bf40","confirmed_at":"2026-10-15T11:00:00Z","price":"602.00"}'
T4 = b'{"id":"bf84470c","confirmed_at":"2026-10-15T10:00:00Z","price":"603.00"}'


def entry(items, path="/api/trade-tape"):
    raw = b'{"items":[' + items + b'],"total":4,"market_hours":true}'
    return {
        "id": "R06-trade-tape",
        "request": {"path": path},
        "exchange": capture(200, {"content-type": "application/json"}, raw),
    }


def test_reordered_items_inside_one_tie_group_pass():
    # T1 and T2 share 12:00: swapping them is the tie nondeterminism.
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    right = entry(T2 + b"," + T1 + b"," + T3 + b"," + T4)
    assert not recording_differences([left], [right])


def test_a_real_ordering_difference_across_tie_groups_fails():
    # 11:00 before 12:00 is a different tie-key sequence: strict, so it fails.
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    right = entry(T3 + b"," + T1 + b"," + T2 + b"," + T4)
    problems = recording_differences([left], [right])
    assert problems
    assert any("sequence" in problem or "order" in problem for problem in problems)


def test_ascending_tie_keys_fail():
    # The corpus contract keeps confirmed_at descending.
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    right = entry(T4 + b"," + T3 + b"," + T2 + b"," + T1)
    assert recording_differences([left], [right])


@pytest.mark.parametrize(
    "items",
    [
        # A changed item value.
        T1.replace(b"600.00", b"600.01") + b"," + T2 + b"," + T3 + b"," + T4,
        # A missing item.
        T1 + b"," + T2 + b"," + T4,
        # An extra item.
        T1 + b"," + T2 + b"," + T3 + b"," + T4 + b"," + T4,
        # A respelled item (whitespace) is still a different byte sequence.
        T1.replace(b'"price":', b'"price": ') + b"," + T2 + b"," + T3 + b"," + T4,
        # A renamed field inside a tie group.
        T1.replace(b'"price"', b'"value"') + b"," + T2 + b"," + T3 + b"," + T4,
    ],
)
def test_changed_missing_extra_or_respelt_item_fails(items):
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    assert recording_differences([left], [entry(items)])


def test_a_total_change_fails():
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    raw = b'{"items":[' + T1 + b"," + T2 + b"," + T3 + b"," + T4 + b'],"total":5,"market_hours":true}'
    right = {
        "id": "R06-trade-tape",
        "request": {"path": "/api/trade-tape"},
        "exchange": capture(200, {"content-type": "application/json"}, raw),
    }
    assert recording_differences([left], [right])


def test_a_market_hours_change_fails():
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    raw = b'{"items":[' + T1 + b"," + T2 + b"," + T3 + b"," + T4 + b'],"total":4,"market_hours":false}'
    right = {
        "id": "R06-trade-tape",
        "request": {"path": "/api/trade-tape"},
        "exchange": capture(200, {"content-type": "application/json"}, raw),
    }
    assert recording_differences([left], [right])


def test_the_rule_does_not_apply_to_other_paths():
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4, path="/api/orderbook")
    right = entry(T2 + b"," + T1 + b"," + T3 + b"," + T4, path="/api/orderbook")
    assert recording_differences([left], [right])


def test_an_unreviewed_rule_file_is_refused(tmp_path):
    from scripts.rust_contract.compare import ContractError, compare_trade_tape_tie_groups

    widened = tmp_path / "widened.json"
    widened.write_text(json.dumps([{"rule": "anything", "request_path": "/api/trade-tape"}]))
    left = entry(T1 + b"," + T2 + b"," + T3 + b"," + T4)
    right = entry(T2 + b"," + T1 + b"," + T3 + b"," + T4)
    with pytest.raises(ContractError, match="unreviewed"):
        compare_trade_tape_tie_groups(
            left["exchange"], right["exchange"],
            request_path="/api/trade-tape", rules_path=widened,
        )
