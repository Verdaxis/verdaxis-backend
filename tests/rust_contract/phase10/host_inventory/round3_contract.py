"""PD10-W2: unresolved identities, strict commands and uncertain roots."""
import json
import tempfile
import unittest
from pathlib import Path

from reader_contract import READER, load


class Round3Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def remote(self, invocation, surface):
        def command(argv):
            if 'list-unit-files' in argv:
                return 'fixture.service enabled\n', True
            if 'cat' in argv and surface == 'unit':
                return 'ExecStart=' + invocation + '\n', True
            if 'crontab' in argv and surface == 'cron':
                return '0 4 * * * ' + invocation + '\n', True
            return '', True
        return self.reader.collect(command, cron_files=[])

    def test_missing_entry_has_no_structural_source(self):
        secret = '0123456789abcdef' + '0123456789abcde'
        self.assertEqual(len(secret), 31)
        for surface in ('unit', 'cron', 'script'):
            with self.subTest(vector='missing-entry', surface=surface):
                invocation = '/usr/bin/python3 /missing-fixture/' + secret + '.py'
                if surface == 'script':
                    with tempfile.TemporaryDirectory(prefix='w0-r3-') as temp:
                        entry = Path(temp) / 'backup.sh'
                        entry.write_text(invocation + '\n')
                        result = self.reader.inspect_graph([str(entry)], roots=[temp])
                else:
                    result = self.remote(invocation, surface)
                self.assertNotIn(secret, json.dumps(result))
                self.assertIn('<unresolved>', json.dumps(result))

    def test_non_simple_commands_always_block(self):
        tails = [('semicolon', ' ; '), ('and', ' && '), ('or', ' || '),
                 ('pipe', ' | '), ('background', ' & '), ('newline', '\n'),
                 ('redirect', ' > '), ('substitution', ' $('), ('backtick', ' `'),
                 ('parameter', ' ${'), ('glob', ' * '), ('extra', ' extra ')]
        for name, operator in tails:
            for surface in ('cron', 'script'):
                with self.subTest(vector=name, surface=surface):
                    with tempfile.TemporaryDirectory(prefix='w0-r3-') as temp:
                        safe = Path(temp) / 'safe.py'
                        safe.write_text('')
                        invocation = '/usr/bin/python3 ' + str(safe) + operator + '/usr/bin/python3 /missing/required.py'
                        if surface == 'script':
                            entry = Path(temp) / 'backup.sh'
                            entry.write_text(invocation + '\n')
                            result = self.reader.inspect_graph([str(entry)], roots=[temp])
                            complete = result['complete']
                        else:
                            result = self.remote(invocation, surface)
                            complete = result['coverage']['imports']
                        self.assertFalse(complete, name + '/' + surface + ' falsely complete')
                        self.assertTrue(result['blocking_dependencies'])
                        self.assertNotIn('/missing/required.py', json.dumps(result))
        for surface in ('cron', 'script'):
            with self.subTest(vector='assignment-substitution', surface=surface):
                with tempfile.TemporaryDirectory(prefix='w0-r3-') as temp:
                    invocation = 'JOB=$(/usr/bin/python3 /missing/required.py)'
                    if surface == 'script':
                        entry = Path(temp) / 'backup.sh'
                        entry.write_text(invocation + '\n')
                        result = self.reader.inspect_graph([str(entry)], roots=[temp])
                        complete = result['complete']
                    else:
                        result = self.remote(invocation, surface)
                        complete = result['coverage']['imports']
                    self.assertFalse(complete)
                    self.assertNotIn('/missing/required.py', json.dumps(result))

    def test_nested_package_candidates_never_guess_order(self):
        with tempfile.TemporaryDirectory(prefix='w0-r3-') as temp:
            root = Path(temp)
            (root / 'tools').mkdir()
            (root / 'backup.py').write_text('import tools.worker\n')
            (root / 'tools/__init__.py').write_text('')
            (root / 'tools/worker.py').write_text('import json\n')
            (root / 'tools/json.py').write_text('')
            (root / 'json.py').write_text('import app.hidden\n')
            result = self.reader.inspect_graph([str(root / 'backup.py')], roots=[temp])
            self.assertFalse(result['complete'], 'nested candidate order falsely complete')
            self.assertTrue(result['blocking_dependencies'])


if __name__ == '__main__':
    unittest.main(verbosity=2)
