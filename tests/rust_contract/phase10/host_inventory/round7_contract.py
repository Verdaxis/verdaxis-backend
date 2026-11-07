"""Byte admission must not erase already parsed priority evidence."""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, ROOT, TRANSPORT, load


def record(path, **changes):
    row = dict(path=path, uid=0, gid=0, mode='0700', size=1, sha256='a'*64,
               imports=[], executables=[], credential_references=[], read_acquired=True)
    row.update(changes)
    return row


class Round7Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)


    def test_late_priority_before_graph_bytes(self):
        from round11_contract import Round11Contract
        case=Round11Contract('test_acquired_overflow_survives_byte_rejection');case.setUp();case.test_acquired_overflow_survives_byte_rejection()

    def test_marker_canonicalization(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_real_parser_dual_class_overflow_installed');case.setUp();case.test_real_parser_dual_class_overflow_installed()

    def test_queue_slice_actual(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_queue_slice_distinguishes_visited_guard');case.setUp();case.test_queue_slice_distinguishes_visited_guard()

    def test_cron_enumeration_actual(self):
        from round10_contract import Round10Contract
        case=Round10Contract('test_surface_retention_matrix');case.setUp();case.test_surface_retention_matrix()

    def test_real_command_byte_drain(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_command_prefix_and_collection');case.setUp();case.test_command_prefix_and_collection()

    def test_observation_and_encoded_output_distinct(self):
        with patch.object(self.reader,'MAX_TOTAL_BYTES',1):
            try:
                self.reader.collect(lambda argv:('private-host-text',True),cron_files=[])
            except ValueError as error:
                result=self.reader.refusal(error)
            else: self.fail('observation byte branch not reached')
        self.assertEqual(result,{'refusal':'output-bytes'})
        with patch.object(self.reader,'MAX_TOTAL_BYTES',1):
            self.assertEqual(json.loads(self.reader.encode_result({'secret':'private-host-text'})),{'refusal':'output-bytes'})

    def test_transport_bounds(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_transport_and_publisher_downstream');case.setUp();case.test_transport_and_publisher_downstream()


    def test_deadline_and_wait_limits(self):
        from round8_contract import Round8Contract
        case=Round8Contract('test_deadline_wait_downstream');case.setUp();case.test_deadline_wait_downstream()
        from round8_contract import Round8Contract
        case=Round8Contract('test_transport_deadline_wait_downstream');case.setUp();case.test_transport_deadline_wait_downstream()

    def test_file_bounds_and_change_after_stat(self):
        from round9_contract import Round9Contract
        case=Round9Contract('test_changed_and_overflow_files_collection');case.setUp();case.test_changed_and_overflow_files_collection()

    def test_depth_limit_separate_from_visited_limit(self):
        from round9_contract import Round9Contract
        case=Round9Contract('test_readable_absolute_child_depth');case.setUp();case.test_readable_absolute_child_depth()

if __name__=='__main__': unittest.main(verbosity=2)
