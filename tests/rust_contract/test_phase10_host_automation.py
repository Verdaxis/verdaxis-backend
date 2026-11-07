"""Manual PM classification stays separate from W0 installed acceptance."""
import copy
import sys
from pathlib import Path

import pytest

DIRECTORY = Path(__file__).resolve().parent / 'phase10/host_inventory'
sys.path.insert(0, str(DIRECTORY))
import automation_contract as contract
import inventory


def register():
    return inventory.parse(inventory.read_bytes(contract.REGISTER))


def test_manual_register_validates_without_claiming_installed_acceptance():
    inventory.validate(register())
    assert register()['scope'] == 'collected-manual'


@pytest.mark.parametrize('name', contract.CHECKS)
def test_task36_named_contract(name):
    contract.CHECKS[name](register())


@pytest.mark.parametrize('mutation', ['app-import', 'missing-row', 'schedule', 'acceptance', 'installed', 'secret'])
def test_manual_register_rejects_unsafe_changes(mutation):
    document = copy.deepcopy(register())
    if mutation == 'app-import':
        document['dependencies'][0]['imports'] = ['app.seeds']
    elif mutation == 'missing-row':
        document['dependencies'].pop()
    elif mutation == 'schedule':
        document['schedules'][0]['expression'] = 'daily'
    elif mutation == 'acceptance':
        document['dependencies'][0]['automation']['owner_acceptance']['owner'] = ''
    elif mutation == 'secret':
        document['dependencies'][0]['evidence'] = 'PASSWORD=fixture-value'
    else:
        document['manual_collection']['installed_acceptance'] = True
    with pytest.raises(ValueError):
        inventory.validate(document)


def test_retained_backup_cannot_become_a_verdaxis_python_exception():
    document = register()
    next(row for row in document['dependencies'] if row['id'] == 'backup')['automation']['python_dependency'] = 'verdaxis'
    with pytest.raises(AssertionError):
        contract.required_verdaxis_python_has_no_exception(document)


def test_empty_cron_inventory_cannot_authorize_removal():
    document = register()
    document['manual_collection']['cron_removals'] = ['unrelated nightly job']
    with pytest.raises(AssertionError):
        contract.only_superseded_exact_cron_entries_are_removed(document)
