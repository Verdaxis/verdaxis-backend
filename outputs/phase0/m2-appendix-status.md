# Sign-off M2 appendix status — final PM decisions applied, 2026-10-16

Base: `e61400f36ee528dfdb2476dd0dbaf2df457a2c54`.
Lane: `feat/phase0-m2-appendix`.
Appendix: `docs/runbooks/phase0-host-appendix.md`.
Tests: `tests/unit/test_phase0_appendix_commands.py` and
`tests/unit/test_phase0_cutover_runbook.py`.

## Result

**M2 ready for review.**

The appendix is one finite, host-specific procedure with no remaining PM design
question. Every former open item is now an exact step, and the four unresolved
choices from the previous revision are closed as PM decisions recorded in the
appendix's `## PM answers — final sequencing decisions` section.

Applied decisions:

1. **Maintenance exception and order.** The maintenance Caddyfile passes only
   `GET /health/ready` and `GET /health/live` to `localhost:8000` through an
   explicit Caddy matcher; every other method and path keeps the 503 response.
   The deploy helper's normal public readiness gate therefore works with no
   `HEALTH_URL` bypass. Window order is: deploy and backend acceptance under
   maintenance -> reopen the API from the captured `before/` Caddyfile -> ONE
   frontend workflow with `candidate_only=false` -> frontend smoke -> founder
   refresh note. No candidate-only run exists; it needs the new SHA live. The
   cached-bundle interval starts at API reopening. Workflow time (10–15 minutes)
   counts inside the 60-minute hard limit, with a 15-minute reservation and a
   re-close-not-rollback rule after reopening.
2. **Final SHAs and bytes** are a window checklist, not a design gap. B1 records
   the approved SHAs, every `before/` file hash, the exact restore/removal delta
   and Caddy `root:root 0644` plus `caddy`-readable proof, with a hash-verification
   stop condition.
3. **Rehearsal inputs.** Scratch business cases reuse the Task 9 HTTP harness
   business cases and its `http_seed.py` identity construction, with owner fixture
   SQL allowed only in scratch `verdaxis_prod`; schema/catalog preparation cites
   the bootstrap tool step; a pending outbox row in the restored dump is a STOP
   with a fresh dump, never a scratch drain; cutover checks both `dispatched_at`
   and `stream_seq` before and after the backend stop.
4. **Backup comparison** is pre-write and static: independent verify -> ACL
   acceptance -> `verdaxis-local-db-backup.service` with the new backend stopped
   -> scratch restore -> per-table counts checked against the verify report ->
   only then deploy and start. New B4a performs the comparison before B5/B7 and
   stops on any mismatch or unclassified table.

## One documented execution prerequisite

The Task 9 runner (`tests/phase0/http_runner.py`) creates its own disposable
container and `tests.disposable_server`, and the integration gate requires
`ENVIRONMENT=test` with an ephemeral port. It cannot be pointed at the
production-mode scratch backend on `127.0.0.1:8100`. The appendix names the
reusable business cases and fixture rules and requires a small reviewed adapter
(or an approved run of those cases against a copy inside the harness's own
container) before A3. This is a concrete-test artifact to build and review, not a
missing PM decision, and it does not change the appendix's command sequence. The
adapter must not disable the disposable-target gate or add a production-mode
target flag to the existing runner.

R1–R3 host-tool fixes and any PM-supplied CLI change remain prerequisites; the
appendix does not bypass tool refusals.

## Test evidence

All tests ran through `verdaxis-prod-test`, one foreground call at a time, with
every call completing. No local Python, build, Docker/Colima or direct SSH.

- Final: `tests/unit/test_phase0_appendix_commands.py` plus
  `tests/unit/test_phase0_cutover_runbook.py`: **13 passed**, zero failures/skips,
  0.61 seconds, wrapper exit 0, on `4c986a05`.
- Red-then-green sequence during this revision: final-order contract test failed
  first (1 failed, 10 passed), then passed after the appendix applied the PM
  sequencing; no test was weakened to pass.
- `git diff --check`: passed.

Test scope: it extracts every repository Python CLI command from the appendix and
reconstructs each real parser from its `main` AST, checking separate tool
manifest/secret names; it extracts both shell tools and runs only their parser
prefix; it checks every shell block with `bash -n`; it compiles every inline
operator Python snippet without executing it; and it enforces the P0-5 scope,
static backup ordering, health-only maintenance matcher and single-promotion
sequence. It does not execute host commands, validate SQL against live schemas,
test BPF/Caddy/systemd/Vercel, or prove private credentials and authority. Those
remain operator and independent-review checks; no runtime proof is claimed from
syntax tests.

## Commits this task

- `8856bdd5` initial inventory-bound appendix and current interfaces.
- `b9719ae5` parser tests over the real CLI parsers.
- `742e1108` P0-5 rehearsal scope and resolved host decisions.
- `0d7bb804` P0-5 validation and remaining-gate status.
- `4c986a05` final maintenance/backup/promotion order.
- `7f96d7a1` test enforcement of the final order.

Nothing was pushed. No host was contacted. No acceptance/oracle document outside
this runbook lane was edited.
