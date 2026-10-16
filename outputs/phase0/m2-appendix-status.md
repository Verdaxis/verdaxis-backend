# Sign-off M2 appendix status — final PM decisions applied, 2026-10-16

Base: `e61400f36ee528dfdb2476dd0dbaf2df457a2c54`.
Lane: `feat/phase0-m2-appendix`.
Appendix: `docs/runbooks/phase0-host-appendix.md`.

## Result

**M2 ready for review.** No remaining prerequisite.

The appendix is one finite, host-specific procedure. Every former open item is an
exact step, and the bounded scratch business smoke is now implemented, tested and
cited. The appendix's `## Open items` section states that no PM design decision
remains; review is not authorization to execute.

Applied decisions:

1. **Maintenance exception and order.** Only `GET /health/ready` and
   `GET /health/live` pass to `localhost:8000` through an explicit Caddy matcher;
   every other method and path keeps the 503 response, so the deploy helper's
   normal public readiness gate works with no `HEALTH_URL` bypass. Window order:
   deploy and backend acceptance under maintenance -> reopen the API from the
   captured `before/` Caddyfile -> ONE frontend workflow with
   `candidate_only=false` -> frontend smoke -> founder refresh note. No
   candidate-only pre-run. The cached-bundle interval starts at API reopening and
   the 10–15 minute workflow time counts inside the 60-minute hard limit.
2. **Final SHAs and bytes** are a window checklist (B1) with the `before/` hashes,
   exact restore/removal delta and `root:root 0644` plus `caddy`-readable proof,
   each with a stop condition.
3. **Rehearsal inputs.** The new adapter creates its own scratch-only identities
   and runs the business cases; schema/catalog preparation cites the bootstrap
   tool step; a pending restored-dump row is a STOP with a fresh dump; cutover
   checks both `dispatched_at` and `stream_seq` before and after the stop.
4. **Backup comparison** is pre-write and static in B4a: independent verify ->
   ACL acceptance -> `verdaxis-local-db-backup.service` with the new backend
   stopped -> scratch restore -> per-table counts checked against the verify
   report -> only then deploy and start, stopping on any mismatch.

## Bounded scratch business smoke

`scripts/phase0/scratch_business_smoke.py` is a small external HTTP client, not a
change to the Task 9 harness:

- `--base-url` accepts only `http://<loopback-ip>:<port>`; it refuses a public
  host, a production or staging host address, a non-loopback IP, `https`, a path,
  a query, embedded credentials, a privileged port, a host name and ports 8000
  and 8001, before any request. `localhost` is refused because only a literal
  address is accepted.
- `--fixture-environment-file` is a root-private 0600 one-line `DATABASE_URL=`
  file for the scratch cluster administrator; it refuses the live port 5432, any
  host that is not loopback, and any name other than the scratch target.
- It creates two clearly marked scratch organizations (a supplier and a separate
  buyer) with a supplier, buyer and admin user and random in-memory passwords it
  never writes to argv, stdout or the report. The buyer needs a separate
  organization, or the trade route refuses a self-trade.
- It runs 18 cases: login and `/api/auth/me`, inventory create, ASK and BID,
  trade hit/confirm/deliver/pay, notifications list and read, `/api/orderbook/my`
  for both roles, and copied public orderbook and prices reads, including a
  pre-existing copied order.
- It writes a JSON report of `case`, `status`, `result` and a bounded `detail`
  (a product detail code, never a body or a secret) and exits nonzero on failure.

The appendix A3 step cites the exact command. The Task 9 runner keeps its
disposable-target gate unchanged and is used only for the adapter's own
regression run.

## Test evidence

All tests ran through `verdaxis-prod-test`, one foreground call at a time, every
call completing. No local Python, build, Docker/Colima or direct SSH.

- End-to-end: `venv/bin/python -m tests.phase0.scratch_smoke_runner` against a
  disposable pinned container backend, with the adapter called as an external
  client: **18 cases verified, exit 0**. The runner reuses the Task 9 image,
  server entry point and ACL setup, then asserts the copied-order read path.
- URL/fixture refusal unit tests plus the appendix parser and runbook suites:
  **38 passed**, 1 warning, no failures. The refusal test covers a public host,
  the production and staging host addresses, a non-loopback IP, `https`, ports
  8000 and 8001, and more.
- The adapter's own earlier runs were red until each real product contract was
  met (ASK supplier details and specification standard, carbon-intensity method,
  separate buyer organization); the final run is green without weakening any
  assertion or the harness gate.
- `git diff --check`: passed.

Scope limits: parser and unit tests do not execute host commands, validate SQL
against a live schema, test BPF/Caddy/systemd/Vercel, or prove private
credentials. The end-to-end run proves the command sequence on synthetic
disposable data, not the production cutover. Those remain operator and
independent-review checks.

## Commits this task

- `8856bdd5` initial inventory-bound appendix and current interfaces.
- `b9719ae5` parser tests over the real CLI parsers.
- `742e1108` P0-5 rehearsal scope and resolved host decisions.
- `0d7bb804` P0-5 validation and remaining-gate status.
- `4c986a05` final maintenance/backup/promotion order.
- `7f96d7a1` test enforcement of the final order.
- `80b26cd8` M2 ready-for-review wording.
- `f2672971` bounded scratch business smoke adapter and its unit test.
- `88069432` disposable runner and fixture seed; A3 command and status update.

Nothing was pushed. No host was contacted. No acceptance/oracle document outside
this runbook lane was edited.
