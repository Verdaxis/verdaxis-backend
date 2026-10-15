"""Executable synthetic tuple proof; Linux systemd remains a rehearsal gate."""
import pytest

BOUNDARIES = [
    'old-renamed', 'new-renamed', 'environment', 'runtime-identity',
    'monitor-identity', 'unit-written', 'unit-loaded', 'authorized',
    'restart', 'readiness',
]


@pytest.mark.parametrize('boundary', BOUNDARIES)
def test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked(tmp_path, boundary):
    from tests.phase0.tuple_fixture import TupleFixture

    fixture = TupleFixture(tmp_path)
    fixture.advance_to(boundary)
    try:
        # Both real start predicates run before the executable tuple probe.
        fixture.assert_public_write_blocked()
        fixture.assert_starts_match_or_are_blocked()
        fixture.interrupt()
        assert fixture.start().returncode != 0
        fixture.assert_public_write_blocked()
        fixture.assert_no_mismatched_execution()
        fixture.rollback()
        assert fixture.start().returncode == 0
        fixture.assert_public_write_blocked()
        fixture.assert_no_mismatched_execution()
    finally:
        fixture.close()


@pytest.mark.parametrize('database', ['verdaxis', 'verdaxis_prod'])
def test_mixed_environment_never_executes_under_fence(tmp_path, database):
    from tests.phase0.tuple_fixture import TupleFixture

    fixture = TupleFixture(tmp_path)
    try:
        fixture.advance_to('new-renamed' if database == 'verdaxis' else 'old-renamed')
        fixture.interrupt()
        fixture.restore_source_only(database)
        fixture.write_environment(database)
        assert fixture.start().returncode != 0
        fixture.assert_public_write_blocked()
        fixture.assert_no_mismatched_execution()
    finally:
        fixture.close()


def test_fixture_detects_open_public_write_route(tmp_path):
    from tests.phase0.tuple_fixture import TupleFixture

    fixture = TupleFixture(tmp_path)
    try:
        (tmp_path / 'maintenance').write_text('open')
        with pytest.raises(AssertionError):
            fixture.assert_public_write_blocked()
    finally:
        fixture.close()


def test_fixture_detects_unloaded_fence_and_wrong_database(tmp_path):
    from tests.phase0.tuple_fixture import TupleFixture

    fixture = TupleFixture(tmp_path)
    try:
        # Negative control: this test rig must detect the original unsafe window.
        fixture.disable_loaded_fence()
        fixture.write_environment('verdaxis_prod')
        assert fixture.start().returncode != 0
        with pytest.raises(AssertionError):
            fixture.assert_no_mismatched_execution()
    finally:
        fixture.close()
