"""Reader and transport tests. All commands and SSH are replaced with fixtures."""
import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
READER = ROOT / 'deploy/tests/host-inventory-reader.py'
TRANSPORT = Path(__file__).with_name('transport.py')


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem.replace('-', '_'), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ReaderContract(unittest.TestCase):
    def setUp(self):
        self.assertTrue(READER.exists(), 'bounded live reader is not implemented')
        self.reader = load(READER)

    def test_strict_redaction_rules_and_nonsecret_control(self):
        cases = [
            ('Environment=MODE=private value', 'Environment=<redacted>'),
            ('EnvironmentFile=-/etc/private.env', 'EnvironmentFile=<redacted>'),
            ('Environment="TOKEN=quoted value"', 'Environment=<redacted>'),
            ('Environment=ONE=1 \\\n TWO=2', 'Environment=<redacted>'),
        ]
        for key in ('PASS', 'dbPassword', 'SECRET', 'toKEN', 'API_KEY', 'DSN',
                    'DATABASE_URL', 'AUTH', 'CREDENTIAL', 'COOKIE'):
            cases.append((f'{key}="sensitive words" next', f'{key}=<redacted> next'))
        for source, expected in cases:
            with self.subTest(source=source):
                self.assertEqual(self.reader.redact(source), expected)
        for value in ('a' * 32, '0123456789abcdef' * 2, 'AbCd09+/' * 5,
                      'https://user:password@example.test/path'):
            with self.subTest(value=value):
                self.assertNotIn(value, self.reader.redact('value ' + value))
                self.assertIn('<redacted>', self.reader.redact('value ' + value))
        self.assertEqual(self.reader.redact('MODE=production /usr/local/bin/job.sh'),
                         'MODE=production /usr/local/bin/job.sh')
        self.assertEqual(self.reader.redact('https://example.test/public'),
                         'https://example.test/public')
        self.assertEqual(self.reader.redact('TOKEN=<redacted>'), 'TOKEN=<redacted>')
        for source in ('curl --password sensitive', 'Authorization: Bearer sensitive',
                       'PASS=escaped\\ value', 'echo "TOKEN=unterminated sensitive'):
            self.assertNotIn('sensitive', self.reader.redact(source))

    def test_nofollow_bounds_metadata_and_imports(self):
        with tempfile.TemporaryDirectory(prefix='w0-reader-') as temp:
            folder = Path(temp)
            script = folder / 'backup.py'
            data = b'import json\nfrom app.models import User\nPASSWORD="sensitive"\n'
            script.write_bytes(data)
            script.chmod(0o750)
            row = self.reader.read_script(str(script))
            self.assertEqual(row['sha256'], hashlib.sha256(data).hexdigest())
            self.assertEqual(row['mode'], '0750')
            self.assertEqual(row['uid'], os.getuid())
            self.assertEqual(row['size'], len(data))
            self.assertTrue(all(r['kind'] == '<unresolved>' for r in row['imports']))
            self.assertNotIn('sensitive', json.dumps(row))
            link = folder / 'link.py'
            link.symlink_to(script)
            with self.assertRaises((ValueError, OSError)):
                self.reader.read_script(str(link))
            ancestor = folder / 'alias'
            ancestor.symlink_to(folder, target_is_directory=True)
            with self.assertRaises((ValueError, OSError)):
                self.reader.read_script(str(ancestor / 'backup.py'))
            big = folder / 'large.sh'
            big.write_bytes(b'x' * (self.reader.MAX_SCRIPT_BYTES + 1))
            with self.assertRaises(ValueError):
                self.reader.read_script(str(big))
            shell = folder / 'restore.sh'
            shell.write_text('#!/bin/sh\n. /etc/private.env\nsource /usr/local/libexec/helper.sh\n')
            row = self.reader.read_script(str(shell))
            self.assertIn({'kind': 'resolved', 'path': '/etc/private.env'}, row['credential_references'])
            self.assertTrue(all(r['kind'] == '<unresolved>' for r in row['imports']))
            # Import names are recorded, never imported, sourced or followed.
            self.assertNotIn('contents', row)

    def test_credential_denylist_before_open(self):
        for path in ('/tmp/.env', '/tmp/.env.production', '/tmp/credentials/db',
                     '/tmp/secret/token', '/tmp/key.pem', '/tmp/id_ed25519',
                     '/tmp/keys/signing', '/home/a/.config/verdaxis/artifact-provenance/record.json',
                     '/tmp/credentials/../backup.sh', '/tmp/db-password', '/tmp/auth_token',
                     '/usr/local/libexec/runtime.env', '/tmp/runtime.ENV'):

            with self.subTest(path=path):
                with self.assertRaisesRegex(ValueError, 'forbidden'):
                    self.reader.read_script(path)

    def test_fixed_commands_redact_before_output_and_never_execute_scripts(self):
        seen = []
        def command(argv):
            seen.append(tuple(argv))
            if 'list-unit-files' in argv:
                return 'fixture.service enabled\nfixture.timer enabled\n', True
            if 'cat' in argv:
                return 'Environment=TOKEN=fixture-secret\nExecStart=/usr/local/bin/backup.sh\n', True
            if 'find' == argv[0]:
                return '', True
            if 'crontab' in argv:
                return 'PASS=fixture-secret\n0 4 * * * /usr/local/bin/backup.sh\n', True
            return 'fixture output', True
        def script_reader(path):
            return {'path': path, 'uid': 0, 'gid': 0, 'mode': '0750', 'size': 1,
                    'sha256': 'a' * 64, 'imports': [], 'imports_complete': True,
                    'credential_references': []}
        result = self.reader.collect(command, cron_files=[], script_reader=script_reader)
        text = json.dumps(result)
        self.assertNotIn('fixture-secret', text)
        self.assertIn('Environment', text)
        self.assertNotIn('"text"', text)
        self.assertIn('/usr/local/bin', self.reader.COMMANDS['executables'])
        self.assertEqual(self.reader.COMMANDS['processes'], ('ps', '-eo', 'pid=,user=,comm='))
        for argv in seen:
            self.assertTrue(self.reader.allowed(argv), argv)
        self.assertFalse(self.reader.allowed(('bash', '/tmp/backup.sh')))
        self.assertFalse(self.reader.allowed(('systemctl', 'restart', 'fixture.service')))
        transport = load(TRANSPORT)
        base = json.loads((ROOT / 'tests/rust_contract/phase10/host-dependencies.json').read_text())
        calls = []
        def runner(argv, data):
            calls.append(argv)
            return json.dumps(result).encode(), True
        document = transport.collect_hosts(base, runner)
        self.assertEqual(len(calls), 2)
        from jsonschema import Draft202012Validator
        schema = json.loads((ROOT / 'tests/rust_contract/phase10/host-inventory.schema.json').read_text())
        Draft202012Validator(schema).validate(document)
        self.assertEqual(document['scope'], 'installed')
        self.assertEqual(document['binding'], 'unsigned, digest-bound')
        self.assertEqual(document['dependencies'], [])
        self.assertEqual(document['schedules'], [])

    def test_transport_one_fixed_stdin_script_per_host(self):
        self.assertTrue(TRANSPORT.exists(), 'fixed transport is not implemented')
        transport = load(TRANSPORT)
        calls = []
        def runner(argv, data):
            calls.append((argv, data))
            return b'{"coverage":{},"observations":[],"scripts":[],"blocking_dependencies":[]}', True
        for host, target in [('staging', '194.233.68.86'), ('production', 'verdaxis-prod-eu')]:
            transport.read_host(host, runner)
            argv, data = calls[-1]
            self.assertEqual(argv, ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15',
                                    '-o', 'StrictHostKeyChecking=yes', target, 'python3 -B -I -'])
            self.assertEqual(data, READER.read_bytes())
        self.assertEqual(len(calls), 2)
        with self.assertRaises(ValueError):
            transport.read_host('other;touch /tmp/bad', runner)
        self.assertEqual(len(calls), 2)
        self.assertEqual(str(transport.EVIDENCE_ROOT), '/Users/jonathanjie/Documents/workspace/Verdaxis/outputs/research/rust-phase10/w0-evidence')


if __name__ == '__main__':
    unittest.main(verbosity=2)
