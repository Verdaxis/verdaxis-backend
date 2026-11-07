"""Critical eviction marker guard and real cross-vector probe rejection."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from boundary_mutations import classify
from reader_contract import READER, load


class Round13Contract(unittest.TestCase):
    def test_eviction_overflow_guard(self):
        reader=load(READER)
        for status in ('credential-denied','removed-package'):
            with self.subTest(status=status):
                rows=reader.PriorityRows(1)
                for value in ('a','b','c'):
                    rows.add({'source':reader.unresolved('source'),'reference':reader.unresolved(value),'status':status})
                result=rows.values()
                expected=[dict(reader.bound_row(status),status='overflow:'+status)]
                facts=[{'source':reader.unresolved('source'),'reference':reader.unresolved(value),'status':status} for value in ('a','b','c')]
                expected.append(min(facts,key=lambda row:json.dumps(row,sort_keys=True)))
                expected.sort(key=lambda row:(reader.row_priority(row),json.dumps(row,sort_keys=True)))
                self.assertEqual(result,expected)
                self.assertEqual(rows.dropped,{'1' if status=='credential-denied' else '2':2})

    def test_priority_class_assignment(self):
        reader=load(READER)
        for status,priority in (('credential-denied',1),('removed-package',2),('bound-exceeded:units',3),('unresolved',4)):
            with self.subTest(status=status):self.assertEqual(reader.row_priority({'status':status}),priority)

    def test_failure_before_site_other_vector_is_invalid(self):
        with tempfile.TemporaryDirectory(prefix='w0-vector-probe-') as folder:
            test=Path(folder)/'synthetic.py';output=Path(folder)/'result.json'
            source='''import unittest
class Synthetic(unittest.TestCase):
    def test_vectors(self):
        for vector in ('A','B'):
            with self.subTest(vector=vector):
                if vector=='A':
                    self.fail('before site in A')
                marker=1
'''
            test.write_text(source)
            line=next(i for i,s in enumerate(source.splitlines(),1) if 'marker=1' in s)
            subprocess.run([sys.executable,str(Path(__file__).with_name('mutation_probe.py')),str(test),'Synthetic.test_vectors',str(test),str(line),str(output)],check=True)
            result=json.loads(output.read_text())
            self.assertTrue(result['reached'])
            self.assertFalse(result['details'][0]['vector_reached'])
            self.assertEqual(classify({'ran':1,'success':True},result),'INVALID')


if __name__=='__main__':unittest.main(verbosity=2)
