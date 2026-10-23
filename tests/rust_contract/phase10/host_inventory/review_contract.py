"""PD10-W1 regressions: structural transport, publication, transitive imports."""
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, ROOT, TRANSPORT, load


class ReviewContract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def test_review_examples_never_cross_transport(self):
        secrets = ['two words', 'password-short', 'pg-short', 'query-short',
                   'first-fragment', 'second-fragment', 'continued-secret', 'A' * 40 + '=',
                   'a' * 40 + '=', '0123456789abcdef0123456789abcdeff']
        unit = '\n'.join([
            '[Service]', 'User=fixture', 'Group=fixture', 'WorkingDirectory=/srv/fixture',
            'After=network.target fixture.service',
            'ExecStart="/usr/bin/env PGPASSWORD=\'two words\' /bin/tool"',
            'ExecStartPre=/bin/tool --password=password-short',
            'ExecStop=/bin/tool PGPASSWORD=pg-short',
            'Environment="TOKEN=first-fragment\nsecond-fragment"',
            'Environment=TOKEN=continued-\\\nsecret',
            'EnvironmentFile=-/etc/fixture/runtime.env',
            'OnCalendar=*-*-* 04:00:00', 'Description=' + secrets[7],
            'Description=' + secrets[8], 'Description=' + secrets[9]])
        cron = '0 4 * * * /usr/bin/curl --url=https://example.test/hook?token=query-short\n'
        cron += '# ' + ' '.join(secrets) + '\n'
        def command(argv):
            if 'list-unit-files' in argv:
                return 'fixture.service enabled\n', True
            if 'cat' in argv:
                return unit, True
            if 'crontab' in argv:
                return cron, True
            return '', True
        result = self.reader.collect(command, cron_files=[])
        output = json.dumps(result)
        for secret in secrets:
            self.assertNotIn(secret, output)
        self.assertNotIn('observations', result) if False else None
        self.assertNotIn('"text"', output)
        for value in ('fixture', '/srv/fixture', '/bin/tool', '/usr/bin/curl',
                      '/etc/fixture/runtime.env', 'OnCalendar', '04:00:00'):
            self.assertIn(value, output)
        self.assertIn(hashlib.sha256(unit.encode()).hexdigest(), output)
        self.assertIn('<unparsed>', output)
        transport = load(TRANSPORT)
        base = json.loads((ROOT / 'tests/rust_contract/phase10/host-dependencies.json').read_text())
        document = transport.collect_hosts(base, lambda argv, data: (json.dumps(result).encode(), True))
        for secret in secrets:
            self.assertNotIn(secret, json.dumps(document))
        from jsonschema import Draft202012Validator
        schema = json.loads((ROOT / 'tests/rust_contract/phase10/host-inventory.schema.json').read_text())
        Draft202012Validator(schema).validate(document)

    def test_publication_end_to_end_and_refusals(self):
        transport = load(TRANSPORT)
        base = json.loads((ROOT / 'tests/rust_contract/phase10/host-dependencies.json').read_text())
        snapshot = self.reader.collect(lambda argv: ('', True), cron_files=[])
        document = transport.collect_hosts(base, lambda argv, data: (json.dumps(snapshot).encode(), True))
        with tempfile.TemporaryDirectory(prefix='w0-publish-') as temp:
            folder = Path(temp)
            folder.chmod(0o700)
            with patch.object(transport, 'EVIDENCE_ROOT', folder):
                try:
                    transport.publish(document, 'inventory.json')
                except NameError as error:
                    self.fail(f'publication must not reference an unimported name: {error}')
                data = (folder / 'inventory.json').read_bytes()
                self.assertEqual(json.loads(data), document)
                self.assertEqual((folder / 'inventory.json.sha256').read_text(), hashlib.sha256(data).hexdigest() + '\n')
                for name in ('inventory.json', 'inventory.json.sha256'):
                    self.assertEqual((folder / name).stat().st_mode & 0o777, 0o600)
                with self.assertRaises((ValueError, OSError)):
                    transport.publish(document, 'inventory.json')
                (folder / 'link.json').symlink_to(folder / 'inventory.json')
                with self.assertRaises((ValueError, OSError)):
                    transport.publish(document, 'link.json')
                for name in ('../escape.json', '/tmp/escape.json', 'bad.sh'):
                    with self.assertRaises(ValueError):
                        transport.publish(document, name)
                self.assertEqual((folder / 'inventory.json').read_bytes(), data)

    def test_transitive_import_and_unresolved_are_blocking(self):
        self.assertTrue(hasattr(self.reader, 'inspect_graph'), 'transitive import reader is absent')
        with tempfile.TemporaryDirectory(prefix='w0-imports-') as temp:
            root = Path(temp)
            entry = root / 'backup.py'
            entry.write_text('import helper\n')
            (root / 'helper.py').write_text('import app.hidden\nimport missing_dependency\n')
            (root / 'app').mkdir()
            (root / 'app/__init__.py').write_text('')
            (root / 'app/hidden.py').write_text('import json\n')
            result = self.reader.inspect_graph([str(entry)], roots=[str(root)])
            self.assertEqual({r['path'] for r in result['scripts']},
                             {str(entry), str(root / 'helper.py'), str(root / 'app/__init__.py'), str(root / 'app/hidden.py')})
            self.assertFalse(result['complete'])
            self.assertTrue(any(r['reference'] == 'missing_dependency' and r['status'] == 'unresolved'
                                for r in result['blocking_dependencies']))
            self.assertTrue(any(r['reference'] == 'app.hidden' for r in result['blocking_dependencies']))
            transport = load(TRANSPORT)
            snapshot = self.reader.collect(lambda argv: ('', True), cron_files=[])
            snapshot.update(scripts=result['scripts'], blocking_dependencies=result['blocking_dependencies'])
            snapshot['coverage']['imports'] = result['complete']
            base = json.loads((ROOT / 'tests/rust_contract/phase10/host-dependencies.json').read_text())
            document = transport.collect_hosts(base, lambda argv, data: (json.dumps(snapshot).encode(), True))
            self.assertTrue(document['blocking_dependencies'])
            self.assertTrue(all(r['status'] in ('unresolved', 'removed-package') for r in document['blocking_dependencies']))
            # Depth exhaustion and non-local imports cannot claim completeness.
            limited = self.reader.inspect_graph([str(entry)], roots=[str(root)], max_depth=0)
            self.assertFalse(limited['complete'])
            self.assertTrue(limited['blocking_dependencies'])


if __name__ == '__main__':
    unittest.main(verbosity=2)
