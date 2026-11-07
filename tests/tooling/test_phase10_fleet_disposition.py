"""Task 25 consumes GREEN W0 contracts; it never promotes fixture scope to live acceptance."""
import ast
import copy
import hashlib
import json
import sys
from argparse import Namespace
from pathlib import Path

import pytest

from tests.tooling.compatibility import historical_text

ROOT = Path(__file__).resolve().parents[2]
DIRECTORY = ROOT / 'tests/tooling/phase10'
sys.path.insert(0, str(DIRECTORY / 'host_inventory'))
import inventory

REGISTER = DIRECTORY / 'host-dependencies.json'
RECORD = DIRECTORY / 'fleet-disposition.json'
TOOL = 'scripts/scrape_fleet_demand.py'
PROBE_DIGEST = 'f5affd0d5a720207b15874ea9fd967043abf74bc34bd4f64d682eb7f4cfccf14'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_record():
    assert RECORD.is_file(), 'Task 25 final disposition/retained-tool record is missing'
    return json.loads(RECORD.read_text())


def check_evidence(record):
    assert record['probe']['sha256'] == PROBE_DIGEST
    assert record['probe']['collected_at'] == '2026-11-05T09:49Z'
    assert record['probe']['path'] == 'outputs/research/rust-phase10/t36/fleet-probe.txt'
    hosts = record['host_evidence']
    assert set(hosts) == {'production', 'staging'}
    for host in hosts.values():
        assert host == {'cron_references': False, 'unit_references': False,
                        'timer_references': False, 'agent_browser_installed': False}
    assert record['workstation_evidence'] == {
        'owner': 'PM/founder', 'platform': 'macOS', 'manual': True,
        'crontab_references': False, 'launch_agent_references': False,
        'launch_daemon_references': False,
        'browser_path': '/opt/homebrew/bin/agent-browser',
    }
    assert record['excluded_shared_host_product'] == 'OpenClaw'


def check_record(record):
    inventory.validate(inventory.parse(inventory.read_bytes(REGISTER)))
    assert record['schema_version'] == 1 and record['task'] == 25
    assert record['w0']['path'] == 'tests/rust_contract/phase10/host-dependencies.json'
    assert record['w0']['sha256'] == digest(REGISTER)
    assert record['w0']['scope'] == 'collected-manual'
    assert record['w0']['installed_acceptance'] is False
    assert record['w0']['fixture_source_sha256'] == digest(DIRECTORY / 'host_inventory/observations.json')
    assert record['owner'] == 'PM/founder workstation'
    assert record['location'] == "founder's macOS workstation"
    assert record['disposition'] == 'off-host, not installed, not ported'
    assert record['tool']['path'] == TOOL
    assert record['tool']['sha256'] == digest(ROOT / TOOL)
    assert record['tool']['removed_package_dependencies'] == []
    assert record['tool']['retained_in_repository_after_w5'] is True
    assert record['tool']['release_artifact'] is False
    assert record['tool']['installed_on_hosts'] is False
    assert record['tool']['source_path_note'].strip()
    assert record['tool']['dependency_proof'].strip()
    assert record['tool']['imports'] == ['datetime', 'json', 'pathlib', 're', 'subprocess']
    assert record['publication'] == {
        'interface': 'reviewed commit of data/fleet_demand.json',
        'pull_request': 159, 'staging_merge': '5d609212d',
        'reader': 'rs/read/src/market/fleet_snapshot.rs',
        'local_artifact_available': False,
    }
    check_evidence(record)


@pytest.fixture
def green_w0(tmp_path):
    # Exercise the predecessor's real collector and validator without editing it.
    tmp_path.chmod(0o700)
    inventory.collect(Namespace(fixture=DIRECTORY / 'host_inventory/observations.json',
                                evidence_dir=tmp_path, output_name='w0.json'))
    inventory.validate(inventory.parse(inventory.read_bytes(tmp_path / 'w0.json')))
    register = inventory.parse(inventory.read_bytes(REGISTER))
    inventory.validate(register)
    return register


def test_fleet_offhost_is_not_installed_or_ported(green_w0):
    assert not any(row['kind'] == 'fleet' for row in green_w0['dependencies'])
    assert not any('fleet' in row['id'] for row in green_w0['schedules'])
    assert not (ROOT / 'rs/read/src/cli/ops/fleet.rs').exists()
    assert not (ROOT / 'rs/read/tests/phase10_fleet.rs').exists()


def test_fleet_disposition_requires_installed_evidence(green_w0):
    # These independent negative cases must not damage the GREEN predecessor.
    record = {
        'probe': {'path': 'outputs/research/rust-phase10/t36/fleet-probe.txt',
                  'sha256': PROBE_DIGEST, 'collected_at': '2026-11-05T09:49Z'},
        'host_evidence': {host: {'cron_references': False, 'unit_references': False,
                              'timer_references': False, 'agent_browser_installed': False}
                          for host in ('production', 'staging')},
        'workstation_evidence': {'owner': 'PM/founder', 'platform': 'macOS', 'manual': True,
                                'crontab_references': False, 'launch_agent_references': False,
                                'launch_daemon_references': False,
                                'browser_path': '/opt/homebrew/bin/agent-browser'},
        'excluded_shared_host_product': 'OpenClaw',
    }
    check_evidence(record)
    for mutation in ('digest', 'missing-host', 'installed', 'scheduled', 'workstation-scheduled'):
        bad = copy.deepcopy(record)
        if mutation == 'digest':
            bad['probe']['sha256'] = '0' * 64
        elif mutation == 'missing-host':
            del bad['host_evidence']['staging']
        elif mutation == 'installed':
            bad['host_evidence']['production']['agent_browser_installed'] = True
        elif mutation == 'scheduled':
            bad['host_evidence']['staging']['timer_references'] = True
        else:
            bad['workstation_evidence']['crontab_references'] = True
        with pytest.raises(AssertionError):
            check_evidence(bad)
    inventory.validate(green_w0)


def test_fleet_disposition_record_is_complete_and_bound_to_green_w0(green_w0):
    record = read_record()
    check_record(record)
    for mutation in ('owner', 'location', 'w0', 'tool', 'dependency'):
        bad = copy.deepcopy(record)
        if mutation in ('owner', 'location'):
            bad[mutation] = ''
        elif mutation in ('w0', 'tool'):
            bad[mutation]['sha256'] = '0' * 64
        else:
            bad['tool']['removed_package_dependencies'] = ['app']
        with pytest.raises(AssertionError):
            check_record(bad)
    for filename, key in [('removal-ledger.json', 'artifacts'), ('scripts.json', 'artifacts')]:
        document = json.loads(historical_text(f"tests/rust_contract/phase10/{filename}"))
        row = next(row for row in document[key] if row['id'] == 'script.scrape_fleet_demand.py')
        assert row['status'] == 'accepted' and row['end_state'] == 'retired-offhost'
        assert row['owner'] == record['owner']
        assert row['offhost_location'] == record['location']
        assert row['source_sha256'] == record['tool']['sha256']
        assert 'fleet_disposition_record_is_complete_and_bound_to_green_w0' in row['evidence']


def test_fleet_retained_tool_has_no_deleted_package_dependency(green_w0):
    tree = ast.parse((ROOT / TOOL).read_text())
    imports = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.update(alias.name.split('.')[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            assert node.level == 0
            imports.add(node.module.split('.')[0])
    assert imports == {'json', 're', 'subprocess', 'datetime', 'pathlib'}
    assert not imports & {'app', 'scripts', 'alembic'}
    assert not TOOL.startswith('deploy/')
