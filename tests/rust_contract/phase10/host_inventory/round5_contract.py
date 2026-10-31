"""Bound saturation is partial evidence, never host-text diagnostics."""
import json
import unittest
from unittest.mock import patch

from reader_contract import READER, TRANSPORT, load


class Round5Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)

    def test_surface_bounds_continue(self):
        from round10_contract import Round10Contract
        case=Round10Contract('test_surface_retention_matrix');case.setUp();case.test_surface_retention_matrix()

    def test_graph_bounds_keep_prefix(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_collection_graph_bound_matrix');case.setUp();case.test_collection_graph_bound_matrix()

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
        from round10_contract import Round10Contract
        case=Round10Contract('test_surface_retention_matrix');case.setUp();case.test_surface_retention_matrix()

    def test_total_output_refuses_with_code(self):
        self.assertTrue(hasattr(self.reader, 'encode_result'), 'bounded encoder absent')
        with patch.object(self.reader, 'MAX_TOTAL_BYTES', 8):
            result = self.reader.encode_result({'private': 'host-private'})
        self.assertEqual(json.loads(result), {'refusal': 'output-bytes'})


if __name__ == '__main__':
    unittest.main(verbosity=2)
