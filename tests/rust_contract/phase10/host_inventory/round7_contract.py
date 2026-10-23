"""Byte admission must not erase already parsed priority evidence."""
import json
import sys
import tempfile
import unittest
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

    def validate(self, result):
        transport = load(TRANSPORT)
        base = json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
        doc = transport.collect_hosts(base, lambda argv,data: (json.dumps(result).encode(), True))
        from jsonschema import Draft202012Validator
        Draft202012Validator(json.loads((ROOT/'tests/rust_contract/phase10/host-inventory.schema.json').read_text())).validate(doc)
        self.assertEqual(result['dependency_coverage'], 'manual-review-required')
        rows = result['blocking_dependencies']
        self.assertEqual(rows, sorted(rows, key=lambda r:(self.reader.row_priority(r),json.dumps(r,sort_keys=True))))
        for item in result['observations']:
            if not result['acquired'][item['surface']]: self.assertFalse(item['acquired'])

    def test_late_priority_before_graph_bytes(self):
        for kind in ('denial','removed','overflow'):
            with self.subTest(vector=kind):
                first = record('/tmp/a.py')
                last = record('/tmp/z.py', credential_references=['/usr/local/libexec/runtime.env'] if kind=='denial' else [],
                              imports=['app.hidden'] if kind=='removed' else [],
                              priority_overflow=['credential-denied'] if kind=='overflow' else [])
                def reader(path): return first if path.endswith('a.py') else last
                def command(argv): return ('/tmp/a.py\n/tmp/z.py\n',True) if argv[0]=='find' else ('',True)
                # Prefix record fits; next parsed record exceeds remaining graph bytes.
                budget = len(json.dumps(first).encode()) + 1
                with patch.object(self.reader,'GRAPH_BYTES',budget,create=True):
                    # Backward-compatible RED patches the old shared limit only
                    # around graph execution, not observation/output validation.
                    original = self.reader.inspect_graph
                    def graph(*args,**kwargs):
                        with patch.object(self.reader,'MAX_TOTAL_BYTES',budget): return original(*args,**kwargs)
                    with patch.object(self.reader,'inspect_graph',graph):
                        result = self.reader.collect(command,cron_files=[],script_reader=reader)
                expected = {'denial':'credential-denied','removed':'removed-package','overflow':'overflow:credential-denied'}[kind]
                self.assertTrue(any(r['status']==expected for r in result['blocking_dependencies']))
                self.assertEqual([r['path'] for r in result['scripts']], ['/tmp/a.py'])
                self.assertTrue(any(r['status']=='bound-exceeded:graph-bytes' for r in result['blocking_dependencies']))
                self.assertNotIn('runtime.env',json.dumps(result))
                self.validate(result)

    def test_marker_canonicalization(self):
        for status in ('credential-denied','removed-package'):
            with self.subTest(vector=status):
                rows=self.reader.PriorityRows(1)
                rows.add(dict(self.reader.bound_row(status),status='overflow:'+status))
                for i in range(3): rows.add(dict(self.reader.bound_row(str(i)),status=status))
                self.assertEqual(sum(r['status']=='overflow:'+status for r in rows.values()),1)

    def test_queue_slice_actual(self):
        def reader(path):
            return record(path,executables=['/tmp/1.py','/tmp/2.py','/tmp/3.py'] if path.endswith('a.py') else [])
        with patch.object(self.reader,'MAX_ITEMS',2):
            result=self.reader.inspect_graph(['/tmp/a.py'],script_reader=reader)
        self.assertIn('import-queue',result['truncations'])
        self.assertLessEqual(len(result['scripts']),2)
        self.assertEqual(sum(r['status']=='bound-exceeded:import-queue' for r in result['blocking_dependencies']),1)

    def test_cron_enumeration_actual(self):
        class Entry:
            def __init__(self,name): self.name=name
            def is_file(self,**kwargs): return True
        class Entries:
            def __enter__(self): return iter([Entry('a'),Entry('b'),Entry('c')])
            def __exit__(self,*args): pass
        with tempfile.TemporaryDirectory() as temp:
            # Patch only directory/file acquisition, not enumeration or its bound.
            real_open=self.reader.os.open
            def open_dir(path,*args,**kwargs): return real_open(temp,*args,**kwargs)
            with patch.object(self.reader,'MAX_ITEMS',2), patch.object(self.reader.os,'open',open_dir), patch.object(self.reader.os,'scandir',lambda fd:Entries()), patch.object(self.reader,'read_file',lambda path:(b'',None)):
                result=self.reader.collect(lambda argv:('',True))
        self.assertFalse(result['acquired']['cron'])
        self.assertIn('cron-count',[c for r in result['truncations'] for c in r['reasons']])
        self.validate(result)

    def test_real_command_byte_drain(self):
        # Real pipe/process/select/drain code; only external command acquisition is substituted.
        original=self.reader.subprocess.Popen
        def spawn(argv,**kwargs):
            return original([sys.executable,'-c',"import os; os.write(1,b'User=ok\\npartial'); os.write(2,b'x'*4096)"],**kwargs)
        with patch.object(self.reader.subprocess,'Popen',spawn),patch.object(self.reader,'MAX_OUTPUT_BYTES',32):
            text,acquired=self.reader.run_command(self.reader.COMMANDS['units'])
        self.assertFalse(acquired)
        self.assertTrue(not text or text.endswith('\n'))
        self.assertNotIn('partial',text)
        self.assertNotIn('xxxx',text)

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
        transport=load(TRANSPORT)
        with patch.object(transport,'MAX_BYTES',8):
            self.assertEqual(transport.read_host('staging',lambda a,d:(b'x'*9,True)),{'refusal':'output-bytes'})
        original=transport.subprocess.Popen
        def spawn(argv,**kwargs): return original([sys.executable,'-c',"import os; os.write(1,b'x'*4096)"],**kwargs)
        with patch.object(transport.subprocess,'Popen',spawn),patch.object(transport,'MAX_BYTES',32):
            data,success=transport.ssh_once(['fixture'],b'pass')
        self.assertEqual((data,success),(b'',False))
        base=json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
        snapshot=self.reader.collect(lambda argv:('',True),cron_files=[])
        doc=transport.collect_hosts(base,lambda a,d:(json.dumps(snapshot).encode(),True))
        with patch.object(transport,'MAX_BYTES',1):
            with self.assertRaises(ValueError): transport.publish(doc,'fixture.json')


if __name__=='__main__': unittest.main(verbosity=2)
