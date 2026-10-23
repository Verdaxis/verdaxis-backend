"""PD10-W3: acquisition is not completeness; deny reads before opening."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, load


class Round4Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def test_assignments_and_all_exec_keys_block(self):
        for line in ('JOB=ok /usr/bin/python3 /missing/required.py',
                     'JOB=ok; /usr/bin/python3 /missing/required.py', 'JOB=ok'):
            with self.subTest(vector=line):
                with tempfile.TemporaryDirectory(prefix='w0-r4-') as temp:
                    path = Path(temp) / 'backup.sh'
                    path.write_text(line + '\n')
                    # Fixture file allowlist is substituted only after the feature exists.
                    context = patch.object(self.reader, 'FILE_ROOTS', (temp,)) if hasattr(self.reader, 'FILE_ROOTS') else patch.dict({}, {})
                    with context:
                        result = self.reader.inspect_graph([str(path)])
                    self.assertTrue(result['blocking_dependencies'])
                    self.assertNotIn('/missing/required.py', json.dumps(result))
        for key in ('ExecStartPost', 'ExecCondition', 'ExecReload', 'ExecStopPost', 'ExecAnything', 'EnvironmentFile'):
            with self.subTest(vector=key):
                def command(argv):
                    if 'list-unit-files' in argv:
                        return 'fixture.service enabled\n', True
                    if 'cat' in argv:
                        return key + '=/missing/required.py\n', True
                    return '', True
                result = self.reader.collect(command, cron_files=[])
                self.assertTrue(result['blocking_dependencies'])
                self.assertNotIn('/missing/required.py', json.dumps(result))

    def test_credentials_and_outside_roots_denied_before_open(self):
        for path in ('/home/fixture/.pgpass', '/etc/shadow', '/etc/gshadow',
                     '/usr/local/bin/.ssh/key', '/usr/local/bin/runtime.env',
                     '/usr/local/bin/file.pem', '/usr/local/bin/file.key',
                     '/usr/local/bin/.netrc', '/usr/local/bin/.git-credentials',
                     '/usr/local/bin/.config/verdaxis/private', '/tmp/outside.py'):
            for mode in ('direct', 'source', 'interpreter'):
                with self.subTest(vector=path, surface=mode):
                    attempted = []
                    def forbidden_open(*args, **kwargs):
                        attempted.append(args)
                        raise PermissionError('fixture refuses open')
                    with patch.object(self.reader.os, 'open', forbidden_open):
                        if mode == 'direct':
                            with self.assertRaises((ValueError, OSError)):
                                self.reader.read_file(path)
                        else:
                            operand = 'source ' + path if mode == 'source' else '/usr/bin/python3 ' + path
                            candidate, certain = self.reader.script_operand(operand)
                            if candidate:
                                self.reader.inspect_graph([candidate])
                    self.assertEqual(attempted, [], 'denial must precede all opens')

    def test_dependency_coverage_is_always_manual(self):
        for text in ('', '# comment\n', 'JOB=ok\n'):
            with self.subTest(vector=text):
                result = self.reader.collect(lambda argv: (text, True), cron_files=[])
                self.assertEqual(result.get('dependency_coverage'), 'manual-review-required')
                self.assertNotIn('imports', result.get('coverage', {}))
                self.assertNotIn('"complete": true', json.dumps(result))


if __name__ == '__main__':
    unittest.main(verbosity=2)
