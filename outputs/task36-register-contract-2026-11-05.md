# Task 36 manual register and fixture contract

## Scope

Local write lane on `feat/rust-phase-10-t36`. No host access, installation,
activation, restore, push or live acceptance. The PM classification dated
2026-11-05 is recorded unchanged. Owner acceptance covers classification only,
not completed replacement handover. App-owned units remain with their named
Phase 10 tasks.

The plan and original inventory captures are in the parent project's `outputs/`.
The register names and hashes those three local capture files. These captures
are not copied into this worktree. Script digests not supplied by the capture
are null, not fabricated. The additional local backup script review supplied
SSH credential reference paths and the restore-helper path; no credential
contents were read.

## Verification

- RED: `HOST_INVENTORY_TEST_PYTHON=python3.14 bash deploy/tests/host-automation-contract.sh`
  failed with `required W0 rows have no PM classification` before register fill.
- GREEN: the same command passes all five named checks.
- `python3.14 -m pytest -c /dev/null -p no:cacheprovider --noconftest -q tests/rust_contract/test_phase10_*.py`:
  **1331 passed**.
- `shellcheck deploy/tests/host-automation-contract.sh`: exit 0.
- `git diff --check`: exit 0.
- Affected synthetic fixture subset: **10 passed, 2 deselected, 31 subtests passed**.
  The two deselected cases exercise paths outside the reader allowlist on this
  Mac. Synthetic transport tests now use `observations.json`, not the mutable
  manual register.
- Broader `host_inventory/*contract.py` run with `TMPDIR=/private/tmp`:
  **26 failed, 54 passed, 156 subtests passed**. Failures concern temporary script
  paths rejected by the inspection allowlist and resulting missing graph rows.
  The full shell inventory harness stops at the first reader path rejection.
  Its no-follow/allowlist boundary was not weakened. Logs are under `work/`.

The default `python3` lacks `jsonschema`; `python3.14` has the required local
contract dependencies. `/dev/null` disables unrelated project pytest settings;
cache is disabled to avoid writes under `/dev`.

## Not acceptance

Fixture GREEN checks that the register retains backup-db.sh and its restore
verification row unchanged. It does not prove restored data or row counts.
Only the PM-supplied backup timer expression (daily 03:00 Asia/Singapore) is
recorded as known; other installed timer expressions and active states remain
uncollected. No cron entry was found and no removal is authorized.

Before W5, the integration/operator lane must collect missing installed script
digests, effective schedules, credential-loading paths, locks, retention
settings, export SQL identity, and restore evidence. Fleet remains Task 25-owned.
Installed/signed acceptance and live replacement handover remain separate gates.
