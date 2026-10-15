import pytest
from scripts.rust_contract.compare import capture, compare_news_tie_groups


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
