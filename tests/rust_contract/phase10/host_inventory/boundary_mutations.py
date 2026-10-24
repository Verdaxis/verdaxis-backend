"""Temporary single-site mutations. Run only in a disposable test checkout."""
import ast
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT=Path(__file__).resolve().parents[4]
R='deploy/tests/host-inventory-reader.py'
T='tests/rust_contract/phase10/host_inventory/transport.py'
B='tests/rust_contract/phase10/host_inventory/'
# exact source, exact replacement, targeted test; each comparison is independent.
SITES=[
 ('priority',R,'if sum(len(items) for items in self.rows.values()) > self.capacity:','if False:', 'round8_contract.py','Round8Contract.test_real_parser_dual_class_overflow_installed'),
 ('critical-overflow',R,'if worst_priority in (1, 2):','if False:','round13_contract.py','Round13Contract.test_eviction_overflow_guard'),
 ('priority-credential',R,"if status == 'credential-denied':",'if False:','round8_contract.py','Round8Contract.test_real_parser_dual_class_overflow_installed'),
 ('priority-removed',R,"if status == 'removed-package':",'if False:','round8_contract.py','Round8Contract.test_real_parser_dual_class_overflow_installed'),
 ('priority-marker',R,'if priority == 3:','if False:','round8_contract.py','Round8Contract.test_real_parser_dual_class_overflow_installed'),
 ('evicted-class',R,"worst_priority == 1",'False','round13_contract.py','Round13Contract.test_eviction_overflow_guard'),
 ('parser-class',R,"priority == 1",'False','round8_contract.py','Round8Contract.test_real_parser_dual_class_overflow_installed'),
 ('encoded',R,'if size > MAX_TOTAL_BYTES:','if False:','round7_contract.py','Round7Contract.test_observation_and_encoded_output_distinct'),
 ('file-type',R,'not stat.S_ISREG(before.st_mode) or before.st_size > MAX_SCRIPT_BYTES','False or before.st_size > MAX_SCRIPT_BYTES','round10_contract.py','Round10Contract.test_nonregular_and_popped_depth_collection'),
 ('file-size',R,'not stat.S_ISREG(before.st_mode) or before.st_size > MAX_SCRIPT_BYTES','not stat.S_ISREG(before.st_mode) or False','round9_contract.py','Round9Contract.test_changed_and_overflow_files_collection'),
 ('read-size',R,'len(raw) > MAX_SCRIPT_BYTES or (before.st_size','False or (before.st_size','round9_contract.py','Round9Contract.test_changed_and_overflow_files_collection'),
 ('metadata',R,'(before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns)','False','round9_contract.py','Round9Contract.test_changed_and_overflow_files_collection'),
 ('references',R,'if len(values) > len(kept):','if False:','round8_contract.py','Round8Contract.test_collection_graph_bound_matrix'),
 ('parser-overflow',R,'if priority in (1, 2):','if False:','round8_contract.py','Round8Contract.test_real_parser_dual_class_overflow_installed'),
 ('statements',R,'if len(pending.encode()) > MAX_SCRIPT_BYTES:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('logical-lines',R,'if len(pending.encode()) > MAX_OUTPUT_BYTES:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('structural',R,'if len(fields) + len(unparsed) > MAX_ITEMS:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('visited',R,'len(visited) >= MAX_ITEMS or depth > max_depth','False or depth > max_depth','round8_contract.py','Round8Contract.test_collection_graph_bound_matrix'),
 ('popped-depth',R,'len(visited) >= MAX_ITEMS or depth > max_depth','len(visited) >= MAX_ITEMS or False','round10_contract.py','Round10Contract.test_nonregular_and_popped_depth_collection'),
 ('graph-bytes',R,'if budget + size > MAX_TOTAL_BYTES:','if False:','round8_contract.py','Round8Contract.test_collection_graph_bound_matrix'),
 ('reference-depth',R,'forbidden(reference) or depth >= max_depth','forbidden(reference) or False','round9_contract.py','Round9Contract.test_readable_absolute_child_depth'),
 ('queue',R,'if len(queue) > MAX_ITEMS:','if False:','round8_contract.py','Round8Contract.test_queue_slice_distinguishes_visited_guard'),
 ('command-deadline',R,'if time.monotonic() >= deadline:','if False:','round8_contract.py','Round8Contract.test_deadline_wait_downstream'),
 ('command-bytes',R,'if total > MAX_OUTPUT_BYTES:','if False:','round8_contract.py','Round8Contract.test_command_prefix_and_collection'),
 ('command-wait',R,'except subprocess.TimeoutExpired:','except subprocess.CalledProcessError:','round8_contract.py','Round8Contract.test_deadline_wait_downstream'),
 ('entry-iterator',R,'if len(entries) >= MAX_ITEMS:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('entry-admission',R,'len(entry_blocks) < MAX_ITEMS','False','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('entry-shared',R,'if len(entry_blocks) >= MAX_ITEMS:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('observation-bytes',R,'if budget > MAX_TOTAL_BYTES:','if False:','round7_contract.py','Round7Contract.test_observation_and_encoded_output_distinct'),
 ('unit-count',R,'if len(names) > MAX_ITEMS:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('cron-enumeration',R,'if len(cron_paths) >= MAX_ITEMS:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('cron-list',R,'if len(cron_paths) > MAX_ITEMS:','if False:','round10_contract.py','Round10Contract.test_surface_retention_matrix'),
 ('ssh-deadline',T,'if time.monotonic() >= deadline:','if False:','round8_contract.py','Round8Contract.test_transport_deadline_wait_downstream'),
 ('ssh-bytes',T,'if total > MAX_BYTES:','if False:','round8_contract.py','Round8Contract.test_transport_and_publisher_downstream'),
 ('ssh-wait',T,'except subprocess.TimeoutExpired:','except subprocess.CalledProcessError:','round8_contract.py','Round8Contract.test_transport_deadline_wait_downstream'),
 ('reply-bytes',T,'if len(data) > MAX_BYTES:','if False:','round10_contract.py','Round10Contract.test_received_response_limit_refusal'),
 ('publish-bytes',T,'if len(data) > MAX_BYTES:','if False:','round9_contract.py','Round9Contract.test_publisher_size_precedes_helper_entry'),
]


def scanned_sites():
    """Independently discover resource comparisons and timeout handlers by AST.

    Lexical arity checks are excluded; metadata/type/overflow guards are included
    as defined by the accepted resource inventory. Compound guards split sites.
    """
    found=[]
    for name in (R,T):
        source=(ROOT/name).read_text()
        tree=ast.parse(source)
        for node in ast.walk(tree):
            if isinstance(node,ast.Compare):
                text=ast.get_source_segment(source,node)
                resource=any(token in text for token in ('MAX_', 'self.capacity', 'deadline', 'max_depth', 'len(kept)'))
                if resource:found.append((name,node.lineno,ast.dump(node,include_attributes=False)))
            elif isinstance(node,ast.ExceptHandler) and isinstance(node.type,ast.Attribute) and node.type.attr=='TimeoutExpired':
                found.append((name,node.lineno,'timeout-handler'))
            elif isinstance(node,ast.UnaryOp) and isinstance(node.op,ast.Not) and isinstance(node.operand,ast.Call) and ast.get_source_segment(source,node.operand.func)=='stat.S_ISREG':
                found.append((name,node.lineno,ast.dump(node,include_attributes=False)))
            elif isinstance(node,ast.Compare) and False:pass
        # Non-numeric independent guards from the established resource scope.
        for node in ast.walk(tree):
            if isinstance(node,ast.Compare):
                text=ast.get_source_segment(source,node)
                if any(isinstance(n,ast.Name) and ('priority' in n.id or n.id=='status') for n in ast.walk(node)) and any(isinstance(n,(ast.Tuple,ast.Set)) or isinstance(n,ast.Constant) and n.value in (1,2,3,'credential-denied','removed-package') for n in ast.walk(node)) or 'before.st_size, before.st_mtime_ns' in text:
                    found.append((name,node.lineno,ast.dump(node,include_attributes=False)))
    return sorted(set(found))


def manifest_sites():
    found=[]
    for site,name,old,new,test,method in SITES:
        source=(ROOT/name).read_text();offset=source.index('def publish(') if site=='publish-bytes' else source.index('def scan_script(') if site=='parser-class' else 0
        index=source.index(old,offset);line=source[:index].count('\n')+1
        # Find the smallest AST comparison/guard represented by this mutation.
        nodes=[]
        for node in ast.walk(ast.parse(source)):
            if isinstance(node,(ast.Compare,ast.UnaryOp,ast.ExceptHandler)) and node.lineno==line:
                text=ast.get_source_segment(source,node) or ''
                if isinstance(node,ast.ExceptHandler):key='timeout-handler'
                else:key=ast.dump(node,include_attributes=False)
                if isinstance(node,ast.Compare) and any(t in text for t in ('MAX_','self.capacity','deadline','max_depth','len(kept)','before.st_size, before.st_mtime_ns','priority','status')):nodes.append((len(text),key))
                elif site=='file-type' and isinstance(node,ast.UnaryOp):nodes.append((len(text),key))
                elif isinstance(node,ast.ExceptHandler):nodes.append((0,key))
        # Disjunct mutation selects its own comparison, not its sibling.
        if site in ('file-type','file-size','read-size','metadata','visited','popped-depth','reference-depth'):
            needle={'file-type':'stat.S_ISREG','file-size':'before.st_size >','read-size':'len(raw) >','metadata':'before.st_mtime_ns','visited':'len(visited)','popped-depth':'depth >','reference-depth':'depth >='}[site]
            nodes=[(len(ast.get_source_segment(source,n) or ''),ast.dump(n,include_attributes=False)) for n in ast.walk(ast.parse(source)) if isinstance(n,(ast.Compare,ast.UnaryOp)) and n.lineno==line and needle in (ast.get_source_segment(source,n) or '')]
        if not nodes:raise ValueError('manifest site not found: '+site)
        found.append((name,line,min(nodes)[1]))
    return sorted(found)


def classify(baseline,mutant):
    if baseline.get('ran')!=1 or not baseline.get('success'):return 'INVALID'
    if mutant.get('ran')!=1 or not mutant.get('reached'):return 'INVALID'
    if mutant.get('success'):return 'SURVIVED'
    details=mutant.get('details',[])
    if details and all(d['kind']=='failure' and d['assertion'] and d['designated'] and d.get('vector_reached',False) for d in details):return 'MUTANT_KILLED'
    return 'INVALID'


def probe(test,method,name,line):
    with tempfile.TemporaryDirectory(prefix='w0-strict-probe-') as folder:
        output=Path(folder)/'result.json'
        result=subprocess.run([sys.executable,'-X','pycache_prefix='+folder,str(ROOT/B/'mutation_probe.py'),str(ROOT/B/test),method,str(ROOT/name),str(line),str(output)],cwd=ROOT,capture_output=True,timeout=30)
        if result.returncode or not output.exists():return {'ran':0,'success':False,'reached':False,'details':[]}
        return json.loads(output.read_text())


def main():
    scanned=scanned_sites();manifest=manifest_sites()
    if scanned!=manifest or len(scanned)!=37:
        print('INVENTORY_MISMATCH',len(scanned),len(manifest),flush=True)
        print('missing',set(scanned)-set(manifest));print('extra',set(manifest)-set(scanned))
        return 1
    failed=False
    originals={name:(ROOT/name).read_bytes() for name in (R,T)}
    for site,name,old,new,test,method in SITES:
        path=ROOT/name;source=originals[name].decode()
        offset=source.index('def publish(') if site=='publish-bytes' else source.index('def scan_script(') if site=='parser-class' else 0
        index=source.index(old,offset);line=source[:index].count('\n')+1
        baseline=probe(test,method,name,line)
        if baseline.get('ran')!=1 or not baseline.get('success'):
            print(f'site={site} line={line} test={test}:{method} baseline=FAIL INVALID',flush=True)
            return 1
        try:
            path.write_text(source[:index]+source[index:].replace(old,new,1))
            mutant=probe(test,method,name,line)
            status=classify(baseline,mutant)
            diagnostic=mutant.get('details',[{}])[0].get('first_line','') if mutant.get('details') else ''
            print(f'site={site} line={line} test={test}:{method} baseline=PASS reached={mutant.get("reached",False)} {status} assertion={json.dumps(diagnostic)}',flush=True)
            failed |= status!='MUTANT_KILLED'
        except subprocess.TimeoutExpired:
            print(f'site={site} line={line} test={test}:{method} baseline=PASS INVALID timeout',flush=True);failed=True
        finally:
            path.write_bytes(originals[name])
            if path.read_bytes()!=originals[name]:raise RuntimeError('restore failed')
    subprocess.run(['git','diff','--exit-code','--',R,T],cwd=ROOT,check=True)
    return int(failed)


if __name__=='__main__':sys.exit(main())
