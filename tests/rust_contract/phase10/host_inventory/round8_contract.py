"""Tests only: final-output invariants and branch-discriminating witnesses."""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from reader_contract import READER, ROOT, TRANSPORT, load
from round7_contract import record
from collection_assertions import assert_collection, flags, observation, ordered
from unittest.mock import ANY


class Round8Contract(unittest.TestCase):
    def setUp(self):
        self.reader = load(READER)
        self.transport = load(TRANSPORT)

    def installed(self,result,expected_false,*,scripts,blocking,observations):
        markers={row['status'] for row in blocking if row['status'].startswith(('bound-exceeded:','overflow:'))}
        return assert_collection(self,self.reader,result,scripts=scripts,blocking=blocking,
                                 markers=markers,acquired=flags(*expected_false),observations=observations)

    def observations(self,paths,failed=(),unit_text='',drop_text=None,drop_fields=()):
        fields=[{'key':'unit','values':unit_text.split()}] if unit_text else []
        rows=[observation('units','\n'.join(sorted(unit_text.splitlines())),fields,acquired='units' not in failed)]
        if unit_text:
            rows.append(observation('dropins',drop_text or '',drop_fields,acquired='dropins' not in failed))
        rows.extend([observation('timers','',acquired='timers' not in failed),observation('processes','',acquired='processes' not in failed),
                     observation('usr_local','\n'.join(sorted(paths)),[{'key':'executable','values':[p]} for p in sorted(paths)],acquired='usr_local' not in failed),
                     observation('packages','',acquired='packages' not in failed)])
        rows.extend(observation('cron','',acquired='cron' not in failed) for _ in range(3))
        return rows

    def markers(self, result):
        return {r['status'] for r in result['blocking_dependencies'] if r['status'].startswith(('bound-exceeded:','overflow:'))}

    def test_queue_slice_distinguishes_visited_guard(self):
        calls=[]; queues=[]
        def reader(path):
            calls.append(path)
            return record(path,executables=['/tmp/1.py','/tmp/2.py','/tmp/3.py'] if path=='/tmp/a.py' else [])
        # Trace real queue state at loop entry, without replacing the bound.
        def trace(frame,event,arg):
            if event=='line' and frame.f_code.co_name=='inspect_graph':
                line=frame.f_lineno
                source=READER.read_text().splitlines()[line-1].strip()
                if source=='while queue:': queues.append(tuple(frame.f_locals['queue']))
            return trace
        original=self.reader.inspect_graph
        def graph(*args,**kwargs):
            with patch.object(self.reader,'MAX_ITEMS',2):
                sys.settrace(trace)
                try: return original(*args,**kwargs)
                finally: sys.settrace(None)
        def command(argv): return ('/tmp/a.py\n',True) if argv[0]=='find' else ('',True)
        with patch.object(self.reader,'inspect_graph',graph): result=self.reader.collect(command,cron_files=[],script_reader=reader)
        self.assertEqual(queues[1],(('/tmp/1.py',1),('/tmp/2.py',1)))
        self.assertEqual(calls,['/tmp/a.py','/tmp/1.py','/tmp/2.py','/tmp/3.py','/tmp/1.py'])
        self.assertEqual([r['path'] for r in result['scripts']],['/tmp/a.py','/tmp/1.py'])
        self.assertEqual(self.markers(result),{'bound-exceeded:import-queue','bound-exceeded:usr_local'})
        self.assertFalse(result['acquired']['usr_local'])
        expected_scripts=[dict(record('/tmp/a.py'),executables=[self.reader.resolved('/tmp/'+str(i)+'.py') for i in (1,2,3)],truncations=[]),dict(record('/tmp/1.py'),truncations=[])]
        expected_rows=ordered(self.reader,[self.reader.bound_row('import-queue'),self.reader.bound_row('usr_local')])
        self.installed(result,expected_false={'usr_local'},scripts=expected_scripts,blocking=expected_rows,observations=self.observations(['/tmp/a.py'],failed={'usr_local'}))

    def test_collection_graph_bound_matrix(self):
        denied='/usr/local/libexec/runtime.env'
        for bound in ('references','blocking-rows','visited','graph-bytes','depth','file'):
            with self.subTest(bound=bound):
                with tempfile.TemporaryDirectory(prefix='w0-r8-') as temp:
                    first=str(Path(temp)/'a.py'); late=str(Path(temp)/'z.py')
                    Path(first).write_text('import ordinary_a\nimport ordinary_b\nimport ordinary_c\n')
                    Path(late).write_text('import app.hidden\n')
                    # External acquisition yields a late parsed record with both critical classes.
                    def reader(path):
                        if path==late: return record(path,imports=['app.hidden'],credential_references=[denied])
                        if bound=='file':
                            with patch.object(self.reader,'MAX_SCRIPT_BYTES',1): return self.reader.scan_script(path)
                        return self.reader.scan_script(path)
                    original=self.reader.inspect_graph
                    def graph(*args,**kwargs):
                        with patch.object(self.reader,'MAX_ITEMS',2 if bound!='visited' else 3), patch.object(self.reader,'MAX_TOTAL_BYTES',len(json.dumps(reader(first)).encode())+1 if bound=='graph-bytes' else 7340032):
                            if bound=='depth': kwargs['max_depth']=0
                            return original(*args,**kwargs)
                    # Visited cap is exercised after late critical acquisition; fourth seed is dropped.
                    seeds=[first,late] if bound!='visited' else [first,late,late[:-3]+'1.py',late[:-3]+'2.py']
                    def acquisition(path):
                        if path in seeds[2:]: return record(path)
                        return reader(path)
                    def command(argv): return ('\n'.join(seeds)+'\n',True) if argv[0]=='find' else ('',True)
                    with patch.object(self.reader,'inspect_graph',graph):
                        result=self.reader.collect(command,cron_files=[],script_reader=acquisition)
                    statuses={r['status'] for r in result['blocking_dependencies']}
                    self.assertIn('credential-denied',statuses); self.assertIn('removed-package',statuses)
                    self.assertEqual([row for row in result['blocking_dependencies'] if row['status'] in ('credential-denied','removed-package')], [
                        {'source':self.reader.resolved(late),'reference':self.reader.unresolved(denied),'status':'credential-denied'},
                        {'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'removed-package'}])
                    for script in result['scripts']:
                        if script['path']==first:
                            count=3 if bound=='visited' else 2
                            self.assertEqual(script['imports'],[self.reader.unresolved(name) for name in ('ordinary_a','ordinary_b','ordinary_c')[:count]])
                            self.assertEqual(script['executables'],[])
                            self.assertEqual(script['credential_references'],[])
                        elif script['path']==late:
                            self.assertEqual(script['imports'],[self.reader.unresolved('app.hidden')])
                            self.assertEqual(script['credential_references'],[self.reader.unresolved(denied)])
                            self.assertEqual(script['executables'],[])
                        else:
                            self.assertEqual((script['imports'],script['executables'],script['credential_references']),([],[],[]))
                    self.assertNotIn(denied,json.dumps(result))
                    expected={'bound-exceeded:blocking-rows','bound-exceeded:usr_local'}
                    if bound in ('blocking-rows','graph-bytes','depth'): expected.add('bound-exceeded:references')
                    if bound=='references': expected.add('bound-exceeded:references')
                    if bound=='visited': expected.add('bound-exceeded:import-queue')
                    if bound=='graph-bytes': expected.add('bound-exceeded:graph-bytes')
                    if bound=='file': expected={'bound-exceeded:blocking-rows','bound-exceeded:usr_local'}
                    self.assertEqual(self.markers(result),expected)
                    expected_paths=[] if bound=='file' else [first]
                    if bound!='graph-bytes': expected_paths += [late]
                    if bound=='visited': expected_paths += [late[:-3]+'1.py']
                    self.assertEqual([r['path'] for r in result['scripts']],expected_paths)
                    script_expectations=[]
                    for path in expected_paths:
                        if path==first:
                            names=('ordinary_a','ordinary_b','ordinary_c')[:3 if bound=='visited' else 2]
                            script_expectations.append(dict(path=path,uid=ANY,gid=ANY,mode=ANY,size=len(Path(path).read_bytes()),sha256=__import__('hashlib').sha256(Path(path).read_bytes()).hexdigest(),imports=[self.reader.unresolved(n) for n in names],executables=[],credential_references=[],read_acquired=True,truncations=[] if bound=='visited' else ['references'],dropped={} if bound=='visited' else {'4':1},priority_overflow=[]))
                        elif path==late:
                            script_expectations.append(dict(record(late),imports=[self.reader.unresolved('app.hidden')],credential_references=[self.reader.unresolved(denied)],truncations=[]))
                        else:script_expectations.append(dict(record(path),truncations=[]))
                    critical=[{'source':self.reader.resolved(late),'reference':self.reader.unresolved(denied),'status':'credential-denied'}, {'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'removed-package'}]
                    expected_blocking=ordered(self.reader,critical+[self.reader.bound_row(status.split(':',1)[1]) for status in expected]+([{'source':self.reader.resolved(late),'reference':self.reader.unresolved('app.hidden'),'status':'unresolved'}] if bound=='visited' else []))
                    obs=self.observations(seeds,failed={'usr_local'})
                    if bound in ('graph-bytes','file'):
                        retained=set(expected_paths)
                        obs[3]['fields']=[{'key':'executable','values':[p]} for p in sorted(seeds) if p in retained]
                        from round10_contract import unparsed
                        obs[3]['unparsed']=[unparsed(json.dumps({'key':'executable','values':[p]})) for p in sorted(seeds) if p not in retained]
                    self.installed(result,expected_false={'usr_local'},scripts=script_expectations,blocking=expected_blocking,observations=obs)

    def test_real_parser_dual_class_overflow_installed(self):
        for kind in ('credential-denied','removed-package'):
            with self.subTest(kind=kind):
                with tempfile.TemporaryDirectory(prefix='w0-r8-') as temp:
                    suffix='.sh' if kind=='credential-denied' else '.py'
                    paths=[str(Path(temp)/(name+suffix)) for name in ('a','z')]
                    for index,path in enumerate(paths):
                        names=range(index*3,index*3+3)
                        Path(path).write_text(''.join('source /usr/local/libexec/runtime'+str(i)+'.env\n' if kind=='credential-denied' else 'import app.module'+str(i)+'\n' for i in names))
                    original=self.reader.inspect_graph
                    def graph(*a,**kw):
                        with patch.object(self.reader,'MAX_ITEMS',2): return original(*a,**kw)
                    def command(argv): return ('\n'.join(paths)+'\n',True) if argv[0]=='find' else ('',True)
                    with patch.object(self.reader,'inspect_graph',graph): result=self.reader.collect(command,cron_files=[])
                    expected_scripts=[]; candidates=[]
                    for index,path in enumerate(paths):
                        names=['/usr/local/libexec/runtime'+str(i)+'.env' if kind=='credential-denied' else 'app.module'+str(i) for i in range(index*3,index*3+2)]
                        raw=Path(path).read_bytes()
                        expected_scripts.append(dict(path=path,uid=ANY,gid=ANY,mode=ANY,size=len(raw),sha256=__import__('hashlib').sha256(raw).hexdigest(),imports=[] if kind=='credential-denied' else [self.reader.unresolved(n) for n in names],executables=[],credential_references=[self.reader.unresolved(n) for n in names] if kind=='credential-denied' else [],read_acquired=True,truncations=['references'],dropped={'1' if kind=='credential-denied' else '2':1},priority_overflow=[kind]))
                        candidates.extend({'source':self.reader.resolved(path),'reference':self.reader.unresolved(n),'status':kind} for n in names)
                    retained=ordered(self.reader,candidates)[:2]
                    overflow=dict(self.reader.bound_row(kind),status='overflow:'+kind)
                    expected_rows=ordered(self.reader,retained+[overflow]+[self.reader.bound_row(x) for x in ('references','blocking-rows','usr_local')])
                    doc=self.installed(result,expected_false={'usr_local'},scripts=expected_scripts,blocking=expected_rows,observations=self.observations(paths,failed={'usr_local'}))
                    for snapshot in doc['host_snapshots'].values():
                        self.assertEqual(sum(r['status']=='overflow:'+kind for r in snapshot['blocking_dependencies']),1)
                    self.assertEqual(self.markers(result),{'overflow:'+kind,'bound-exceeded:references','bound-exceeded:blocking-rows','bound-exceeded:usr_local'})
                    self.assertEqual([r['path'] for r in result['scripts']],paths)

    def test_command_prefix_and_collection(self):
        original=self.reader.subprocess.Popen
        # Synchronize child stdout prefix before stderr saturation using a short
        # pause in the fixture child only; collector timeout/bound is unmocked.
        def spawn(argv,**kwargs): return original([sys.executable,'-c',"import os,time; os.write(1,b'User=kept\\npartial'); time.sleep(.05); os.write(2,b'x'*4096)"],**kwargs)
        with patch.object(self.reader.subprocess,'Popen',spawn),patch.object(self.reader,'MAX_OUTPUT_BYTES',32):
            text,ok=self.reader.run_command(self.reader.COMMANDS['units'])
        self.assertEqual((text,ok),('User=kept\n',False))
        def command(argv):
            if 'list-unit-files' in argv:return 'a.service\n',True
            if 'cat' in argv:return text,ok
            return '',True
        result=self.reader.collect(command,cron_files=[])
        self.assertEqual(result['observations'][1]['fields'],[{'key':'User','values':['kept']}])
        self.assertEqual(self.markers(result),{'bound-exceeded:dropins'})
        self.assertFalse(result['acquired']['dropins'])
        self.installed(result,expected_false={'dropins'},scripts=[],blocking=[self.reader.bound_row('dropins')],observations=self.observations([],failed={'dropins'},unit_text='a.service\n',drop_text=text,drop_fields=[{'key':'User','values':['kept']}]))

    def test_deadline_wait_downstream(self):
        import subprocess
        for branch in ('deadline','wait'):
            with self.subTest(branch=branch):
                original=self.reader.subprocess.Popen
                def spawn(argv,**kw):
                    child=original([sys.executable,'-c','import time;time.sleep(60)'],**kw)
                    if branch=='wait':
                        real_wait=child.wait; calls=[0]
                        def wait(*a,**k):
                            calls[0]+=1
                            if calls[0]==1: raise subprocess.TimeoutExpired('fixture',.1)
                            return real_wait(*a,**k)
                        child.wait=wait
                    return child
                class EmptySelector:
                    def __enter__(self):return self
                    def __exit__(self,*a):pass
                    def register(self,*a):pass
                    def get_map(self):return {}
                clock=patch.object(self.reader.time,'monotonic',side_effect=[0,10000,10000,10000]) if branch=='deadline' else patch.object(self.reader.selectors,'DefaultSelector',EmptySelector)
                with patch.object(self.reader.subprocess,'Popen',spawn),clock:
                    raw=self.reader.run_command(self.reader.COMMANDS['units'])
                result=self.reader.collect(lambda argv:raw,cron_files=[])
                self.assertEqual(result['scripts'],[])
                self.assertEqual(self.markers(result),{'bound-exceeded:'+x for x in ('units','timers','processes','usr_local','packages','cron')})
                self.assertTrue(all(not flag for flag in result['acquired'].values()))
                failed={'units','dropins','timers','processes','usr_local','packages','cron'}
                self.installed(result,expected_false=failed,scripts=[],blocking=ordered(self.reader,[self.reader.bound_row(x) for x in failed if x!='dropins']),observations=self.observations([],failed=failed))

    def test_transport_deadline_wait_downstream(self):
        import subprocess
        for branch in ('deadline','wait'):
            with self.subTest(branch=branch):
                original=self.transport.subprocess.Popen
                def spawn(argv,**kw):
                    child=original([sys.executable,'-c','import time;time.sleep(60)'],**kw)
                    if branch=='wait':
                        real_wait=child.wait; calls=[0]
                        def wait(*a,**k):
                            calls[0]+=1
                            if calls[0]==1:raise subprocess.TimeoutExpired('fixture',.1)
                            return real_wait(*a,**k)
                        child.wait=wait
                    return child
                class EmptySelector:
                    def __enter__(self):return self
                    def __exit__(self,*a):pass
                    def register(self,*a):pass
                    def get_map(self):return {}
                clock=patch.object(self.transport.time,'monotonic',side_effect=[0,10000,10000,10000]) if branch=='deadline' else patch.object(self.transport.selectors,'DefaultSelector',EmptySelector)
                with patch.object(self.transport.subprocess,'Popen',spawn),clock:
                    raw=self.transport.ssh_once(['fixture'],b'x')
                self.assertEqual(raw,(b'',False))
                self.assertEqual(self.transport.read_host('staging',lambda a,d:raw),{'refusal':'transport-error'})
                base=json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
                self.assertEqual(self.transport.collect_hosts(base,lambda a,d:raw),{'host_refusals':{'staging':{'refusal':'transport-error'},'production':{'refusal':'transport-error'}}})

    def test_transport_and_publisher_downstream(self):
        original=self.transport.subprocess.Popen
        def spawn(argv,**kw):return original([sys.executable,'-c',"import os;os.write(1,b'x'*4096)"],**kw)
        with patch.object(self.transport.subprocess,'Popen',spawn),patch.object(self.transport,'MAX_BYTES',32):
            raw=self.transport.ssh_once(['fixture'],b'x')
        self.assertEqual(self.transport.read_host('staging',lambda a,d:raw),{'refusal':'transport-error'})
        base=json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
        self.assertEqual(self.transport.collect_hosts(base,lambda a,d:raw),{'host_refusals':{'staging':{'refusal':'transport-error'},'production':{'refusal':'transport-error'}}})
        result=self.reader.collect(lambda argv:('',True),cron_files=[])
        doc=self.installed(result,expected_false=set(),scripts=[],blocking=[],observations=self.observations([]))
        real_open=self.transport.os.open
        def guarded_open(path,*args,**kwargs):
            if isinstance(path,str) and str(self.transport.EVIDENCE_ROOT).startswith(path) and path not in ('/', '/Users', '/Users/jonathanjie'):
                raise AssertionError('destination opened')
            return real_open(path,*args,**kwargs)
        with patch.object(self.transport,'MAX_BYTES',1),patch.object(self.transport.os,'open',guarded_open):
            with self.assertRaisesRegex(ValueError,'^output bound exceeded$'):self.transport.publish(doc,'fixture.json')


if __name__=='__main__':unittest.main(verbosity=2)
