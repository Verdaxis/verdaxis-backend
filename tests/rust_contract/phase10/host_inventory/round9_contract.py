"""Exact depth/file collection identities and publisher-acquisition witness."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, ROOT, TRANSPORT, load
from round7_contract import record


class Round9Contract(unittest.TestCase):
    def setUp(self):
        self.reader=load(READER)
        self.transport=load(TRANSPORT)

    def installed(self,result,expected_false):
        surfaces={'units','timers','dropins','cron','processes','usr_local','packages'}
        self.assertEqual(result['acquired'],{name:name not in expected_false for name in surfaces})
        for observation in result['observations']:
            self.assertEqual(observation['acquired'],observation['surface'] not in expected_false)
        self.assertEqual(result['dependency_coverage'],'manual-review-required')
        rows=result['blocking_dependencies']
        self.assertEqual(rows,sorted(rows,key=lambda r:(self.reader.row_priority(r),json.dumps(r,sort_keys=True))))
        wire=json.dumps(result,sort_keys=True).encode()
        base=json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
        doc=self.transport.collect_hosts(base,lambda argv,data:(wire,True))
        from jsonschema import Draft202012Validator
        Draft202012Validator(json.loads((ROOT/'tests/rust_contract/phase10/host-inventory.schema.json').read_text())).validate(doc)
        return doc

    def test_readable_absolute_child_depth(self):
        for depth in (0,1):
            with self.subTest(depth=depth):
                calls=[]
                parent='/tmp/parent.py'; child='/tmp/child.py'; late='/tmp/z.py'
                denied='/usr/local/libexec/runtime.env'
                def reader(path):
                    calls.append(path)
                    if path==parent:return record(path,executables=[child])
                    if path==late:return record(path,imports=['app.hidden'],credential_references=[denied])
                    self.assertEqual(path,child)
                    return record(path)
                original=self.reader.inspect_graph
                def graph(*args,**kwargs):return original(*args,max_depth=depth,**kwargs)
                def command(argv):return (parent+'\n'+late+'\n',True) if argv[0]=='find' else ('',True)
                with patch.object(self.reader,'inspect_graph',graph):result=self.reader.collect(command,cron_files=[],script_reader=reader)
                self.assertEqual(calls,[parent,late] if depth==0 else [parent,child,late,child])
                self.assertEqual([r['path'] for r in result['scripts']],[parent,late] if depth==0 else [parent,late,child])
                expected=self.reader.unresolved(child) if depth==0 else self.reader.resolved(child)
                self.assertEqual(result['scripts'][0]['executables'],[expected])
                critical=[{'source':self.reader.resolved(late),'reference':self.reader.unresolved(denied),'status':'credential-denied'},
                          {'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'removed-package'}]
                self.assertEqual(result['blocking_dependencies'][:2],critical)
                expected_rows=critical+[{'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'unresolved'}]
                if depth==0:expected_rows.append({'source':self.reader.resolved(parent),'reference':self.reader.unresolved(child),'status':'unresolved'})
                expected_rows.sort(key=lambda r:(self.reader.row_priority(r),json.dumps(r,sort_keys=True)))
                self.assertEqual(result['blocking_dependencies'],expected_rows)
                self.assertEqual(result['truncations'],[])
                self.installed(result,expected_false=set())

    def test_changed_and_overflow_files_collection(self):
        for kind in ('changed','read-overflow'):
            with self.subTest(kind=kind):
                with tempfile.TemporaryDirectory(prefix='w0-r9-') as temp:
                    first=str(Path(temp)/'a.sh'); late=str(Path(temp)/'z.py')
                    Path(first).write_bytes(b'abc')
                    original=self.reader.os.fstat
                    calls=[0]
                    def fstat(fd):
                        value=original(fd);calls[0]+=1
                        if calls[0]==1:Path(first).write_bytes(b'changed' if kind=='changed' else b'x'*40)
                        return value
                    def reader(path):
                        if path==late:return record(path,imports=['app.hidden'],credential_references=['/usr/local/libexec/runtime.env'])
                        with patch.object(self.reader.os,'fstat',fstat),patch.object(self.reader,'MAX_SCRIPT_BYTES',16):return self.reader.scan_script(path)
                    def command(argv):return (first+'\n'+late+'\n',True) if argv[0]=='find' else ('',True)
                    result=self.reader.collect(command,cron_files=[],script_reader=reader)
                    self.assertEqual([r['path'] for r in result['scripts']],[late])
                    self.assertEqual(result['scripts'][0]['imports'],[self.reader.unresolved('app.hidden')])
                    self.assertEqual(result['scripts'][0]['credential_references'],[self.reader.unresolved('/usr/local/libexec/runtime.env')])
                    expected=[{'source':self.reader.resolved(late),'reference':self.reader.unresolved('/usr/local/libexec/runtime.env'),'status':'credential-denied'},
                              {'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'removed-package'},
                              {'source':self.reader.unresolved(first),'reference':self.reader.unresolved('<unreadable-or-forbidden>'),'status':'unresolved'},
                              {'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'unresolved'}]
                    expected.sort(key=lambda r:(self.reader.row_priority(r),json.dumps(r,sort_keys=True)))
                    self.assertEqual(result['blocking_dependencies'],expected)
                    self.assertEqual(result['truncations'],[])
                    self.installed(result,expected_false=set())

    def test_publisher_size_precedes_helper_entry(self):
        result=self.reader.collect(lambda argv:('',True),cron_files=[])
        doc=self.installed(result,expected_false=set())
        original=self.transport.importlib.util.module_from_spec
        guards=[]
        def module_from_spec(spec):
            module=original(spec)
            loader=spec.loader.exec_module
            def execute(target):
                loader(target)
                if spec.name=='w0_inventory':
                    from unittest.mock import Mock
                    guard=Mock(side_effect=AssertionError('destination acquired'))
                    target.private_directory=guard;guards.append(guard)
            spec.loader.exec_module=execute
            return module
        with patch.object(self.transport.importlib.util,'module_from_spec',module_from_spec),patch.object(self.transport,'MAX_BYTES',1):
            with self.assertRaisesRegex(ValueError,'^output bound exceeded$'):self.transport.publish(doc,'fixture.json')
        self.assertEqual(len(guards),1)
        guards[0].assert_not_called()


if __name__=='__main__':unittest.main(verbosity=2)
