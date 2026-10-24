"""Exact final collection expectations for surface retention limits."""
import hashlib
import json
import tempfile
import unittest
from unittest.mock import patch

from collection_assertions import assert_collection, flags, observation, ordered
from reader_contract import READER, load
from round7_contract import record


def unparsed(text):
    return {'value':'<unparsed>','sha256':hashlib.sha256(text.encode()).hexdigest()}


class Round10Contract(unittest.TestCase):
    def setUp(self):
        self.reader=load(READER)

    def test_surface_retention_matrix(self):
        for branch in ('structural','entries','shared-entries','unit-count','cron-list','cron-enumeration','statements','logical-lines'):
            with self.subTest(branch=branch):
                reader=self.reader
                seed='/tmp/critical.py'; denied='/usr/local/libexec/runtime.env'
                source_text=''
                if branch=='structural':source_text='User=first\nGroup=second\nWorkingDirectory=/third\n'
                if branch=='entries':source_text='ExecStart=/zzz/a.py\nExecStart=/zzz/b.py\nExecStart=/zzz/c.py\n'
                if branch=='shared-entries':source_text='EnvironmentFile=/private/a.env\nEnvironmentFile=/private/b.env\n'
                if branch=='statements':source_text='ExecStart="'+'x'*80
                if branch=='logical-lines':source_text='Description="'+'x'*80
                units='a.service b.service c.service' if branch=='unit-count' else 'a.service'
                def command(argv):
                    if 'list-unit-files' in argv:return units,True
                    if 'cat' in argv:return source_text,True
                    if argv[0]=='find':return seed,True
                    if 'crontab' in argv and branch=='shared-entries':return 'JOB=late\n',True
                    return '',True
                critical=record(seed,imports=['app.hidden'],credential_references=[denied])
                def script_reader(path):
                    if path!=seed:raise OSError('fixture absent')
                    return critical
                acquired=flags()
                reasons={}
                if branch in ('structural','entries','statements','logical-lines'):
                    acquired['dropins']=False
                if branch=='shared-entries':acquired['dropins']=False;acquired['cron']=False
                if branch=='unit-count':acquired['units']=False;acquired['dropins']=False
                if branch.startswith('cron-'):acquired['cron']=False
                # Critical facts occupy capacity two, replacing any ordinary rows.
                # Every fixture has the ordinary app.hidden unresolved row too.
                acquired['usr_local']=False
                block=[{'source':reader.resolved(seed),'reference':reader.unresolved(denied),'status':'credential-denied'},
                       {'source':reader.resolved(seed),'reference':reader.unresolved('app.hidden'),'status':'removed-package'},
                       reader.bound_row('blocking-rows'),reader.bound_row('usr_local')]
                if branch in ('structural','entries','statements','logical-lines','shared-entries','unit-count'):block.append(reader.bound_row('dropins')) if branch!='unit-count' else None
                if branch=='unit-count':block.append(reader.bound_row('units'))
                if branch.startswith('cron-') or branch=='shared-entries':block.append(reader.bound_row('cron'))
                expected_script=dict(critical,imports=[reader.unresolved('app.hidden')],credential_references=[reader.unresolved(denied)],truncations=[])
                expected=[]
                unit_fields=[{'key':'unit','values':units.split()}]
                expected.append(observation('units',units,unit_fields,acquired=acquired['units']))
                drop_fields=[];drop_unparsed=[];drop_trunc=[]
                if branch=='structural':
                    drop_fields=[{'key':'User','values':['first']},{'key':'Group','values':['second']}];drop_trunc=['structural-items']
                elif branch=='entries':
                    drop_unparsed=[unparsed(json.dumps({'key':'ExecStart','values':['/zzz/a.py']})),unparsed(json.dumps({'key':'ExecStart','values':['/zzz/b.py']}))];drop_trunc=['structural-items']
                elif branch=='shared-entries':
                    drop_unparsed=[unparsed(json.dumps({'key':'EnvironmentFile','values':['/private/a.env']})),unparsed(json.dumps({'key':'EnvironmentFile','values':['/private/b.env']}))]
                elif branch=='statements':drop_unparsed=[unparsed(source_text)]
                elif branch=='logical-lines':drop_trunc=['logical-lines']
                for _ in range(2 if branch=='unit-count' else 1):
                    expected.append(observation('dropins',source_text,drop_fields,acquired=acquired['dropins'],unparsed=drop_unparsed,truncations=drop_trunc))
                for surface,text,fields in [('timers','',[]),('processes','',[]),('usr_local',seed,[]),('packages','',[])]:
                    up=[unparsed(json.dumps({'key':'executable','values':[seed]}))] if False else []
                    if surface=='usr_local':fields=[{'key':'executable','values':[seed]}]
                    expected.append(observation(surface,text,fields,acquired=acquired[surface],unparsed=up))
                cron_text='JOB=late\n' if branch=='shared-entries' else ''
                for _ in range(3):expected.append(observation('cron',cron_text,acquired=acquired['cron'],unparsed=[unparsed(cron_text)] if cron_text else []))
                class Entry:
                    def __init__(self,name):self.name=name
                    def is_file(self,**kwargs):return True
                class Entries:
                    def __enter__(self):return iter([Entry('a'),Entry('b'),Entry('c')])
                    def __exit__(self,*args):pass
                read_paths=[]
                def read_cron(path):read_paths.append(path);return b'',None
                with tempfile.TemporaryDirectory() as temp:
                    real_open=reader.os.open
                    def open_dir(path,*a,**kw):return real_open(temp,*a,**kw)
                    contexts=[patch.object(reader,'MAX_ITEMS',2),patch.object(reader,'MAX_SCRIPT_BYTES',32 if branch=='statements' else 262144),patch.object(reader,'MAX_OUTPUT_BYTES',32 if branch=='logical-lines' else 2097152)]
                    if branch=='cron-enumeration':contexts += [patch.object(reader.os,'open',open_dir),patch.object(reader.os,'scandir',lambda fd:Entries()),patch.object(reader,'read_file',read_cron)]
                    if branch=='cron-list':contexts += [patch.object(reader,'read_file',read_cron)]
                    from contextlib import ExitStack
                    with ExitStack() as stack:
                        for context in contexts:stack.enter_context(context)
                        result=reader.collect(command,cron_files=None if branch=='cron-enumeration' else ['/etc/cron.d/a','/etc/cron.d/b','/etc/cron.d/c'] if branch=='cron-list' else [],script_reader=script_reader)
                if branch.startswith('cron-'):
                    self.assertEqual(read_paths,['/etc/cron.d/a','/etc/crontab'] if branch=='cron-enumeration' else ['/etc/cron.d/a','/etc/cron.d/b'])
                    for _ in range(2):expected.append(observation('cron','',acquired=False))
                markers={row['status'] for row in block if row['status'].startswith('bound-exceeded:')}
                assert_collection(self,reader,result,scripts=[expected_script],blocking=ordered(reader,block),markers=markers,acquired=acquired,observations=expected)


if __name__=='__main__':unittest.main(verbosity=2)
