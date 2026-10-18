# Activation S2 source correction

The 19 October activation failed at sudo authority, repeat budget preparation,
and Python-dependent Rust prestarts. No live operations were performed here.
Policy stays off for a safe repair/bootstrap release. A later activation needs
separate approval, publication and exact unit installation.

## Contracts

- Reviewed sudoers sources live in `deploy/sudoers/`, separate by environment.
  Production grants backend restart/stop only; staging also grants exact single
  Rust restart/stop/disable. No wildcard or general sudo grant. Follow that
  directory's administrator-only installation procedure.
- Real deploy checks every exact command with `sudo -n -l` before any deploy
  state write/source mutation. Execution is also `sudo -n`, never a password
  prompt. Python and Rust restart separately; Python passes exact readiness first.
- Budget preparation publishes a private fsynced sibling by atomic rename under
  the deploy lock. Existing regular artifacts are replaced; output symlinks are
  refused. Same-release retry and a subsequent release are tested.
- Migration control remains inaccessible to the app role. Deploy's existing
  migrator-backed exact checkpoint helper verifies the live revision under the
  lock, then writes SHA/revision to private `revision.json`. The Rust unit uses
  stdlib `-I -S` prestarts to check the deploy guard and budget/revision identities.
  It loads neither `.env` nor application settings, imports no database driver,
  and receives no migrator secret. A manual out-of-band migration is outside this
  contract and requires a new canonical deployment; the record is not a live DB
  probe and must never be described as one.
- Budget environment drift is rechecked in the locked deploy control plane before
  restart; the long-running Rust unit does not receive that environment file.
- Rust-only start/readiness failure rechecks exact Python readiness at selected
  environment/full SHA. If ready, stop and disable Rust, retain Python and write
  durable `rust-failed` state. Exit remains nonzero. This state blocks future
  autonomous unit starts and jobs; a canonical same-SHA retry or forward release
  under the locks resolves it. Never clear state manually. If Python fails its
  exact check, retain the existing blocked-state and Python-stop behavior.

## Fixture blind spots and audit

The affected fixtures substitute process boundaries deliberately; none is proof
of host sudoers, installed units, credentials or startup:

- `tests/unit/test_rust_release.py`: `_write_fake_tools` previously made sudo and
  systemctl unconditionally successful and curl return synthetic readiness. Its
  unit is `/bin/true`; its Python wrapper suppresses real revision/migration
  helpers. It hid exact sudo command denial, unit prestart imports and joint
  restart failure. It initially built only one artifact per fixture and never
  tried a second deploy. Added denied-authority, Rust-start, Rust-readiness,
  repeated deploy and retry paths; scoped tool simulation remains intentional.
- `tests/unit/test_deploy_health.py`: fake sudo/systemctl/curl and Python/Alembic
  ledger wrappers likewise prove transaction ordering, not sudoers/systemd/DB
  behavior. Adjusted noninteractive argument handling; keep those boundaries
  explicit in result claims.
- `tests/unit/test_rust_units.py`: static prestart string assertions had accepted
  the settings-dependent revision command without executing it. New test runs
  every exact unit ExecStartPre, relocating only checkout paths/systemd variables,
  with no `.env` and isolated stdlib Python (no site packages/psycopg2). Existing
  bootstrap test uses real systemd-analyze with stub dependency units, not a real
  service start; it cannot prove ExecStartPre or sudo authorization.
- Other search matches were not alternate command-authority proof:
  `test_migration_checkpoint.py` mocks database heads and inspects deploy text;
  `test_assert_host_git_owner.py` mocks sudo Git argv for owner switching;
  `tests/monitor/test_runtime_identity.py` uses real optional sudo to nobody;
  `tests/monitor/test_systemd_units.py` and Phase 0 runbook/appendix and KYC-source
  tests inspect source/commands. No other fake systemd-analyze implementation
  was found in the searched tests. Installer tests in test_deploy_health mainly
  assert source boundaries, not a root installation on the real host.

## Production lane requirements

Copy exact-command preflight and noninteractive execution, reviewed minimal
per-environment sudoers, atomic private artifact replacement, isolated prestarts,
and separate restart/readiness phases. Optional Rust failure must never stop an
exact-ready Python API. Do not enable production Rust, widen database roles or
copy staging identities/unit names. Do not copy the existing production branch
mapping bug (`DEFAULT_BRANCH=staging`) as deployment authority: production lane
must resolve that separately before any live production use. Root installer
preflight and apply both require the fixed clean exact-SHA checkout.
