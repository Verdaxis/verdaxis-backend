"""Run Task 10b with explicit disposable session zone and captured clock."""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from scripts.rust_contract import run_disposable

parser = argparse.ArgumentParser()
parser.add_argument('--timezone', choices=('Asia/Singapore', 'UTC'), required=True)
parser.add_argument('--clock', required=True)
parser.add_argument('--kyc-gate', choices=('enabled', 'disabled'), default='enabled')
args = parser.parse_args()
run_disposable.ORACLE_TIMEZONE = args.timezone
run_disposable.CORPUS_CLOCK = args.clock
runner = run_disposable.Runner('curves_canonical', seed=True, keep=False,
                              seed_mode='corpus', gate=args.kyc_gate == 'enabled')
try:
    runner.run()
finally:
    runner.cleanup()
