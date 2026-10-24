"""Distinct entry reason and rejected-record overflow integration witnesses."""
import json
import unittest
from unittest.mock import patch

from collection_assertions import assert_collection, flags, observation, ordered
from reader_contract import READER, load
from round7_contract import record


class Round11Contract(unittest.TestCase):
    def setUp(self):self.reader=load(READER)

    def test_acquired_overflow_survives_byte_rejection(self):
        reader=self.reader
        first=record('/tmp/a.py'); late=record('/tmp/z.py',priority_overflow=['credential-denied'])
        def acquire(path):return first if path.endswith('a.py') else late
        def command(argv):return ('/tmp/a.py\n/tmp/z.py',True) if argv[0]=='find' else ('',True)
        original=reader.inspect_graph
        def graph(*a,**kw):
            with patch.object(reader,'MAX_TOTAL_BYTES',len(json.dumps(first).encode())+1):return original(*a,**kw)
        with patch.object(reader,'inspect_graph',graph):result=reader.collect(command,cron_files=[],script_reader=acquire)
        blocks=ordered(reader,[dict(reader.bound_row('credential-denied'),status='overflow:credential-denied'),reader.bound_row('graph-bytes'),reader.bound_row('usr_local')])
        from round10_contract import unparsed
        observations=[observation('units',''),observation('timers',''),observation('processes',''),
                      observation('usr_local','/tmp/a.py\n/tmp/z.py',[{'key':'executable','values':['/tmp/a.py']}],acquired=False,unparsed=[unparsed(json.dumps({'key':'executable','values':['/tmp/z.py']}))]),
                      observation('packages',''),*[observation('cron','') for _ in range(3)]]
        assert_collection(self,reader,result,scripts=[dict(first,truncations=[])],blocking=blocks,markers={'overflow:credential-denied','bound-exceeded:graph-bytes','bound-exceeded:usr_local'},acquired=flags('usr_local'),observations=observations,truncations=[{'surface':'usr_local','reasons':['graph-bytes']}])


if __name__=='__main__':unittest.main(verbosity=2)
