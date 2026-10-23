"""Bound saturation is partial evidence, never host-text diagnostics."""
import json
import unittest
from unittest.mock import patch

from reader_contract import READER, TRANSPORT, load


class Round5Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def test_surface_bounds_continue(self):
        for surface, text in [('units', 'a.service b.service c.service\n'),
                              ('dropins', 'User=a\nGroup=b\nWorkingDirectory=/c\n'),
                              ('cron', 'JOB=host-private-one\nJOB=host-private-two\nJOB=host-private-three\n'),
                              ('usr_local', '/tmp/a.py\n/tmp/b.py\n/tmp/c.py\n')]:
            with self.subTest(vector=surface):
                def command(argv):
                    if 'list-unit-files' in argv:
                        return (text if surface == 'units' else 'a.service\n'), True
                    if 'cat' in argv and surface == 'dropins' or 'crontab' in argv and surface == 'cron' or argv[0] == 'find' and surface == 'usr_local':
                        return text, True
                    return '', True
                with patch.object(self.reader, 'MAX_ITEMS', 2):
                    try:
                        result = self.reader.collect(command, cron_files=[])
                    except ValueError:
                        self.fail(surface + ' bound must retain partial inventory')
                self.assertFalse(result['acquired'][surface])
                self.assertTrue(result.get('truncations'))
                self.assertTrue(any(r['status'].startswith('bound-exceeded:') for r in result['blocking_dependencies']))
                self.assertNotIn('host-private', json.dumps(result))
                self.assertEqual(result['dependency_coverage'], 'manual-review-required')

    def test_graph_bounds_keep_prefix(self):
        def record(path):
            return {'path': path, 'uid': 0, 'gid': 0, 'mode': '0700', 'size': 1,
                    'sha256': 'a' * 64, 'imports': ['private-one', 'private-two', 'private-three'],
                    'executables': [], 'credential_references': [], 'read_acquired': True}
        for count in (1, 3):
            with self.subTest(vector='graph-' + str(count)):
                with patch.object(self.reader, 'MAX_ITEMS', 2):
                    try:
                        result = self.reader.inspect_graph(['/tmp/' + str(i) + '.py' for i in range(count)], script_reader=record)
                    except ValueError:
                        self.fail('graph bound must retain prefix')
                self.assertTrue(result.get('truncations'))
                self.assertLessEqual(len(result['scripts']), 2)
                self.assertNotIn('private-one', json.dumps(result))

    def test_fixed_refusal_vocabulary_and_transport(self):
        self.assertTrue(hasattr(self.reader, 'refusal'), 'structural refusal API absent')
        for error, code in [(OSError('host-private'), 'os-error'),
                            (ValueError('host-private'), 'value-error'),
                            (RecursionError('host-private'), 'recursion')]:
            with self.subTest(vector=code):
                result = self.reader.refusal(error)
                self.assertEqual(result, {'refusal': code})
                self.assertNotIn('host-private', json.dumps(result))
                transport = load(TRANSPORT)
                result = transport.read_host('staging', lambda argv, data: (json.dumps(result).encode(), False))
                self.assertEqual(result, {'refusal': code})

    def test_each_bound_reason_is_structural(self):
        codes = ('statements', 'logical-lines', 'references', 'blocking-rows',
                 'import-queue', 'graph-bytes')
        for code in codes:
            with self.subTest(vector=code):
                def reader(path):
                    raise self.reader.SurfaceBound(code)
                result = self.reader.inspect_graph(['/tmp/fixture.py'], script_reader=reader)
                self.assertEqual(result['truncations'], [code])
                self.assertTrue(any(row['status'] == 'bound-exceeded:' + code
                                    for row in result['blocking_dependencies']))
        for surface, argv_name in [('units', 'list-unit-files'), ('cron', 'crontab')]:
            with self.subTest(vector='command-bytes-' + surface):
                def command(argv):
                    return ('host-private', False) if argv_name in argv else ('', True)
                result = self.reader.collect(command, cron_files=[])
                self.assertFalse(result['acquired'][surface])
                self.assertTrue(any(row['status'] == 'bound-exceeded:' + surface
                                    for row in result['blocking_dependencies']))
                self.assertNotIn('host-private', json.dumps(result))

    def test_total_output_refuses_with_code(self):
        self.assertTrue(hasattr(self.reader, 'encode_result'), 'bounded encoder absent')
        with patch.object(self.reader, 'MAX_TOTAL_BYTES', 8):
            result = self.reader.encode_result({'private': 'host-private'})
        self.assertEqual(json.loads(result), {'refusal': 'output-bytes'})


if __name__ == '__main__':
    unittest.main(verbosity=2)
