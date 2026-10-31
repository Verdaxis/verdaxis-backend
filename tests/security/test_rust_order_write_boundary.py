"""The Rust order-book write boundary is generated from, and pinned to, Python.

Test-host-only and read-only. The reviewed Python remains the oracle: this suite
recomputes the committed spec through the generator, proves the source digests
still describe the files the spec was built from, and pins every constant, lock
statement, write table and operation the boundary stores into the Rust source
itself. No Python feature is added and no database or HTTP call is made.

The Rust side of the same contract lives in `rs/read/tests/order_write_boundary.rs`.
"""

import hashlib
import json
import re
from pathlib import Path

from scripts.rust_contract import phase2_market_vectors

ROOT = Path(__file__).resolve().parents[2]
SPEC_PATH = ROOT / "tests/rust_contract/phase2/market-write-spec.json"
BOUNDARY = ROOT / "rs/read/src/order_write"
REGISTRATION = ROOT / "rs/read/src/lib.rs"

# The reviewed source files the spec pins, with their roles.
REVIEWED_SOURCES = (
    "app/services/market_locks.py",
    "app/services/market_transactions.py",
    "app/services/idempotency.py",
    "app/services/command_results.py",
    "app/models/command_result.py",
    "app/models/market_event.py",
    "app/services/db_errors.py",
    "deploy/postgres/app_acl_policy.sql",
)

WRITE_TABLES = ("market_command_results", "market_event_outbox")

# Each Rust constant and the spec value it must carry. The Rust test compares
# the same values at compile time; this pin fails on the test host even when no
# Rust toolchain is present.
CONSTANT_PINS = {
    "MARKET_LOCK_TIMEOUT": ("market_lock_timeout",),
    "MARKET_STATEMENT_TIMEOUT": ("market_statement_timeout",),
    "IDEMPOTENCY_LOCK_TIMEOUT": ("idempotency_lock_timeout",),
    "IDEMPOTENCY_STATEMENT_TIMEOUT": ("idempotency_statement_timeout",),
    "LOCK_BUSY_SQLSTATE": ("lock_busy_sqlstate",),
    "LOCK_TIMEOUT_SQLSTATE": ("lock_timeout_sqlstate",),
    "IDEMPOTENCY_KEY_MIN": ("idempotency_key_min",),
    "IDEMPOTENCY_KEY_MAX": ("idempotency_key_max",),
    "DEFAULT_MARKET_TRANSACTION_ATTEMPTS": ("market_transaction_attempts",),
}

MESSAGE_PINS = {
    "IDEMPOTENCY_KEY_BOUNDS": "key_bounds",
    "MARKET_ORGANIZATION_REQUIRED": "market_organization",
    "REPLAY_CONFLICT_MESSAGE": "replay_conflict",
    "SLICE_BUSY_MESSAGE": "slice_busy",
    "SLICE_TIMEOUT_MESSAGE": "slice_timeout",
    "SERIALIZATION_FAILED_MESSAGE": "serialization_failed",
    "IDEMPOTENCY_BUSY_MESSAGE": "idempotency_busy",
    "IDEMPOTENCY_TIMEOUT_MESSAGE": "idempotency_timeout",
    "ORDER_NOT_FOUND_MESSAGE": "order_not_found",
}

_CONSTANT = re.compile(
    r'pub const ([A-Z0-9_]+):\s*(?:&str|usize|\[&str;\s*\d+\])\s*=\s*'
    r'("(?:[^"\\]|\\.)*"|\[[^\]]*\]|\d+);',
    re.DOTALL,
)
_STRING_ARRAY = re.compile(r'\[(.*)\]', re.DOTALL)
_STRING = re.compile(r'"((?:[^"\\]|\\.)*)"')


def spec() -> dict:
    return json.loads(SPEC_PATH.read_text(encoding="utf-8"))


def boundary_source(name: str) -> str:
    return (BOUNDARY / name).read_text(encoding="utf-8")


def rust_constants() -> dict[str, object]:
    """Every `pub const` in the boundary, as decoded Rust values."""
    found: dict[str, object] = {}
    for name in ("locks.rs", "transaction.rs", "command.rs"):
        for match in _CONSTANT.finditer(boundary_source(name)):
            constant, raw = match.group(1), match.group(2).strip()
            if raw.startswith("["):
                body = _STRING_ARRAY.match(raw)
                found[constant] = _STRING.findall(body.group(1)) if body else []
            elif raw.startswith('"'):
                found[constant] = _STRING.match(raw).group(1)
            else:
                found[constant] = int(raw)
    return found


def test_lock_and_write_methods_are_private():
    source = boundary_source("transaction.rs")
    assert "pub trait WriteSession" not in source
    assert "pub async fn run_write_unit" not in source
    entry = source[source.index("pub async fn execute_order_command"):]
    entry = entry[:entry.index("\n}")]
    assert "NOT_IMPLEMENTED" in entry
    assert "run_write_unit" not in entry


def test_committed_spec_matches_the_reviewed_python():
    assert SPEC_PATH.exists(), f"missing {SPEC_PATH.relative_to(ROOT)}"
    # `--check` recomputes the document from the reviewed Python and compares it
    # byte for byte with the committed file.
    assert phase2_market_vectors.main(["--check"]) == 0


def test_source_digests_still_describe_the_reviewed_files():
    document = spec()
    digests = document["source_sha256"]
    assert set(digests) == set(REVIEWED_SOURCES)
    for source, digest in digests.items():
        actual = hashlib.sha256((ROOT / source).read_bytes()).hexdigest()
        assert actual == digest, f"{source} moved without a regenerated spec"


def test_lock_statements_and_bounds_are_the_reviewed_ones():
    constants = spec()["constants"]
    assert constants["market_lock_timeout"] == "500ms"
    assert constants["market_statement_timeout"] == "5s"
    assert constants["idempotency_lock_timeout"] == "500ms"
    assert constants["idempotency_statement_timeout"] == "5s"
    assert constants["retryable_sqlstates"] == ["40001", "40P01"]
    assert constants["market_transaction_attempts"] == 2
    assert constants["market_transaction_attempt_bounds"] == [1, 3]
    lock_sql = constants["lock_sql"]
    assert lock_sql["market_slice_set_lock_timeout"] == (
        f"SET LOCAL lock_timeout = '{constants['market_lock_timeout']}'"
    )
    assert lock_sql["market_slice_set_statement_timeout"] == (
        f"SET LOCAL statement_timeout = '{constants['market_statement_timeout']}'"
    )
    assert lock_sql["market_slice_advisory_lock"] == "SELECT pg_advisory_xact_lock(:lock_key)"


def test_rust_constants_pin_the_python_values():
    constants = spec()["constants"]
    found = rust_constants()
    for name, path in CONSTANT_PINS.items():
        assert name in found, f"{name} is missing from the boundary"
        expected = constants[path[0]]
        assert found[name] == expected, f"{name}: {found[name]!r} != {expected!r}"
    messages = constants["messages"]
    for name, key in MESSAGE_PINS.items():
        assert name in found, f"{name} is missing from the boundary"
        assert found[name] == messages[key], f"{name}: {found[name]!r} != {messages[key]!r}"
    assert found["RETRYABLE_SQLSTATES"] == constants["retryable_sqlstates"]
    assert found["MAX_MARKET_TRANSACTION_ATTEMPTS"] == constants[
        "market_transaction_attempt_bounds"
    ][1]


def test_rust_receipt_operations_are_the_reviewed_six():
    constants = spec()["constants"]
    source = boundary_source("command.rs")
    expected = sorted(constants["receipt_operations"].values())
    # Only the operation names carry a dot; the resource-type arms do not.
    actual = sorted(re.findall(r'Operation::\w+ => "([^"]*\.[^"]*)"', source))
    assert actual == expected
    # Every operation declares its stored resource type, and no resource-row
    # operation is a receipt operation.
    normalized = " ".join(source.split())
    assert 'Operation::OrderAmend | Operation::OrderCancel => "order"' in normalized
    trade_arms = (
        'Operation::TradeConfirm | Operation::TradeDecline | '
        'Operation::TradeDeliver | Operation::TradePay => "trade"'
    )
    assert trade_arms in normalized
    for operation in constants["idempotency_operations"].values():
        assert operation not in source, f"{operation} must not be a receipt operation"


def test_rust_writes_only_the_two_reviewed_tables():
    document = spec()
    tables = {row["table"]: row for row in document["write_tables"]}
    assert sorted(tables) == sorted(WRITE_TABLES)
    for name in WRITE_TABLES:
        grants = tables[name]["grants"]
        assert "SELECT" in grants and "INSERT" in grants, f"{name}: {grants}"
    assert "UPDATE" in tables["market_event_outbox"]["grants"]
    source = boundary_source("transaction.rs")
    targets = {target.lower() for target in re.findall(r"INSERT INTO (\w+)", source, re.IGNORECASE)}
    assert targets == set(WRITE_TABLES), targets


def test_the_boundary_registration_is_one_marked_block_with_no_route():
    manifest = REGISTRATION.read_text(encoding="utf-8")
    lines = [line.strip() for line in manifest.splitlines()]
    assert lines.count("pub mod order_write;") == 1
    assert "Phase 2 Task 20 registration block" in manifest
    block = manifest[manifest.index("Phase 2 Task 20 registration block") :]
    block = block[: block.index("end Task 20 registration block")]
    assert "mounts no route" in " ".join(block.split())
    # No route module reaches the boundary.
    for path in sorted((ROOT / "rs/read/src/routes").glob("*.rs")):
        assert "order_write" not in path.read_text(encoding="utf-8"), path.name
