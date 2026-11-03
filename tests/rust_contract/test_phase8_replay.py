"""Run each retained receipt case through the real Rust PostgreSQL boundary."""
import os
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def run_case(name):
    # A missing disposable database is a failed gate, not a skipped required case.
    assert os.environ.get("RUST_TEST_DATABASE_URL"), "disposable runtime URL required"
    assert os.environ.get("RUST_TEST_MIGRATOR_DATABASE_URL"), "disposable migrator URL required"
    result = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests", "--lib",
            f"order_write::command::replay_tests::{name}", "--", "--exact", "--nocapture",
        ],
        cwd=ROOT,
        env={**os.environ, "SQLX_OFFLINE": "true"},
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=240,
    )
    assert result.returncode == 0, result.stdout
    assert "1 passed; 0 failed" in result.stdout, result.stdout


def test_receipt_rejects_wrong_resource_and_context():
    run_case("receipt_rejects_wrong_resource_and_context")


def test_receipt_replay_matrix():
    run_case("receipt_replay_matrix")


def test_receipt_hash_matches_python_vector():
    run_case("receipt_hash_matches_python_vector")


def test_inventory_replay_is_principal_bound():
    run_case("inventory_replay_is_principal_bound")


def test_no_query_after_commit_for_keyed_and_unkeyed():
    run_case("no_query_after_commit_for_keyed_and_unkeyed")


@pytest.mark.parametrize("case", [
    "receipt_insert_contention_replays_the_winner",
    "receipt_insert_contention_revalidates_the_winner",
    "ambiguous_commit_is_not_retried",
    "safely_aborted_commit_retries_only_the_approved_states",
])
def test_receipt_completion(case):
    run_case(case)


@pytest.mark.parametrize("case", [
    "ordered_replay_preserves_recorded_participation",
    "ordered_replay_refuses_foreign_identity_and_context",
    "ordered_receipt_collision_has_one_effect_set",
    "ordered_response_failure_rolls_back_staged_work",
    "ordered_commit_transport_loss_uses_receipt_recovery",
    "ordered_commit_server_abort_retries_safely",
])
def test_ordered_receipt_completion(case):
    run_case(case)
