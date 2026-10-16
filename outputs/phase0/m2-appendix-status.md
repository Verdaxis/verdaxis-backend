# Sign-off M2 appendix status — 2026-10-16

Base: `e61400f36ee528dfdb2476dd0dbaf2df457a2c54`.
Lane: `feat/phase0-m2-appendix`.

## Result

Appendix: `docs/runbooks/phase0-host-appendix.md`.
Main runbook now has one current state, a dated historical integration note,
and the final production host-tool interfaces. No acceptance/oracle files were
edited. No host operation or production/staging SSH was performed.

**M2 remains open for execution.** A finite, fully executable appendix cannot
be derived from the supplied inventory alone. The appendix records the known
commands and order, but expressly stops rather than inventing missing host
facilities, script patch points, credentials or permissions. It is not an
approved start-to-end procedure yet.

## PM answers required

1. Scratch Linux/systemd facility, private cluster endpoint/storage, exact egress
   isolation and verification commands (not `17/main` or `17/test`).
2. Staging PM login/sudo authority and effective backup/monitor service units.
3. Complete Caddy/backend/drop-in/backup script bytes and reviewed replacements.
4. Release/archive/package/cache/mirror inputs, install/Settings launcher,
   authority-builder and target extension/refence commands.
5. Existing source/target roles' `pg_control_system()` EXECUTE and source SELECT
   authority without changing old database grants.
6. Both backup prefix/inventory patches, restore invocation/destination, demo
   identity keys and acceptance of no new monitor reader in Phase 0.
7. Protected Vercel rollback context, exact smoke allowlist and founder-controlled
   encrypted key-ring recovery commands.
8. Window/drain limits, signed acceptance and actual before-state timer inventory.

Decisions are retained: target-only CONNECT fence, no HBA edit/new live roles,
old DB untouched, UTC, pre-write-only rollback, pause staging monitor and backup
pull, preserve old backups, existing update banner plus founder refresh note.

## Completed tests

All tests ran through `verdaxis-prod-test`, in the foreground, and completed.
No local Python environment, build, Docker or Colima was used.

- Red: `tests/unit/test_phase0_appendix_commands.py`: **3 failed, 3 passed**
  (missing appendix/current-state contract).
- Final: `tests/unit/test_phase0_appendix_commands.py` plus
  `tests/unit/test_phase0_cutover_runbook.py`: **9 passed**, 0 failures/skips,
  0.59 seconds, wrapper exit 0.
- `git diff --check`: passed.

The new test extracts every repository Python module command in the appendix
and reconstructs its parser directly from the real `main` AST. It runs only
ArgumentParser/add_argument statements, never application imports, private
input loading or DB connections. It also extracts both repository shell-tool
commands and executes only their real argument-parser prefix, before Git,
source changes, secret loading or installation. Negative cases reject obsolete
bootstrap flags. Separate tool manifests and secret names are checked.

External system commands, inline fsync snippets, gh/Vercel and host configuration
are not proved by parser tests. Their host-specific execution still requires
PM answers and the full isolated Linux rehearsal. No claim of closing M2 is
made from green option-parser tests.
