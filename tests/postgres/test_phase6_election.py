"""Run the elected-session sequencing component on disposable PostgreSQL.

The Rust target ``streams_election`` holds the scenarios. Its database tests
are ignored in ordinary builds; this suite runs every one of them with the
deployed application role and fails unless each named test passed. A second
case holds the sequencer lock with the unchanged Python dispatcher and shows
that the Rust session cannot assign while Python leads.
"""

from __future__ import annotations

import asyncio
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from app.services.market_event_dispatch import (
    SEQUENCER_ADVISORY_LOCK_KEY,
    MarketEventDispatcher,
)

ROOT = Path(__file__).resolve().parents[2]
TARGET = "streams_election"
OFFLINE_TESTS = {
    "shared_sequencer_key",
    "failure_budget_bounds_attempts",
    "session_settings_reject_unsafe_targets",
}
DATABASE_TESTS = {
    "unlock_false_discards_session",
    "unlock_error_discards_session",
    "repeated_assignment_failure_stops_spinning",
    "python_lock_blocks_assignment",
    "session_lock_count_balanced",
    "committed_rows_only",
    "lock_and_assignment_share_backend",
    "full_transaction_retry",
    "session_replacement_requires_election",
    "unknown_commit_outcome",
    "connection_deadlines",
    "session_close_releases_lock",
    "wake_follows_other_sessions",
    "assignment_waits_for_shared_row",
    "session_pool_stays_closed",
    "session_ends_before_commit",
}
PASSED_LINE = re.compile(r"^test (\w+) \.\.\. ok$", re.MULTILINE)
SUMMARY_LINE = re.compile(
    r"^test result: ok\. (\d+) passed; 0 failed; 0 ignored;", re.MULTILINE
)


def _app_url() -> str:
    url = os.environ["DATABASE_URL"]
    assert url.startswith("postgresql+asyncpg://"), "the harness supplies the app role URL"
    return url


def _cargo(*selection: str, extra_env: dict[str, str] | None = None) -> set[str]:
    """Run the target with its database tests included; return passed names."""
    env = {
        **os.environ,
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "RUST_TEST_PYTHON": sys.executable,
        "SQLX_OFFLINE": "true",
        "STREAM_ELECTION_APP_URL": _app_url().replace(
            "postgresql+asyncpg://", "postgresql://", 1
        ),
        **(extra_env or {}),
    }
    filters = [selection[0]] if selection else []
    exact = ["--exact"] if selection else []
    result = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests", "--test", TARGET,
            *filters, "--", "--include-ignored", "--test-threads=1", *exact,
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    print(result.stdout)
    print(result.stderr[-4000:])
    assert result.returncode == 0, f"cargo test exited {result.returncode}"
    passed = set(PASSED_LINE.findall(result.stdout))
    summary = SUMMARY_LINE.search(result.stdout)
    assert summary is not None, "no passing test summary"
    assert int(summary.group(1)) == len(passed)
    return passed


@pytest.mark.asyncio
async def test_rust_election_component(analytics_pg_url):
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.begin() as connection:
            await connection.execute(text(
                "CREATE TABLE phase6_election_shared_row (singleton boolean PRIMARY KEY, generation bigint);"
            ))
            await connection.execute(text(
                "INSERT INTO phase6_election_shared_row VALUES (true, 0)"
            ))
            role = os.environ["RUNTIME_TEST_APP_ROLE"]
            assert re.fullmatch(r"[a-z_]+", role)
            await connection.execute(text(
                f"GRANT SELECT, UPDATE ON phase6_election_shared_row TO {role}"
            ))
            await connection.execute(text("""
                CREATE FUNCTION phase6_election_shared_row_update() RETURNS trigger
                LANGUAGE plpgsql AS $$ BEGIN
                    UPDATE phase6_election_shared_row SET generation = generation + 1;
                    RETURN NEW;
                END $$
            """))
            await connection.execute(text("""
                CREATE TRIGGER phase6_election_shared_row_update
                AFTER UPDATE ON market_event_outbox FOR EACH ROW
                WHEN (NEW.aggregate_id = 'assignment_waits_for_shared_row')
                EXECUTE FUNCTION phase6_election_shared_row_update()
            """))
        assert await asyncio.to_thread(_cargo) == OFFLINE_TESTS | DATABASE_TESTS
    finally:
        async with engine.begin() as connection:
            await connection.execute(text(
                "DROP TRIGGER IF EXISTS phase6_election_shared_row_update ON market_event_outbox"
            ))
            await connection.execute(text(
                "DROP FUNCTION IF EXISTS phase6_election_shared_row_update()"
            ))
            await connection.execute(text("DROP TABLE IF EXISTS phase6_election_shared_row"))
        await engine.dispose()


@pytest.mark.asyncio
async def test_python_dispatcher_lock_blocks_rust_assignment(analytics_pg_url):
    assert SEQUENCER_ADVISORY_LOCK_KEY == 7264537260226625
    engine = create_async_engine(_app_url())
    dispatcher = MarketEventDispatcher(engine, bus=None)
    pending_sql = text(
        "SELECT count(*) FROM market_event_outbox "
        "WHERE aggregate_type = 'phase6_election' "
        "AND aggregate_id = 'python_lock_blocks_assignment' AND stream_seq IS NULL"
    )
    try:
        await dispatcher._connect_listener()
        assert await dispatcher._try_acquire_leadership()
        passed = await asyncio.to_thread(
            _cargo,
            "python_lock_blocks_assignment",
            extra_env={"STREAM_ELECTION_LOCK_HOLDER": "python"},
        )
        assert passed == {"python_lock_blocks_assignment"}
        async with engine.connect() as connection:
            assert await connection.scalar(pending_sql) == 2
        # Python still leads and sequences the rows Rust left pending.
        assert await dispatcher._assign_pending_sequences() >= 2
        async with engine.connect() as connection:
            assert await connection.scalar(pending_sql) == 0
    finally:
        await dispatcher.stop()
        await engine.dispose()
