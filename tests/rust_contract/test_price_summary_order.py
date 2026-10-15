import json

import pytest
from scripts.rust_contract.compare import capture, recording_differences

A = b'{"window":"SPOT","price":"12.00"}'
B = b'{"window":"2031-01","price":"13.00"}'


def entry(items, path="/api/prices", suffix=b',"generated_at":"fixed"}'):
    raw = b'{"summaries":[' + items + b"]" + suffix
    return {
        "id": "prices",
        "request": {"path": path},
        "exchange": capture(200, {"content-type": "application/json"}, raw),
    }


def test_reordered_summary_passes():
    assert not recording_differences([entry(A + b"," + B)], [entry(B + b"," + A)])


@pytest.mark.parametrize(
    "items",
    [
        A,
        A + b"," + B + b"," + B,
        A.replace(b"12.00", b"12.01") + b"," + B,
        A.replace(b'"price":', b'"price": ') + b"," + B,
    ],
)
def test_changed_missing_extra_or_respelt_item_fails(items):
    assert recording_differences([entry(A + b"," + B)], [entry(items)])


@pytest.mark.parametrize(
    "path", ["/api/prices/reference", "/api/trade-tape", "/api/news", "/api/orderbook"]
)
def test_rule_does_not_apply_elsewhere(path):
    assert recording_differences(
        [entry(A + b"," + B, path)], [entry(B + b"," + A, path)]
    )


def test_surrounding_bytes_stay_exact():
    assert recording_differences(
        [entry(A + b"," + B)],
        [entry(B + b"," + A, suffix=b', "generated_at":"fixed"}')],
    )


@pytest.mark.parametrize(
    "change",
    [
        {"request_path": "/api/prices/reference"},
        {"json_path": "/body/prices"},
        {"rule": "all-arrays"},
    ],
)
def test_widened_rule_configuration_refused(tmp_path, change):
    from scripts.rust_contract.compare import (
        APPROVED_PRICE_SUMMARY_RULES,
        ContractError,
        compare_price_summary_multiset,
    )

    path = tmp_path / "rules.json"
    path.write_text(json.dumps([{**APPROVED_PRICE_SUMMARY_RULES[0], **change}]))
    with pytest.raises(ContractError):
        compare_price_summary_multiset(
            entry(A)["exchange"],
            entry(A)["exchange"],
            request_path="/api/prices",
            rules_path=path,
        )


def test_recorder_uses_same_summary_rule():
    from scripts.rust_contract.record import differences

    assert not differences([entry(A + b"," + B)], [entry(B + b"," + A)])


def test_another_array_in_price_body_is_not_relaxed():
    assert recording_differences(
        [entry(A, suffix=b',"other":[1,2]}')],
        [entry(A, suffix=b',"other":[2,1]}')],
    )
