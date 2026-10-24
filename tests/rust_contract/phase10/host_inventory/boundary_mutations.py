"""Temporary single-site mutations. Run only in a disposable test checkout."""
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


def main():
    failed=False
    originals={name:(ROOT/name).read_bytes() for name in (R,T)}
    for site,name,old,new,test,method in SITES:
        path=ROOT/name;source=originals[name].decode()
        # publish and reply have the same check: select the intended function.
        offset=source.index('def publish(') if site=='publish-bytes' else 0
        index=source.index(old,offset)
        line=source[:index].count('\n')+1
        mutated=source[:index]+source[index:].replace(old,new,1)
        try:
            path.write_text(mutated)
            # Avoid stale timestamp/size-based pycache across rapid restoration.
            with tempfile.TemporaryDirectory(prefix='w0-mut-cache-') as cache:
                result=subprocess.run([sys.executable,'-X','pycache_prefix='+cache,str(ROOT/B/test),method],cwd=ROOT,capture_output=True,timeout=30)
            killed=result.returncode!=0
            print(f'site={site} line={line} test={test}:{method} '+('MUTANT_KILLED' if killed else 'SURVIVED'),flush=True)
            failed |= not killed
        except subprocess.TimeoutExpired:
            print(f'site={site} line={line} test={test}:{method} SURVIVED timeout',flush=True)
            failed=True
        finally:
            path.write_bytes(originals[name])
            if path.read_bytes()!=originals[name]:raise RuntimeError('restore failed')
    subprocess.run(['git','diff','--exit-code','--',R,T],cwd=ROOT,check=True)
    return int(failed)


if __name__=='__main__':sys.exit(main())
