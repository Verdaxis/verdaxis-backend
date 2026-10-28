"""A verification report is published only after complete serialization/fsync."""
import json
import os
import stat

import pytest


def publish(path, value):
    from scripts.phase0.verify import publish_report
    publish_report(path, value)


def test_report_is_private_and_complete(tmp_path):
    path = tmp_path / 'report.json'
    publish(path, {'verified': True})
    assert json.loads(path.read_text()) == {'verified': True}
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert list(tmp_path.iterdir()) == [path]


def test_report_serialization_failure_leaves_no_report(tmp_path):
    path = tmp_path / 'report.json'
    with pytest.raises(TypeError):
        publish(path, {'bad': object()})
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize('failure_call', [1, 2])
def test_report_fsync_failure_leaves_no_report(tmp_path, monkeypatch, failure_call):
    real_fsync = os.fsync
    calls = 0

    def fail(descriptor):
        nonlocal calls
        calls += 1
        if calls == failure_call:
            raise OSError('injected fsync failure')
        real_fsync(descriptor)

    monkeypatch.setattr(os, 'fsync', fail)
    with pytest.raises(OSError):
        publish(tmp_path / 'report.json', {'verified': True})
    assert list(tmp_path.iterdir()) == []


def test_report_write_failure_leaves_no_report(tmp_path, monkeypatch):
    def fail(*args, **kwargs):
        raise OSError('injected write failure')
    monkeypatch.setattr(os, 'write', fail)
    with pytest.raises(OSError):
        publish(tmp_path / 'report.json', {'verified': True})
    assert list(tmp_path.iterdir()) == []


def test_existing_report_is_never_overwritten(tmp_path):
    path = tmp_path / 'report.json'
    path.write_text('previous evidence')
    with pytest.raises(FileExistsError):
        publish(path, {'verified': True})
    assert path.read_text() == 'previous evidence'
    assert list(tmp_path.iterdir()) == [path]


def test_report_racing_destination_is_not_removed(tmp_path, monkeypatch):
    real_link = os.link
    path = tmp_path / 'report.json'

    def race(source, destination, **kwargs):
        path.write_text('other writer')
        return real_link(source, destination, **kwargs)

    monkeypatch.setattr(os, 'link', race)
    with pytest.raises(FileExistsError):
        publish(path, {'verified': True})
    assert path.read_text() == 'other writer'
    assert list(tmp_path.iterdir()) == [path]


def test_report_symlink_parent_is_refused(tmp_path):
    actual = tmp_path / 'actual'
    actual.mkdir()
    link = tmp_path / 'link'
    link.symlink_to(actual, target_is_directory=True)
    with pytest.raises(ValueError):
        publish(link / 'report.json', {'verified': True})
    assert list(actual.iterdir()) == []
