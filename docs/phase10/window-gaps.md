# Release window entry points

Baseline: `b36279c328a6a193093965f71e8e1a9876618d71`. Local source review only;
no installed-host claim. Staging combines W2/W3; production combines W2/W4.
Every route group, including production CRUD, moves to Rust. Python remains
installed, stopped and disabled through soak. W5 is separate.

## Missing at the baseline

| Window | Missing executable step | Source evidence |
|---|---|---|
| W2, W4 | Ordered activation after immutable unit installation; record active timers, stop/drain/fence direct launchers, run each unit once, accept each group and provide its rollback | `scripts/install_systemd_units.sh:3,23-25,61-67`; `deploy/scripts/install-host-automation.sh:2`; installer deliberately does not activate |
| W2, W4 | Alert ownership transfer in both directions, including pending/uncertain journal reconciliation and direct-launcher fences | `deploy/monitor/README.md:201-219`; `deploy/tests/alert-handoff-contract.sh:2-18`; individual alert commands are not a window driver |
| W2, W4 | Verify retired recovery remains inactive and replacement restart/watchdog controls are installed | `tests/rust_contract/phase10/removal-ledger.json:74-88` retires automatic recovery; `rs/read/src/cli/mod.rs:274-279,674-678` retains unavailable argument parsing, not an activation target. Verify the recorded service-manager replacement, never start the retained Python recovery |
| W3, W4 | Source-bound adoption and repeat-deploy policy rows | `deploy/migration-checkpoints.tsv:63-64` ends at the offboarding self-transition; `rs/read/src/cli/migrate.rs:487-532` requires an allowlisted pair and does not adopt on an Alembic self-transition |
| W3, W4 | M startup identity: production literal, artifact list and compiled drift assertion | `rs/read/src/runtime/config.rs:11-20`; `rs/read/src/runtime/artifact.rs:23-34`; `deploy/rust/allowed-revisions.json`: existing lists name Alembic checkpoints, not `sqlx_1` |
| W3, W4 | Baseline/run integration under both locks, blocked state and exact approval; retained startup controls without a Python restart | `rs/read/src/cli/migrate.rs:538-598,692-707`; `scripts/deploy.sh:1008-1011,1238-1258` still requires and restarts the Python service |
| W3, W4 | Complete private pre-M capture, verification and restore; actual old executable startup, not identity-only rollback; interruption boundaries for every publication | `outputs/phase10-integration-2026-11-05.md:84-89,104-105,118-131`; existing deploy installs a forward artifact, with no pre-M bundle entry point |
| W3, W4 | All-groups Caddy candidate, validation, activation and per-group probes from saved live bytes | `scripts/rust_caddy_candidate.py:2,10-15,434-445` supports one group; `scripts/rust_phase5_candidate.py:6-12,110-123` supports Phase 5 only; neither activates the complete site |
| W3, W4 | Bound window evidence for exact readiness, session/auth, passive harm and each group before timers/state can be released | `outputs/phase10-integration-2026-11-05.md:103-112,129-131`; existing component fixtures do not establish window acceptance |

## Reuse, not replacement

- **Task 6:** `verdaxis-read migrate status`, `migrate baseline` and `migrate run`
  with exact environment, full SHA, current/target, environment file and deploy
  nonce. Keep its checksum, fingerprint, role and both-lock checks.
  Database fixture entry: `rs/read/tests/run_postgres.sh:246-261,287-308`;
  control ACL fixture: `bash deploy/tests/sqlx-control-acl.sh`.
- **Task 7:** retain `scripts/deploy.sh` canonical path/branch, clean source,
  immutable archive, exact approvals, artifact provenance, budget authority,
  cache fences, ACL convergence, durable state and exact-ready checks.
  `bash deploy/tests/deploy-transaction.sh` is its shell-stub contract, not
  real systemd/database acceptance (`:2-20,30-32`).
- **Task 10:** `sudo -n scripts/install_systemd_units.sh --dry-run|--apply
  --environment ENV --source-ref FULL_SHA --binary-sha256 DIGEST`; installation
  stays separate from activation. Reuse `deploy/scripts/install-host-automation.sh`.
  Static contract: `bash deploy/tests/units-contract.sh --static`. The complete
  contract requires a Linux ELF through `VERDAXIS_READ_BIN` (`:252-255,315`).
- **Task 28:** `verdaxis-read monitor alert --reconcile`, `--rearm`, `--repair`
  and `--release CHECK`; release only after producers stop and journals drain.
  Reuse `bash deploy/tests/alert-handoff-contract.sh`; the
  `--disposable-identities` suite is Linux-only and is not local Mac evidence.
- **Task 30:** reuse the recorded retirement contract: verify old recovery and
  diagnosis cannot start, and verify exact installed restart drop-ins and Rust
  watchdog controls. No native recovery command is required by that disposition.
  Argument parsing is not implementation or authority to reactivate recovery.
- **Caddy:** reuse both candidate modules above. Preserve exact saved rollback
  bytes, already-live abilities/offboarding, preauth ordering and environment
  differences. Do not treat a fixture site as installed inventory.

## Implementation and acceptance order

1. Pin M identity and source-bound transitions with failing contracts first.
2. Complete the Python-free deploy transaction without removing standing gates.
3. Add protected bundle capture/verify/restore with publication kill points.
4. Add complete Caddy tooling and ordered W2 transfer with group rollback.
5. Add every new file to the removal ledger and every changed shell entry point
   to the ShellCheck list. Review and commit each complete step locally.

Task 31 later composes these entry points in a disposable Linux/systemd target.
It must exercise real identities/ACLs, signed artifacts, actual old-binary start,
all interruption boundaries, observed timers, route ownership and rollback.
No live window or W5 deletion is authorized by this file.
