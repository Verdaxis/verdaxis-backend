"""Task 36 register fixtures, not installed acceptance or a restore rehearsal."""
import argparse
from pathlib import Path

from inventory import parse, read_bytes, validate

REGISTER = Path(__file__).resolve().parents[1] / 'host-dependencies.json'
ACCEPTANCE = {'owner': 'PM', 'date': '2026-11-05', 'scope': 'classification-only'}


def required_verdaxis_python_has_no_exception(document):
    assert document['scope'] == 'collected-manual', 'required W0 rows have no PM classification'
    rows = document['dependencies']
    assert rows and set(document['required_dependencies']) == {row['id'] for row in rows}
    for row in rows:
        policy = row['automation']
        assert not row['removed_package_dependencies'], row['id']
        if policy['python_dependency'] == 'verdaxis':
            assert policy['decision'] == 'retire' and policy['window'] == 'W5', row['id']
        else:
            assert policy['decision'] == 'retained' and policy['window'] == 'unchanged', row['id']


def only_superseded_exact_cron_entries_are_removed(document):
    assert document['manual_collection']['cron_entries'] == {'production': [], 'staging': []}
    assert document['manual_collection']['cron_removals'] == [], 'no inventoried cron to remove'
    assert not any(row['kind'] == 'cron' for row in document['dependencies'])


def backup_restore_preserves_inventory_and_row_counts(document):
    rows = {row['id']: row for row in document['dependencies']}
    backup = rows['backup']
    assert backup['path'] == '/home/verdaxis-prod/backup-db.sh'
    assert backup['automation']['decision'] == 'retained'
    assert backup['automation']['window'] == 'unchanged'
    assert backup['automation']['python_dependency'] == 'system'
    restore = rows['restore']
    assert restore['path'] == '/home/verdaxis-prod/verify-backup-restore.sh'
    assert restore['automation']['decision'] == 'retained'
    assert restore['automation']['window'] == 'unchanged'
    assert restore['restore_contract'] == backup['restore_contract']
    assert 'inventory and row counts' in restore['restore_contract']
    assert document['manual_collection']['restore_verified'] is False


def staging_pull_preserves_target_identity_and_schedule(document):
    row = next(row for row in document['dependencies'] if row['id'] == 'staging-pull')
    assert row['host'] == 'staging' and row['path'] == '/home/verdaxis-prod/backup-db.sh'
    assert row['unix_identity'] == 'verdaxis-prod'
    assert row['database_identity'] == 'production export over SSH; SQL role uncollected'
    assert row['destination'] == '/home/verdaxis-prod/backups'
    assert row['automation']['window'] == 'unchanged'
    schedule = next(row for row in document['schedules'] if row['id'] == 'staging-pull')
    assert schedule == {'id': 'staging-pull', 'host': 'staging',
                        'expression': 'OnCalendar=*-*-* 03:00:00 Asia/Singapore',
                        'owner': 'verdaxis-backup.timer'}


def owner_acceptance_covers_each_required_replacement(document):
    for row in document['dependencies']:
        policy = row['automation']
        assert policy['owner_acceptance'] == ACCEPTANCE, row['id']
        if policy['decision'] == 'retire':
            assert policy['reason'].strip(), row['id']
            assert policy['replacement_tasks'] or row['id'] in {'codex-diagnose', 'old-production-units'}
    assert document['manual_collection']['installed_acceptance'] is False


CHECKS = {check.__name__: check for check in (
    required_verdaxis_python_has_no_exception,
    only_superseded_exact_cron_entries_are_removed,
    backup_restore_preserves_inventory_and_row_counts,
    staging_pull_preserves_target_identity_and_schedule,
    owner_acceptance_covers_each_required_replacement,
)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', choices=CHECKS, required=True)
    args = parser.parse_args()
    document = parse(read_bytes(REGISTER))
    validate(document)
    CHECKS[args.check](document)
    print('PASS ' + args.check + ' (register fixture only)')


if __name__ == '__main__':
    main()
