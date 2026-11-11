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

## Agent model transport

`rs/read/src/agent/llm/` owns the async OpenCode Go chat-completions
transport. Its single model table permits `deepseek-v4.1-flash`. Credentials
come from three runtime environment slots; usage meters are cached per slot
for 60 seconds. The client selects available headroom, rotates after 429,
and disables unauthorized slots until restart. Bounded requests have no
redirects and return key-free attempt receipts, including failed attempts.

`ops prepare-runtime` projects optional serve-only `OPENCODE_API_KEY`,
`OPENCODE_API_KEY_2`, `OPENCODE_API_KEY_3`, `AGENT_ENABLED` and `AGENT_MODEL`.
The transport does not mount routes or enable the agent. Later Phase A tasks
own the decision contract, audit persistence, tools and route controls.
Local mock HTTP tests do not contact the provider.

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

The shared-host external monitor uses a separately pinned CI binary at
`/usr/local/libexec/verdaxis/verdaxis-read-monitor`. Its selective installer is
`deploy/external_monitor/install.sh`; the operator procedure is
`outputs/external-monitor-install.md`. It replaces only two service units and
never activates the timer or recovery. Python diagnosis and masked recovery
remain; the Rust `recover` command has no execution implementation. The earlier
Python external monitor source and its tests are removed.

The source Python API, its schema engine, API container and backend test suites
are removed. Python remains for retained system-interpreter host tooling and
its tests. The pre-M rollback bundle and its reader gate are gone with the
closed rollback window. `requirements.txt` and `constraints.txt` stay
byte-identical for the installed M deploy helper.

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
