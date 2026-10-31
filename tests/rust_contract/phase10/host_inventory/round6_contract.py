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
        from round8_contract import Round8Contract
        case=Round8Contract('test_real_parser_dual_class_overflow_installed');case.setUp();case.test_real_parser_dual_class_overflow_installed()

    def test_late_important_rows_replace_ordinary(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_collection_graph_bound_matrix');case.setUp();case.test_collection_graph_bound_matrix()

    def _again(self, reader):
        with patch.object(self.reader, 'MAX_ITEMS', 2):
            return self.reader.inspect_graph(['/tmp/a.py', '/tmp/b.py'], script_reader=reader)

    def test_class_saturation_has_indelible_marker(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_real_parser_dual_class_overflow_installed');case.setUp();case.test_real_parser_dual_class_overflow_installed()

    def test_actual_bound_branches(self):
        from round10_contract import Round10Contract
        case=Round10Contract('test_surface_retention_matrix');case.setUp();case.test_surface_retention_matrix()
        from round8_contract import Round8Contract
        case=Round8Contract('test_collection_graph_bound_matrix');case.setUp();case.test_collection_graph_bound_matrix()


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
        from round10_contract import Round10Contract
        case=Round10Contract('test_surface_retention_matrix');case.setUp();case.test_surface_retention_matrix()


if __name__ == '__main__':
    unittest.main(verbosity=2)
