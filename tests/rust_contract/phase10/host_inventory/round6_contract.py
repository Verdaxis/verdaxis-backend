"""Real parser saturation and priority invariants; never contacts hosts."""
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, ROOT, TRANSPORT, load


class Round6Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def test_denial_survives_real_reference_truncation(self):
        with tempfile.TemporaryDirectory(prefix='w0-r6-') as temp:
            path = Path(temp) / 'backup.sh'
            denied = '/usr/local/libexec/runtime.env'
            path.write_text('a=1\nb=2\nc=3\nsource ' + denied + '\n')
            with patch.object(self.reader, 'MAX_ITEMS', 2):
                result = self.reader.inspect_graph([str(path)])
            rows = result['blocking_dependencies']
            self.assertTrue(any(r['status'] == 'credential-denied' and
                                r['reference']['sha256'] == hashlib.sha256(denied.encode()).hexdigest() for r in rows))
            self.assertNotIn(denied, json.dumps(result))
            self.assertIn('references', result['truncations'])

    def test_late_important_rows_replace_ordinary(self):
        for important in ('app.hidden', '/usr/local/libexec/runtime.env'):
            with self.subTest(vector=important):
                def reader(path):
                    return {'path': path, 'uid': 0, 'gid': 0, 'mode': '0700', 'size': 1,
                            'sha256': 'a' * 64, 'read_acquired': True, 'executables': [],
                            'imports': ['x', 'y', 'z'] if path.endswith('a.py') else ([important] if important == 'app.hidden' else []),
                            'credential_references': [important] if path.endswith('b.py') and important != 'app.hidden' else []}
                with patch.object(self.reader, 'MAX_ITEMS', 2):
                    result = self.reader.inspect_graph(['/tmp/a.py', '/tmp/b.py'], script_reader=reader)
                expected = 'removed-package' if important == 'app.hidden' else 'credential-denied'
                self.assertTrue(any(r['status'] == expected for r in result['blocking_dependencies']))
                self.assertEqual(result, self._again(reader))

    def _again(self, reader):
        with patch.object(self.reader, 'MAX_ITEMS', 2):
            return self.reader.inspect_graph(['/tmp/a.py', '/tmp/b.py'], script_reader=reader)

    def test_class_saturation_has_indelible_marker(self):
        self.assertTrue(hasattr(self.reader, 'PriorityRows'), 'priority retention absent')
        for status in ('credential-denied', 'removed-package'):
            with self.subTest(vector=status):
                rows = self.reader.PriorityRows(2)
                for i in range(5):
                    rows.add({'source': self.reader.unresolved('s'), 'reference': self.reader.unresolved(str(i)), 'status': status})
                for i in range(5):
                    rows.add({'source': self.reader.unresolved('s'), 'reference': self.reader.unresolved(str(i)), 'status': 'unresolved'})
                self.assertTrue(any(r['status'] == 'overflow:' + status for r in rows.values()))
                self.assertTrue(rows.dropped)

    def test_actual_bound_branches(self):
        for branch in ('statements', 'logical-lines', 'references', 'blocking-rows', 'import-queue', 'graph-bytes', 'cron-count', 'unit-count', 'entries', 'structural-items'):
            with self.subTest(vector=branch):
                with tempfile.TemporaryDirectory(prefix='w0-r6-') as temp:
                    script = Path(temp) / 'backup.sh'
                    script.write_text('a=1\nb=2\nc=3\n')
                    def command(argv):
                        if 'list-unit-files' in argv:
                            return ('a.service b.service c.service\n' if branch == 'unit-count' else 'a.service\n'), True
                        if 'cat' in argv:
                            if branch == 'logical-lines': return 'Description="' + 'x' * 80, True
                            if branch in ('entries', 'statements'): return 'ExecStart="' + 'x' * 80, True
                            return 'User=a\nGroup=b\nWorkingDirectory=/c\n', True
                        if 'crontab' in argv: return 'A=1\nB=2\nC=3\n', True
                        if argv[0] == 'find': return str(script) + '\n', True
                        return '', True
                    cron = []
                    for i in range(3):
                        p = Path(temp) / (str(i) + '.cron'); p.write_text(''); cron.append(p)
                    with patch.object(self.reader, 'MAX_ITEMS', 2), patch.object(self.reader, 'MAX_SCRIPT_BYTES', 32 if branch == 'statements' else 262144), patch.object(self.reader, 'MAX_OUTPUT_BYTES', 32 if branch == 'logical-lines' else 2097152):
                        if branch in ('references', 'blocking-rows', 'import-queue', 'graph-bytes'):
                            def record(path):
                                return {'path': path, 'uid': 0, 'gid': 0, 'mode': '0700', 'size': 1, 'sha256': 'a'*64, 'imports': ['x','y','z'], 'credential_references': [], 'executables': [], 'read_acquired': True}
                            if branch == 'references':
                                result = self.reader.inspect_graph([str(script)])
                            else:
                                with patch.object(self.reader, 'MAX_TOTAL_BYTES', 1 if branch == 'graph-bytes' else 7340032):
                                    result = self.reader.inspect_graph(['/tmp/a.py','/tmp/b.py','/tmp/c.py'] if branch == 'import-queue' else ['/tmp/a.py'], script_reader=record)
                            reasons = result['truncations']
                        else:
                            result = self.reader.collect(command, cron_files=cron if branch == 'cron-count' else [])
                            reasons = [r for item in result['truncations'] for r in item['reasons']]
                        self.assertIn(branch, reasons)
                        self.assertEqual(result['dependency_coverage'], 'manual-review-required')
                        self.assertTrue(any(r['status'].startswith('bound-exceeded:') for r in result['blocking_dependencies']))

    def test_hostile_refusals_are_normalized(self):
        transport = load(TRANSPORT)
        for value in ({'refusal':'os-error','extra':'private'}, {'refusal':'private'}, {'refusal':[]}, {'refusal':{}}, {'refusal':42}):
            for success in (True, False):
                with self.subTest(vector=value, success=success):
                    try:
                        result = transport.read_host('staging', lambda argv, data: (json.dumps(value).encode(), success))
                    except Exception as error:
                        self.fail('hostile refusal must be normalized: ' + type(error).__name__)
                    self.assertEqual(result, {'refusal':'transport-error'})

    def test_late_flags_and_schema(self):
        def command(argv):
            return ('a.service b.service c.service\n', True) if 'list-unit-files' in argv else ('', True)
        with patch.object(self.reader, 'MAX_ITEMS', 2):
            result = self.reader.collect(command, cron_files=[])
        self.assertFalse(result['acquired']['units'])
        self.assertTrue(all(not o['acquired'] for o in result['observations'] if o['surface']=='units'))
        transport = load(TRANSPORT)
        base = json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
        document = transport.collect_hosts(base, lambda argv,data: (json.dumps(result).encode(), True))
        from jsonschema import Draft202012Validator
        Draft202012Validator(json.loads((ROOT/'tests/rust_contract/phase10/host-inventory.schema.json').read_text())).validate(document)


if __name__ == '__main__':
    unittest.main(verbosity=2)
