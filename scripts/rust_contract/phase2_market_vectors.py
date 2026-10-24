"""Export the reviewed order-book write vectors from Python into one spec file.

Test-host-only and read-only: it imports the four Python modules that own the
market write boundary (`market_locks`, `market_transactions`, `idempotency`,
`command_results`), records their constants, their lock-key results and their
request hashes, and writes `tests/rust_contract/phase2/market-write-spec.json`.
It adds no Python feature, opens no database and sends no HTTP request.

Usage:

    python -m scripts.rust_contract.phase2_market_vectors --print
    python -m scripts.rust_contract.phase2_market_vectors --write
    python -m scripts.rust_contract.phase2_market_vectors --check

`--print` writes one marker-delimited JSON document to stdout so an isolated
test host can return it to the developer without a shared filesystem. `--check`
recomputes the document and compares it with the committed file: a Python
constant, hash or lock key that moves without a regenerated spec fails there,
and the Rust boundary test compares Rust against the same committed file.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import importlib
import json
import re
import sys
from datetime import UTC, date, datetime, timedelta, timezone
from decimal import Decimal
from enum import Enum
from pathlib import Path
from uuid import UUID

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

SPEC_PATH = ROOT / "tests/rust_contract/phase2/market-write-spec.json"
BEGIN_MARKER = "MARKET_VECTORS_BEGIN"
END_MARKER = "MARKET_VECTORS_END"

# The Python sources this spec pins. A digest change means the reviewed
# behaviour moved; regenerate the spec and re-review the difference.
SOURCES = (
    "app/services/market_locks.py",
    "app/services/market_transactions.py",
    "app/services/idempotency.py",
    "app/services/command_results.py",
    "app/models/command_result.py",
    "app/models/market_event.py",
    "app/services/db_errors.py",
    "deploy/postgres/app_acl_policy.sql",
)

# Tables the boundary writes and the grants that already allow it. Global
# constraint 9: the port adds no migration, view, function, grant or role.
WRITE_TABLES = ("market_command_results", "market_event_outbox")


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _source_digests() -> dict[str, str]:
    return {name: _sha256_text((ROOT / name).read_text(encoding="utf-8")) for name in SOURCES}


def _encode_canonical(value: object) -> object:
    """Encode one Python value as the tagged form both stacks decode.

    The tag removes every cross-language ambiguity (Python `Decimal` against a
    Java-style binary float, `UUID` against a string) while staying readable in
    review. `phase2` payloads are `model_dump(mode="json")` output plus the
    typed values below.
    """
    if value is None:
        return {"k": "null"}
    if isinstance(value, bool):
        return {"k": "bool", "v": value}
    if isinstance(value, int):
        return {"k": "int", "v": value}
    if isinstance(value, str):
        return {"k": "text", "v": value}
    if isinstance(value, float):
        raise ValueError("market write payloads must not carry binary floats")
    if isinstance(value, Enum):
        return _encode_canonical(value.value)
    if isinstance(value, Decimal):
        return {"k": "decimal", "v": str(value)}
    if isinstance(value, UUID):
        return {"k": "uuid", "v": str(value)}
    if isinstance(value, datetime):
        if value.tzinfo is None or value.utcoffset() is None:
            raise ValueError("vector datetimes must be timezone-aware")
        return {"k": "timestamp", "v": value.isoformat()}
    if isinstance(value, date):
        return {"k": "date", "v": value.isoformat()}
    if isinstance(value, dict):
        if any(not isinstance(key, str) for key in value):
            raise ValueError("vector objects must use string keys")
        return {"k": "object", "v": {key: _encode_canonical(item) for key, item in value.items()}}
    if isinstance(value, (list, tuple)):
        return {"k": "array", "v": [_encode_canonical(item) for item in value]}
    raise ValueError(f"unsupported vector value: {type(value).__name__}")


def _decode_canonical(node: object) -> object:
    """Decode the tagged form back into the Python value it encodes."""
    kind = node["k"]
    if kind == "null":
        return None
    if kind == "bool":
        return bool(node["v"])
    if kind == "int":
        return int(node["v"])
    if kind == "text":
        return node["v"]
    if kind == "decimal":
        return Decimal(node["v"])
    if kind == "uuid":
        return UUID(node["v"])
    if kind == "timestamp":
        return datetime.fromisoformat(node["v"])
    if kind == "date":
        return date.fromisoformat(node["v"])
    if kind == "object":
        return {key: _decode_canonical(item) for key, item in node["v"].items()}
    if kind == "array":
        return [_decode_canonical(item) for item in node["v"]]
    raise ValueError(f"unknown vector tag: {kind}")


# One reviewed slice-lock key case table. `side` is recorded to prove the hash
# ignores it: BID and ASK share one executable slice.
SLICE_KEY_CASES = (
    ("spot-bid", "BID", "10000000-0000-4000-8000-0000000000a1", None, "SPOT"),
    ("spot-ask-same-slice", "ASK", "10000000-0000-4000-8000-0000000000a1", None, "SPOT"),
    ("spot-alias-and-padding", "BID", "10000000-0000-4000-8000-0000000000a1", None, " Spot "),
    ("month", "ASK", "10000000-0000-4000-8000-0000000000a1", "20000000-0000-4000-8000-0000000000b1", "2027-03"),
    ("quarter", "BID", "10000000-0000-4000-8000-0000000000a1", "20000000-0000-4000-8000-0000000000b1", "2026-Q1"),
    ("quarter-legacy-alias", "ASK", "10000000-0000-4000-8000-0000000000a1", "20000000-0000-4000-8000-0000000000b1", "Q1_2026"),
    ("calendar", "ASK", "10000000-0000-4000-8000-0000000000a2", None, "2028-CAL"),
    ("no-delivery-point", "BID", "10000000-0000-4000-8000-0000000000a3", None, "2026-12"),
)

# One reviewed multi-slice lock schedule: duplicates, two sides of one slice and
# three distinct slices in an order that is not already sorted.
SLICE_SORT_CASES = (
    (
        "unsorted-with-duplicates",
        (
            ("ASK", "10000000-0000-4000-8000-0000000000a2", None, "2026-Q1"),
            ("BID", "10000000-0000-4000-8000-0000000000a1", "20000000-0000-4000-8000-0000000000b1", "SPOT"),
            ("ASK", "10000000-0000-4000-8000-0000000000a1", "20000000-0000-4000-8000-0000000000b1", "Spot"),
            ("BID", "10000000-0000-4000-8000-0000000000a1", None, "2027-CAL"),
        ),
    ),
    (
        "point-sorts-after-empty",
        (
            ("BID", "10000000-0000-4000-8000-0000000000a1", "20000000-0000-4000-8000-0000000000b1", "SPOT"),
            ("BID", "10000000-0000-4000-8000-0000000000a1", None, "SPOT"),
        ),
    ),
    ("single", (("BID", "10000000-0000-4000-8000-0000000000a9", None, "2026-Q4"),)),
)

IDEMPOTENCY_KEY_CASES = (
    ("tr", "30000000-0000-4000-8000-0000000000c1", "trade.confirm", "retry-key"),
    ("order", "30000000-0000-4000-8000-0000000000c1", "order.amend", "retry-key"),
    ("other-scope", "30000000-0000-4000-8000-0000000000c2", "order.amend", "retry-key"),
    ("unicode-key", "30000000-0000-4000-8000-0000000000c1", "order.cancel", "ключ-1"),
    ("padded", "30000000-0000-4000-8000-0000000000c1", "order.cancel", "  padded  "),
)

COMMAND_HASH_CASES = (
    (
        "canonical-economic-values",
        {
            "operation": "order.amend",
            "order_id": UUID("10000000-0000-4000-8000-0000000000a1"),
            "update": {
                "price_per_mt_usd": Decimal("750.00"),
                "quantity_mt": Decimal("10"),
                "expires_at": datetime(2026, 10, 3, 12, tzinfo=UTC),
                "availability_window": "Q1_2026",
            },
        },
    ),
    (
        "normalized-decimal-and-zone",
        {
            "update": {
                "price_per_mt_usd": Decimal("750"),
                "quantity_mt": Decimal("10.00"),
                "expires_at": datetime(2026, 10, 3, 20, tzinfo=timezone(timedelta(hours=8))),
                "availability_window": "Q1_2026",
            },
            "order_id": UUID("10000000-0000-4000-8000-0000000000a1"),
            "operation": "order.amend",
        },
    ),
    (
        "absent-key-differs-from-null-key",
        {"operation": "trade.decline", "reason": None},
    ),
    (
        "absent-only",
        {"operation": "trade.decline"},
    ),
    (
        "nested-and-unicode",
        {
            "operation": "trade.confirm",
            "note": "café ☕ \u2028",
            "items": [1, True, None, {"z": "0", "a": "1"}],
        },
    ),
)

IDEMPOTENCY_HASH_CASES = (
    (
        "json-native-order-payload",
        {
            "side": "BID",
            "price_per_mt_usd": "750.00",
            "quantity_mt": 10,
            "delivery_point_id": "20000000-0000-4000-8000-0000000000b1",
            "is_demo": False,
            "certifications": ["ISCC"],
        },
    ),
    (
        "nested-json",
        {"actor_id": "30000000-0000-4000-8000-0000000000c1", "offer": {"price": 1, "tags": []}},
    ),
    ("empty-object", {}),
)


def _slice_lock_rows(market_locks) -> list[dict[str, object]]:
    rows = []
    for label, side, product_id, delivery_point_id, window in SLICE_KEY_CASES:
        key = market_locks.market_slice_lock_key(
            side=side,
            product_id=UUID(product_id),
            delivery_point_id=None if delivery_point_id is None else UUID(delivery_point_id),
            availability_window=window,
        )
        rows.append(
            {
                "label": label,
                "side": side,
                "product_id": product_id,
                "delivery_point_id": delivery_point_id,
                "availability_window": window,
                "normalized_window": market_locks.normalize_availability_window(window),
                "lock_key": key,
                "sort_key": list(
                    market_locks.market_slice_lock_sort_key(
                        (
                            side,
                            UUID(product_id),
                            None if delivery_point_id is None else UUID(delivery_point_id),
                            window,
                        )
                    )
                ),
            }
        )
    return rows


class _RecordingLockSession:
    """One session double that records the reviewed lock path, not a guess."""

    def __init__(self) -> None:
        self.statements: list[str] = []
        self.lock_keys: list[int] = []

    async def execute(self, statement, parameters=None):  # noqa: ANN001, ANN201
        self.statements.append(getattr(statement, "text", str(statement)))
        if parameters and "lock_key" in parameters:
            self.lock_keys.append(parameters["lock_key"])
        return None

    def get_bind(self):  # noqa: ANN201
        class _Dialect:
            name = "postgresql"

        class _Bind:
            dialect = _Dialect()

        return _Bind()


def _slice_sort_rows(market_locks) -> list[dict[str, object]]:
    """Record the real acquisition order of `acquire_market_slice_locks`.

    The reviewed helper deduplicates on the side-independent slice identity and
    sorts by the canonical key, then acquires each advisory lock. Recording the
    executed lock keys derives both the order and the key from Python itself.
    """
    rows = []
    for label, keys in SLICE_SORT_CASES:
        typed = [
            (side, UUID(product_id), None if point is None else UUID(point), window)
            for side, product_id, point, window in keys
        ]
        session = _RecordingLockSession()
        asyncio.run(market_locks.acquire_market_slice_locks(session, typed))
        rows.append(
            {
                "label": label,
                "input": [
                    {
                        "side": side,
                        "product_id": str(product_id),
                        "delivery_point_id": None if point is None else str(point),
                        "availability_window": window,
                    }
                    for side, product_id, point, window in keys
                ],
                "acquired_lock_keys": session.lock_keys,
            }
        )
    return rows


def _idempotency_key_rows(idempotency) -> list[dict[str, object]]:
    return [
        {
            "label": label,
            "scope_id": scope_id,
            "operation": operation,
            "key": key,
            "lock_key": idempotency.idempotency_lock_key(UUID(scope_id), operation, key),
        }
        for label, scope_id, operation, key in IDEMPOTENCY_KEY_CASES
    ]


def _command_hash_rows(command_results) -> list[dict[str, object]]:
    rows = []
    for label, payload in COMMAND_HASH_CASES:
        rows.append(
            {
                "label": label,
                "payload": _encode_canonical(payload),
                "sha256": command_results.command_request_hash(payload),
            }
        )
    return rows


def _idempotency_hash_rows(idempotency) -> list[dict[str, object]]:
    rows = []
    for label, payload in IDEMPOTENCY_HASH_CASES:
        encoded = _encode_canonical(payload)
        decoded = _decode_canonical(encoded)
        if decoded != payload:
            raise AssertionError(f"vector {label} does not round-trip")
        rows.append(
            {
                "label": label,
                "payload": encoded,
                "sha256": idempotency.idempotency_request_hash(payload),
            }
        )
    return rows


def _model_columns(module_name: str, table: str) -> list[str]:
    module = importlib.import_module(module_name)
    for value in vars(module).values():
        table_value = getattr(value, "__table__", None)
        if table_value is not None and getattr(table_value, "name", None) == table:
            return [column.name for column in table_value.columns]
    raise AssertionError(f"model for {table} not found in {module_name}")


def _table_grants(table: str) -> list[str]:
    """Read the reviewed ACL entry for one table from the deploy policy text."""
    text = (ROOT / "deploy/postgres/app_acl_policy.sql").read_text(encoding="utf-8")
    match = re.search(rf"\('{re.escape(table)}',\s*ARRAY\[([^\]]*)\]\)", text)
    if match is None:
        raise AssertionError(f"no reviewed grant entry for {table}")
    return [part.strip().strip("'") for part in match.group(1).split(",")]


def _captured_lock_sql(market_locks) -> dict[str, str]:
    """Record the exact statement text the reviewed slice-lock path executes.

    The double stands in for one session and records the literal SQL text the
    reviewed Python issues before its first advisory lock, plus the advisory
    lock call itself.
    """

    async def _record() -> dict[str, str]:
        session = _RecordingLockSession()
        await market_locks._acquire_advisory_lock(session, key=1, busy_detail="busy")
        statements = list(session.statements)
        if len(statements) != 3:
            raise AssertionError("one lock path must issue the two bounds and one lock")
        return {
            "market_slice_set_lock_timeout": statements[0],
            "market_slice_set_statement_timeout": statements[1],
            "market_slice_advisory_lock": statements[2],
        }

    return asyncio.run(_record())


def _attempt_bounds(market_transactions) -> list[int]:
    """Derive the accepted retry bound from the reviewed factory itself."""
    for accepted in (1, 2, 3):
        market_transactions.retry_market_transaction(max_attempts=accepted)
    rejected = []
    for candidate in (0, 4):
        try:
            market_transactions.retry_market_transaction(max_attempts=candidate)
        except ValueError:
            rejected.append(candidate)
    if rejected != [0, 4]:
        raise AssertionError("market transaction attempt bound moved")
    return [1, 3]



def build_spec() -> dict[str, object]:
    from app.services import command_results, idempotency, market_locks, market_transactions

    ids = idempotency
    operations = {
        "order_amend": command_results.ORDER_AMEND_OPERATION,
        "order_cancel": command_results.ORDER_CANCEL_OPERATION,
        "trade_confirm": command_results.TRADE_CONFIRM_OPERATION,
        "trade_decline": command_results.TRADE_DECLINE_OPERATION,
        "trade_deliver": command_results.TRADE_DELIVER_OPERATION,
        "trade_pay": command_results.TRADE_PAY_OPERATION,
    }
    if len(set(operations.values())) != len(operations):
        raise AssertionError("receipt operations must be distinct")
    idempotency_operations = {
        "order_create": ids.ORDER_CREATE_OPERATION,
        "trade_create": ids.TRADE_CREATE_OPERATION,
        "inventory_publish": ids.INVENTORY_PUBLISH_OPERATION,
    }
    if set(idempotency_operations.values()) & set(operations.values()):
        raise AssertionError("a receipt operation must not be a resource-row operation")

    return {
        "version": 1,
        "generated_by": "scripts/rust_contract/phase2_market_vectors.py",
        "source_sha256": _source_digests(),
        "constants": {
            "python_requires": "3.12",
            "market_lock_timeout": market_locks.MARKET_LOCK_TIMEOUT,
            "market_statement_timeout": market_locks.MARKET_STATEMENT_TIMEOUT,
            "idempotency_lock_timeout": ids.IDEMPOTENCY_LOCK_TIMEOUT,
            "idempotency_statement_timeout": ids.IDEMPOTENCY_STATEMENT_TIMEOUT,
            "retryable_sqlstates": sorted(market_transactions.RETRYABLE_MARKET_SQLSTATES),
            "market_transaction_attempts": market_transactions.DEFAULT_MARKET_TRANSACTION_ATTEMPTS,
            "market_transaction_attempt_bounds": _attempt_bounds(market_transactions),
            "lock_busy_sqlstate": "55P03",
            "lock_timeout_sqlstate": "57014",
            "idempotency_key_min": 1,
            "idempotency_key_max": 255,
            "messages": {
                "key_bounds": "Idempotency-Key must be 1-255 characters",
                "market_organization": "Market organization is required",
                "replay_conflict": (
                    "Idempotency-Key was reused with a different request or actor context"
                ),
                "slice_busy": "Market slice is busy; retry the request",
                "slice_timeout": "Market slice lock timed out; retry the request",
                "serialization_failed": (
                    "Market transaction serialization failed; retry the request"
                ),
                "idempotency_busy": "Idempotency key is busy; retry the same request",
                "idempotency_timeout": "Idempotency lock timed out; retry the request",
                "order_not_found": "Order not found",
            },
            "receipt_operations": operations,
            "idempotency_operations": idempotency_operations,
            "lock_sql": _captured_lock_sql(market_locks),
        },
        "write_tables": [
            {"table": table, "grants": _table_grants(table), "columns": _model_columns(module, table)}
            for table, module in (
                ("market_command_results", "app.models.command_result"),
                ("market_event_outbox", "app.models.market_event"),
            )
        ],
        "slice_lock_keys": _slice_lock_rows(market_locks),
        "slice_lock_sort": _slice_sort_rows(market_locks),
        "idempotency_lock_keys": _idempotency_key_rows(ids),
        "command_request_hashes": _command_hash_rows(command_results),
        "idempotency_request_hashes": _idempotency_hash_rows(ids),
    }


def _document() -> str:
    return json.dumps(build_spec(), indent=2, sort_keys=False, ensure_ascii=False) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--print", action="store_true", help="write the marker-delimited document")
    group.add_argument("--write", action="store_true", help="write the committed spec file")
    group.add_argument("--check", action="store_true", help="compare the spec with Python")
    args = parser.parse_args(argv)

    document = _document()
    if args.print:
        sys.stdout.write(f"{BEGIN_MARKER}\n{document}{END_MARKER}\n")
        return 0
    if args.write:
        SPEC_PATH.parent.mkdir(parents=True, exist_ok=True)
        SPEC_PATH.write_text(document, encoding="utf-8")
        print(f"wrote {SPEC_PATH.relative_to(ROOT)}")
        return 0
    committed = SPEC_PATH.read_text(encoding="utf-8") if SPEC_PATH.exists() else None
    if committed is None:
        print(f"spec missing: {SPEC_PATH.relative_to(ROOT)}", file=sys.stderr)
        return 1
    if committed != document:
        print("market write spec is stale; regenerate it from this Python source", file=sys.stderr)
        return 1
    print("market write spec matches Python")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
