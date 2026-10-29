"""Start either test adapter behind the loopback body-stall relay."""
import os
import signal
import subprocess
import sys
from pathlib import Path

from scripts.rust_contract.phase2_routes import rust_example_path
from scripts.rust_contract.referral_timeout_relay import start

ROOT = Path(__file__).resolve().parents[2]


def stop_on_signal(signal_number, frame):
    """Let the runner's SIGTERM unwind through the cleanup below."""
    raise SystemExit(128 + signal_number)


def main():
    if os.environ.get('ENVIRONMENT') != 'test':
        raise SystemExit('test only')
    signal.signal(signal.SIGTERM, stop_on_signal)
    target = start(os.environ['PHASE2_MAIL_URL'])
    environment = {**os.environ, 'PHASE2_MAIL_URL': target}
    if sys.argv[1] == 'python':
        command = [sys.executable, '-m', 'scripts.rust_contract.referral_oracle', *sys.argv[2:]]
    else:
        command = [str(rust_example_path(ROOT, 'referral_contract'))]
    child = subprocess.Popen(command, env=environment)
    try:
        raise SystemExit(child.wait())
    finally:
        if child.poll() is None:
            child.terminate()
        child.wait(timeout=15)


if __name__ == '__main__':
    main()
