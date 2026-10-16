# Sign-off M2 appendix status — PM answers applied, 2026-10-16

Base: `e61400f36ee528dfdb2476dd0dbaf2df457a2c54`.
Lane: `feat/phase0-m2-appendix`.
Appendix: `docs/runbooks/phase0-host-appendix.md`.

## Current result

Read the complete PM answers dated 16 October and applied P0-5. Task 10 now uses
scratch `17/p0rehearsal` on 5434, same OS, filtered transient systemd processes,
no provider credentials and new backend on 127.0.0.1:8100. No live/staging service
change during rehearsal. Fixed-path unit swapping and actual old→new→old
restoration are explicitly **not rehearsed**; P0-5 accepts that pilot risk and
requires syntax/dry-run review instead. No claim that localhost allowance
isolates rehearsal from live loopback ports: tool identity and scratch-only
credentials are required boundaries.

Added commands for constrained archive/wheel installation, source observations
and fingerprint, separate manifests/secrets, target-only CONNECT/settings,
transient backend, timer capture/drain, Caddy closure, final dump, tuple rename,
fence/fsync/permit, exact pre-write rollback block, backup/demo name edits,
restorecheck database and recorded-active timer restoration. The main runbook
now reflects the narrower current rehearsal and post-reopen key-ring decision.

30-minute target/60-minute hard limit; founder announces/approves/recommends
refresh. Key-ring work occurs after reopening with founder present. Source
privileges are checked facts, not grounds for new live grants. Staging backup
script remains unchanged; local prefix changes as PM directed. No new monitor
reader is installed. No mirror, separate OS, forced reload or live business
smoke is introduced.

**M2 remains open at specific conflicts, not the former eight unanswered questions.**

## Remaining open items

1. All-path Caddy 503 prevents the normal deploy helper's fixed public readiness
   gate and frontend promotion smoke. Candidate-only frontend validation also
   requires the named new backend SHA before the window. PM must resolve the
   exception/sequence explicitly; no ambient HEALTH_URL bypass works.
2. Record final SHAs and review actual Caddy/backend/drop-in bytes, including the
   exact restore/removal delta and Caddy read permissions. These are private
   window inputs, not missing OS/facility decisions.
3. Select approved scratch business-smoke/fixture, confirm source catalog and
   target schema/default ACL preparation, and approve a scratch-only drain if
   the restored dump has pending outbox events. B1/B2 independent review pending.
4. Choose a consistent-snapshot or pre-write backup/count comparison boundary;
   comparing restored counts with a changing live database is not restore proof.

No actual host command was run. No source/acceptance/oracle document outside the
runbook lane was changed. No push. The report does not claim permission to begin
cutover with these open items.

## Completed tests

All tests used `verdaxis-prod-test`, one foreground call at a time. Every call
finished. No local Python execution, build, Docker/Colima or direct SSH.

- P0-5 red regression: **1 failed, 8 passed**, missing updated scope/open-items.
- Intermediate parser run: **1 failed, 10 passed**; new ACL parser required its
  literal `_IDENTITIES` choices in the AST-only test namespace. Fixed test support
  without importing application code or opening a connection.
- Final parser/runbook/syntax run: **12 passed, zero failures/skips**, 0.84 seconds,
  wrapper exit 0.
- `git diff --check`: passed.

Command:

```sh
verdaxis-prod-test "$PWD" 'ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars DATABASE_URL="sqlite+aiosqlite:///:memory:" venv/bin/python -m pytest tests/unit/test_phase0_appendix_commands.py tests/unit/test_phase0_cutover_runbook.py -q --tb=short'
```

Tests extract repository Python CLI commands and build their real parsers from
AST parser statements only; shell tools use only their parser prefix. They check
all shell blocks with `bash -n`, compile inline Python without executing it,
check separate tool manifest/secret names, and enforce P0-5/current-state text.
They do not validate SQL against live schemas, execute root modifications, test
BPF/Caddy/systemd/Vercel, or prove private credentials/authority. Those remain
operator/review checks. No false runtime proof is claimed from syntax tests.

## History (superseded)

Before PM answers, commits `8856bdd5` and `b9719ae5` recorded eight PM questions
and 9 passing parser/runbook tests. The separate-OS requirement and key recovery
before reopening from that draft are superseded by P0-5/Q7. Commit `742e1108`
applies the new scope and command sequence; this status and test extension
record the final follow-up.
