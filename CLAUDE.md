# Verdaxis backend instructions

## Application ownership

Rust (`rs/`) is the only backend. Do not add a Python application or API
comparison stack. Read current Rust source and the relevant project rules before
editing. Preserve user work and keep changes within the assigned branch.

## Runtime and release

- Production and staging use systemd, verified artifacts and loopback Rust
  listeners defined in `deploy/rust/*-policy.json`.
- `scripts/deploy.sh` is the deploy transaction. CI builds and signs artifacts;
  it never deploys to hosts.
- Host access, activation, Caddy reloads and timer transfers require explicit
  authorization. Read the current host instructions and live-window runbook
  before any authorized host action.
- Keep `requirements.txt` and `constraints.txt` byte-identical in this release.
  The installed M helper checks them against the next source release.
- Preserve other operator compatibility paths that could block the next deploy;
  report them rather than silently change their protocol.

## Coding rules

Use clear Rust types, checked arithmetic and explicit error boundaries. Keep
financial values exact. Preserve authorization, organization scope, transaction
ordering, row locks, bounded I/O and idempotent replay behavior. KYC cleanup
operations must remain available under their existing authentication and
ownership checks. Never add synthetic identity or KYC data to a live database.

## Verification

Run formatting, both workflow clippy commands, the whole locked workspace test
suite, affected disposable PostgreSQL runners, tooling and monitor tests, and
ShellCheck on the reviewed list. Report failures and skips honestly. Follow the
lane's shared Cargo target and lock rules; never create a per-worktree target.
Update `ARCHITECTURE.md` when structure or ownership changes. Do not commit
`.codesight/`, runtime secrets, test databases or temporary gate logs.
