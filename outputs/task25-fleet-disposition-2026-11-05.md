# Task 25 — Fleet publication disposition

## Decision

PM disposition: **off-host, not installed, not ported**. The owner is the
PM/founder. Capture runs manually on their macOS workstation. No Verdaxis host
browser, cron, timer, capture adapter or Rust parser is added.

Retain `scripts/scrape_fleet_demand.py` in the repository after W5, outside
`deploy/`, for audit and optional manual workstation use. Exclude it from the
release artifact and every host installation, as with Task 26 off-host tools.
The removal ledger and `scripts.json` now accept this disposition. Retention is
not permission to execute it on a host.

Publication remains a reviewed commit of `data/fleet_demand.json`. PM identifies
PR #159, merged to staging as `5d609212d`; the existing Rust reader is
`rs/read/src/market/fleet_snapshot.rs`. This checkout has neither that data file
nor the supplied merge object. No artifact contents or merge bytes were locally
verified, and no replacement data was invented.

## Evidence and boundaries

The Task 25 record is `tests/rust_contract/phase10/fleet-disposition.json`.
It binds the unchanged Task 36 register by SHA-256:
`1f91f4c3124c23fb2b10ae75d3915d53002a9e1186e9c8fa3ec9a9c5f9596483`.
The register validates but is `collected-manual`, not signed installed
acceptance. Its limitations and outstanding W5 gates remain unchanged.

The PM probe at `outputs/research/rust-phase10/t36/fleet-probe.txt` in the
parent workspace was read locally. Its verified SHA-256 is
`f5affd0d5a720207b15874ea9fd967043abf74bc34bd4f64d682eb7f4cfccf14`.
PM collected it at 2026-11-05 09:49Z. Both hosts have no scraper cron/unit/timer
reference or installed agent-browser. The shared host's `fleet-*` units are
OpenClaw, not Verdaxis; no Verdaxis rows were added for them.

PM also attests no workstation crontab or LaunchAgent/LaunchDaemon reference.
The manual workstation browser path is `/opt/homebrew/bin/agent-browser`.
The pinned repository script still has a legacy Linux path and a suggested
weekly cron. Source text does not prove installed state. The script is unchanged.
Its AST imports only `datetime`, `json`, `pathlib`, `re` and `subprocess`:
no removed application package is required.

## TDD and verification

Used a temporary test environment at `work/task25-venv` with pytest, httpx,
SQLAlchemy and jsonschema. No runtime app, host connection or browser ran.

Command prefix:

```sh
ENVIRONMENT=test RELEASE_SHA=test \
JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
DATABASE_URL=sqlite+aiosqlite:///:memory:
```

- RED: `work/task25-venv/bin/python -m pytest tests/rust_contract/test_phase10_fleet_disposition.py -q`
  returned **1 failed, 3 passed**. The sole failure was
  `fleet_disposition_record_is_complete_and_bound_to_green_w0`: the Task 25
  record was missing. The real Task 35 synthetic collector/validator and the
  unchanged Task 36 manual register validated throughout.
- GREEN: the same command returned **4 passed** after the record and ledger
  updates. Negative evidence cases remain separate from predecessor fixtures.
- Phase 10 gate: `work/task25-venv/bin/python -m pytest tests/rust_contract/test_phase10*.py -q`
  returned **1335 passed**.
- `git diff --check` passed.
- ShellCheck was not required: no shell file changed.

No host access, installation, live acceptance or push occurred. Tasks 31/33
still need separately authorized signed installed evidence before W5.
