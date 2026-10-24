"""Exact depth/file collection identities and publisher-acquisition witness."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, ROOT, TRANSPORT, load
from round7_contract import record
from collection_assertions import assert_collection, flags, observation, ordered


class Round9Contract(unittest.TestCase):
    def setUp(self):
        self.reader=load(READER)
        self.transport=load(TRANSPORT)

    def installed(self,result,expected_false,*,scripts,blocking,observations):
        return assert_collection(self,self.reader,result,scripts=scripts,blocking=blocking,
                                 markers=set(),acquired=flags(*expected_false),observations=observations)

    def observations(self,paths):
        text='\n'.join(sorted(paths))
        return [observation('units',''),observation('timers',''),observation('processes',''),
                observation('usr_local',text,[{'key':'executable','values':[p]} for p in sorted(paths)]),
                observation('packages',''),*[observation('cron','') for _ in range(3)]]

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
                expected_scripts=[dict(record(parent),executables=[expected],truncations=[]),
                                  dict(record(late),imports=[self.reader.unresolved('app.hidden')],credential_references=[self.reader.unresolved(denied)],truncations=[])]
                if depth:expected_scripts.append(dict(record(child),truncations=[]))
                self.installed(result,expected_false=set(),scripts=expected_scripts,blocking=expected_rows,observations=self.observations([parent,late]))

    def test_changed_and_overflow_files_collection(self):
        for kind in ('size','changed','read-overflow'):
            with self.subTest(kind=kind):
                with tempfile.TemporaryDirectory(prefix='w0-r9-') as temp:
                    first=str(Path(temp)/'a.sh'); late=str(Path(temp)/'z.py')
                    Path(first).write_bytes(b'x'*40 if kind=='size' else b'abc')
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
                    expected_script=dict(record(late),imports=[self.reader.unresolved('app.hidden')],credential_references=[self.reader.unresolved('/usr/local/libexec/runtime.env')],truncations=[])
                    expected_observations=self.observations([first,late])
                    expected_observations[3]['fields']=[{'key':'executable','values':[late]}]
                    from round10_contract import unparsed
                    expected_observations[3]['unparsed']=[unparsed(json.dumps({'key':'executable','values':[first]}))]
                    self.installed(result,expected_false=set(),scripts=[expected_script],blocking=expected,observations=expected_observations)

    def test_publisher_size_precedes_helper_entry(self):
        result=self.reader.collect(lambda argv:('',True),cron_files=[])
        doc=self.installed(result,expected_false=set(),scripts=[],blocking=[],observations=self.observations([]))
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
