"""Synthetic W0 evidence. Never contact a runtime host or open credentials."""
import copy
import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
COLLECT = ROOT / 'deploy/tests/collect-host-inventory.sh'
VERIFY = ROOT / 'deploy/tests/verify-host-inventory.sh'
FIXTURE = Path(__file__).with_name('observations.json')


def run(script, *args):
    return subprocess.run(['bash', str(script), *map(str, args)], cwd=ROOT,
                          capture_output=True, text=True, timeout=30)


class InventoryContract(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='w0-contract-')
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.folder.chmod(0o700)
        self.observations = json.loads(FIXTURE.read_text())

    def collect(self, observations=None):
        source = self.folder / 'observations.json'
        source.write_text(json.dumps(observations or self.observations))
        output = self.folder / 'inventory.json'
        result = run(COLLECT, '--fixture', source, '--evidence-dir', self.folder,
                     '--output-name', output.name)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(run(VERIFY, '--fixture', output).returncode, 0)
        return output

    def refused(self, document, reason):
        path = self.folder / 'invalid.json'
        path.write_text(json.dumps(document))
        # Bind the changed bytes so a semantic refusal cannot be a digest failure.
        path.with_suffix('.json.sha256').write_text(hashlib.sha256(path.read_bytes()).hexdigest() + '\n')
        result = run(VERIFY, '--fixture', path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(reason, result.stderr)

    def test_w0_missing_dependency_is_refused(self):
        if not COLLECT.exists():
            # Tests-only checkpoint: the initial W0 candidate omits staging-pull.
            incomplete = copy.deepcopy(self.observations)
            incomplete['dependencies'] = [d for d in incomplete['dependencies']
                                          if d['id'] != 'staging-pull']
            present = {d['id'] for d in incomplete['dependencies']}
            self.assertIn('staging-pull', present,
                          'incomplete W0 fixture omits required host dependency: staging-pull')
        output = self.collect()
        doc = json.loads(output.read_text())
        doc['dependencies'] = [d for d in doc['dependencies'] if d['id'] != 'staging-pull']
        self.refused(doc, 'required dependency missing')
        doc = json.loads(output.read_text())
        doc['dependencies'][0]['classification'] = 'exception'
        self.refused(doc, 'schema')
        doc = json.loads(output.read_text())
        doc['dependencies'][0]['removed_package_dependencies'] = ['app']
        self.refused(doc, 'unaffected dependency imports removed package')
        # A replacement-required row is explicit blocking work, not a cutover waiver.
        doc = json.loads(output.read_text())
        doc['dependencies'][0]['classification'] = 'replacement-required'
        doc['dependencies'][0]['removed_package_dependencies'] = ['app']
        self.refused(doc, 'NO_GO')
        output.write_text(output.read_text() + ' ')
        self.assertNotEqual(run(VERIFY, '--fixture', output).returncode, 0)

    def test_w0_covers_both_hosts_and_host_owned_automation(self):
        if not COLLECT.exists():
            return
        doc = json.loads(self.collect().read_text())
        self.assertEqual({h['id'] for h in doc['hosts']}, {'production', 'staging'})
        for host in doc['hosts']:
            for field in ('units', 'timers', 'dropins', 'cron', 'processes', 'usr_local', 'packages', 'imports'):
                self.assertTrue(host['coverage'][field])
                bad = copy.deepcopy(doc)
                next(h for h in bad['hosts'] if h['id'] == host['id'])['coverage'][field] = False
                self.refused(bad, 'coverage incomplete')
        for category in ('backup', 'restore', 'staging-pull', 'dump'):
            self.assertTrue(any(d['kind'] == category for d in doc['dependencies']))
        bad = copy.deepcopy(doc)
        bad['hosts'] = bad['hosts'][:1]
        self.refused(bad, 'both hosts')
        bad = copy.deepcopy(doc)
        bad['dependencies'][0]['sha256'] = 'bad'
        self.refused(bad, 'schema')
        bad = copy.deepcopy(doc)
        bad['unexpected'] = 'not allowed'
        self.refused(bad, 'schema')

    def test_fleet_offhost_has_no_removed_package_dependency(self):
        if not COLLECT.exists():
            return
        doc = json.loads(self.collect().read_text())
        fleet = next(d for d in doc['dependencies'] if d['id'] == 'fleet')
        self.assertEqual(fleet['location'], 'offhost')
        self.assertEqual(fleet['removed_package_dependencies'], [])
        for field, value in [('removed_package_dependencies', ['app.models']),
                             ('owner', ''), ('import_evidence', ''), ('path', '')]:
            bad = copy.deepcopy(doc)
            next(d for d in bad['dependencies'] if d['id'] == 'fleet')[field] = value
            self.refused(bad, 'fleet' if field == 'removed_package_dependencies' else 'schema')

    def test_w0_classification_does_not_create_a_schedule(self):
        if not COLLECT.exists():
            return
        before = copy.deepcopy(self.observations)
        output = self.collect()
        doc = json.loads(output.read_text())
        self.assertEqual(doc['schedules'], before['schedules'])
        self.assertEqual(doc['binding'], 'unsigned, digest-bound')
        self.assertEqual(doc['scope'], 'fixture')
        text = output.read_text()
        for secret in ('fixture-password-123', 'fixture-token-456', 'fixture-url-password'):
            self.assertNotIn(secret, text)
        self.assertIn('[REDACTED]', text)
        self.assertEqual(doc['credential_references'], before['credential_references'])
        bad = copy.deepcopy(doc)
        bad['schedules'].append({'id': 'fleet-weekly', 'host': 'offhost', 'expression': 'weekly', 'owner': 'fixture'})
        self.refused(bad, 'schedule set changed')
        bad = copy.deepcopy(doc)
        bad['scope'] = 'installed'
        self.refused(bad, 'fixture scope required')
        # Stage 1 must refuse live collection before opening a host connection.
        result = run(COLLECT, '--live', '--host', 'production')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('stage 1', result.stderr)
        # Never overwrite an existing evidence artifact or follow an output link.
        result = run(COLLECT, '--fixture', FIXTURE, '--evidence-dir', self.folder,
                     '--output-name', output.name)
        self.assertNotEqual(result.returncode, 0)
        link = self.folder / 'link.json'
        link.symlink_to(output)
        self.assertNotEqual(run(COLLECT, '--fixture', FIXTURE, '--evidence-dir', self.folder,
                                '--output-name', link.name).returncode, 0)
        empty = ROOT / 'tests/rust_contract/phase10/host-dependencies.json'
        self.assertNotEqual(run(VERIFY, '--fixture', empty).returncode, 0)


if __name__ == '__main__':
    unittest.main(verbosity=2)
