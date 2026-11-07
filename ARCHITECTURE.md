# Architecture

## Backend

Rust is the sole application backend. The `rs/` Cargo workspace contains
`verdaxis-read` and the retained contract crate. `verdaxis-read` combines HTTP
serving with typed operator commands. Its CLI owns `serve`, `migrate`, `acl`,
`job`, `monitor`, `recover`, `ops`, `guard` and `canary`.

HTTP adapters live under `rs/read/src/routes/`. Domain modules hold trading,
authentication, compliance, KYC, market events, streams and storage behavior.
SQLx supplies PostgreSQL access. Financial calculations use exact decimal
values. Route adapters preserve scoped authorization and transaction boundaries;
job commands reuse the domain code rather than a separate application stack.

## Schema and runtime identity

`rs/read/migrations/1_baseline.sql` is the native migration baseline.
Adoption validates the expected catalog, owners and retained historical audit
row before publishing sqlx history. Runtime admission accepts `sqlx_1` and binds
the environment, full source SHA and verified binary identity. The historical
migration audit data remains in the database; it is not an executable migration
engine in this repository.

`deploy/rust/` holds release policy and frozen data. The public-read cache
requires a separately published authority record and matching database identity.
Revocation and drain precede replacement. Listener and pool budgets remain bound
to prepared runtime artifacts; this deletion does not change their wire format.

## Deployment and host automation

Production and staging run the verified binary under systemd on loopback ports
8010 and 8011 respectively. Caddy routes public requests. `scripts/deploy.sh`
verifies immutable source, artifact provenance and reviewed policy; it runs native
migration and ACL convergence, publishes runtime identity, and restarts Rust.
Blocked startup state remains until bound signed acceptance succeeds.

`scripts/install_systemd_units.sh` and
`deploy/scripts/install-host-automation.sh` install reviewed units and automation.
Maintenance commands under `rs/read/src/cli/jobs/` cover news, auth maintenance,
analytics pruning, reminders, onboarding and demo activity. Monitor commands and
alert handoff preserve existing journal and ownership boundaries.

The source Python API, its schema engine, API container and backend test suites
are removed. Python remains for retained system-interpreter host tooling and
its tests. Release compatibility files remain unchanged where the installed M
helper or pre-M restore depends on their exact behavior. In particular,
`requirements.txt`, `constraints.txt` and `deploy/scripts/pre-m-bundle.sh` are
not changed in this release. Legacy operator paths are listed in the gate report;
they do not imply that this checkout can start the old API.

## CI graph

CI uses standard free GitHub runners. Rust tests, release compilation and
PostgreSQL runner cells execute independently. Stable Cargo caches bind the
pinned toolchain, Cargo.lock and lane identity, not each source commit.
Publication waits for all remaining tests and validates the exact packaged
binary and archive digests before signing the same subjects as before.

The protected names `test` and `postgres-analytics` continue to report. The
former covers retained tooling; the latter aggregates Rust PostgreSQL cells.
The Python API requirements are not installed by normal CI. Tooling dependencies
are separate and bounded to retained script and monitor tests.

## Test ownership

`rs/read/tests/` and `rs/contract/tests/` retain independent Rust behavior tests.
Python-reference comparisons are removed; fixed expected Rust assertions remain
where the former comparison already provided them. PostgreSQL fixtures use the
native baseline and explicit SQL data, not the deleted application.
`rs/read/Cargo.toml` sets `autotests = false`: most test files are modules of
the `rs/read/tests/group_*.rs` binaries, so the crate is linked once per group,
and runners select a file with `--test group_<area> <file>::`. Files that must
be a crate root or own their process keep their own `[[test]]` target.
`scripts/check_test_groups.py` fails when a test file is in no binary.
`rs/read/tests/run_auth_streams_cache.sh` seeds and runs the auth, stream,
public-cache and alert targets whose database cases need their own variables.
`tests/tooling/` and `tests/monitor/` cover retained host tools. Disposable test
resources and temporary logs are not committed.

Historical design and rollout records under `docs/` describe their dated
checkpoints, not current application entry points. Current gate results and
remaining release compatibility work are in `outputs/ci-faster-results.md`.
