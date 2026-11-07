# Verdaxis Exchange backend

Verdaxis is a maritime fuel trading exchange. Rust is the only application
backend. The workspace in `rs/` builds `verdaxis-read`, which serves the API and
runs bounded maintenance, migration, monitoring and operator commands.

## Source map

- `rs/read/src/routes/`: HTTP adapters and authorization boundaries.
- `rs/read/src/`: trading, authentication, compliance, stream and storage code.
- `rs/read/src/cli/`: service, jobs, migration, ACL and monitor commands.
- `rs/read/migrations/1_baseline.sql`: the sqlx baseline.
- `rs/contract/`: retained Rust contract primitives and tests.
- `deploy/`: reviewed units, policy, Caddy configuration, host tooling and tests.
- `tests/tooling/` and `tests/monitor/`: host-tooling tests, not an API backend.

## Development and checks

Use an isolated checkout and a disposable database. Never point a test runner
at a deployed database or use a deployed listener for development.
The toolchain is pinned in `rs/rust-toolchain.toml`.

```bash
SQLX_OFFLINE=true cargo test --manifest-path rs/Cargo.toml --locked --workspace
cargo fmt --manifest-path rs/Cargo.toml --all -- --check
SQLX_OFFLINE=true cargo clippy --manifest-path rs/Cargo.toml --locked \
  --workspace --all-targets -- -D warnings
```

PostgreSQL runners under `rs/read/tests/` own their disposable database
containers. Their target guards and exact role assertions are part of the tests.
Read a runner before executing it. Local shared-build rules in the lane brief
take precedence over the generic commands above.

## Deployment

Both environments use systemd and a verified release binary. Production Rust
binds `127.0.0.1:8010`; staging binds `127.0.0.1:8011`. Caddy owns public routing.
The authoritative source policies are `deploy/rust/production-policy.json` and
`deploy/rust/staging-policy.json`.

`scripts/deploy.sh` verifies immutable source and signed artifact identity. It
uses native migration, ACL, runtime preparation and readiness commands, then
requires signed acceptance before clearing blocked startup state. CI does not
deploy. Host access and service activation require separate operator authority.

Before this release, the PM must install the M binary, adopt each database to
`sqlx_1`, transfer maintenance jobs to Rust, and stop the old Python service.
Drain active jobs before changing timer ownership. Use the approved live-window
runbook and current host instructions; this source change performs no host work.

## Compatibility files

`requirements.txt` and `constraints.txt` remain byte-identical for one release:
the installed M deploy helper rejects a release that changes them. They are not
backend CI inputs and are not a request to install an API environment.

`deploy/scripts/pre-m-bundle.sh` remains unchanged. It restores a captured old
release, including its Python service. Other retained operator helpers can have
legacy application imports; these are compatibility paths, not a second running
backend. Their disposition and gate results are recorded in
`outputs/ci-faster-results.md`. Do not run these paths against this checkout as
if it still contained the old application.
