"""Required operator gates must precede the irreversible checklist steps."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_fence_is_loaded_before_source_replacement_and_rollback_start():
    text = (ROOT / 'docs/runbooks/phase0-cutover.md').read_text()
    initial = text[text.index('## Install the separate fence'):text.index('## Drain before')]
    rollback = text[text.index('## Emergency pre-write rollback'):text.index('## Append-only')]
    for section in (initial, rollback):
        assert 'systemctl daemon-reload' in section
        assert 'LoadState' in section
        assert 'DropInPaths' in section
        assert 'ExecStartPre' in section
        assert 'NeedDaemonReload' in section
    assert rollback.index('systemctl daemon-reload') < rollback.index('sudo systemctl restart')


def test_reopening_checks_frontend_entry_and_pilot_refresh_note():
    text = (ROOT / 'docs/runbooks/phase0-cutover.md').read_text()
    assert 'new frontend entry HTML' in text
    assert 'existing update banner' in text
    assert 'please refresh' in text
    assert 'active pilot users' in text
