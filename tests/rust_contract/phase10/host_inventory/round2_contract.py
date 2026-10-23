"""Each R2 vector runs against serialized remote output, before validation."""
import json
import tempfile
import unittest
from pathlib import Path

from reader_contract import READER, load


class Round2Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def test_script_operands_are_default_deny_on_every_surface(self):
        hex31 = ('0123456789abcdef' + '0123456789abcde')
        self.assertEqual(len(hex31), 31)
        vectors = [('hex31', 'source ' + hex31, hex31),
                   ('url', "source 'https://example.test/hook?url=token=short-value'", 'short-value'),
                   ('multiline', 'source "first-fragment\nsecond-fragment"', 'first-fragment')]
        for name, statement, secret in vectors:
            for surface in ('graph', 'remote'):
                with self.subTest(vector=name, surface=surface):
                    with tempfile.TemporaryDirectory(prefix='w0-r2-') as temp:
                        script = Path(temp) / 'backup.sh'
                        script.write_text('#!/bin/sh\n' + statement + '\n')
                        if surface == 'graph':
                            result = self.reader.inspect_graph([str(script)], roots=[temp])
                        else:
                            def command(argv):
                                return (str(script) + '\n', True) if argv[0] == 'find' else ('', True)
                            result = self.reader.collect(command, cron_files=[])
                        output = json.dumps(result)
                        self.assertNotIn(secret, output, f'{name}/{surface} leaks operand')
                        self.assertNotIn('second-fragment', output)
                        self.assertIn('<unresolved>', output)
                        self.assertIn('sha256', output)
                        self.assertFalse(result.get('complete', result.get('coverage', {}).get('imports')))

    def test_interpreter_and_wrapper_operands_are_inspected(self):
        for name, prefix in [('python', '/usr/bin/python3'), ('env', '/usr/bin/env python3'),
                             ('sh', '/bin/sh'), ('bash', 'bash')]:
            for surface in ('unit', 'cron'):
                with self.subTest(vector=name, surface=surface):
                    with tempfile.TemporaryDirectory(prefix='w0-entry-') as temp:
                        script = Path(temp) / ('backup.sh' if name in {'sh', 'bash'} else 'backup.py')
                        script.write_text('source /not/present.sh\n' if name in {'sh', 'bash'} else 'import app.missing\n')
                        invocation = prefix + ' ' + str(script) + ' --password=never-transport'
                        def command(argv):
                            if 'list-unit-files' in argv:
                                return 'fixture.service enabled\n', True
                            if 'cat' in argv and surface == 'unit':
                                return 'ExecStart=' + invocation + '\n', True
                            if 'crontab' in argv and surface == 'cron':
                                return '0 4 * * * ' + invocation + '\n', True
                            return '', True
                        result = self.reader.collect(command, cron_files=[])
                        self.assertIn(str(script), {r['path'] for r in result['scripts']}, f'{name}/{surface} skips script')
                        self.assertFalse(result['coverage']['imports'])
                        self.assertTrue(result['blocking_dependencies'])
                        self.assertNotIn('never-transport', json.dumps(result))

    def test_local_standard_library_name_is_inspected_first(self):
        with tempfile.TemporaryDirectory(prefix='w0-shadow-') as temp:
            entry = Path(temp) / 'backup.py'
            shadow = Path(temp) / 'json.py'
            entry.write_text('import json\n')
            shadow.write_text('import app.missing\n')
            result = self.reader.inspect_graph([str(entry)], roots=[temp])
            self.assertIn(str(shadow), {r['path'] for r in result['scripts']}, 'stdlib shadow skipped')
            self.assertFalse(result['complete'])
            self.assertTrue(result['blocking_dependencies'])


if __name__ == '__main__':
    unittest.main(verbosity=2)
