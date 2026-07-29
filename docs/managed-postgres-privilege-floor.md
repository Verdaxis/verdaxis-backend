# Managed PostgreSQL Privilege Floor

This document defines the minimum database capabilities Verdaxis requires
before staging or production may use a managed PostgreSQL provider. It does not
approve a provider or a cutover.

## Current Self-Hosted Contract

Verdaxis currently enforces:

- separate application, migrator, and read-only backup roles;
- exact database, `public` schema, application-object, column, sequence, and
  default ACLs;
- no membership edge involving a protected role;
- migrator ownership of the database, `public` schema, migration-control table,
  application tables, and application sequences;
- no runtime-app authority for undeclared tables, columns, or sequences; and
- PostgreSQL 17/PostGIS 3.6 with session advisory locks and a long-lived
  `LISTEN` connection.

The executable policy remains in `deploy/postgres/`. A managed target must not
silently weaken the application or backup roles to compensate for provider
ownership.

## Measured Hosted-Like Conflicts

`tests/postgres/test_hosted_privilege_envelope.py` reproduces a common hosted
privilege envelope on disposable PostgreSQL 17:

1. A non-superuser role with `CREATEROLE` receives an automatic membership with
   `ADMIN OPTION` when it creates another role. PostgreSQL 17 does not grant
   `SET ROLE` on that edge, but the edge still violates Verdaxis's exact
   zero-membership policy.
2. A role that does not own the database cannot transfer database ownership to
   the Verdaxis migrator.
3. A role that does not own `public` cannot transfer that schema to the
   Verdaxis migrator.

The test proves these PostgreSQL semantics only. It does not certify Supabase
roles, grants, network paths, pooling, extensions, backups, or support policy.
Those facts must be measured on an empty paid project.

## Required Provider Capabilities

A target is eligible for further rehearsal only when it provides:

- one isolated project for each environment;
- distinct credentials for runtime DML, migrations, and read-only backups;
- verified TLS on SQLAlchemy, Alembic, raw asyncpg listener, and operator
  `psql` paths;
- a direct session endpoint that preserves transaction and session advisory
  locks, prepared statements, and long-lived `LISTEN` connections;
- provider-supported PostGIS installation with a reviewed extension schema and
  `search_path`;
- enough DDL authority for Alembic to create, alter, and own every Verdaxis
  application table and sequence;
- exact governed-object and column grants, including no undeclared runtime
  writes and no backup writes;
- no browser-facing or customer-accessible provider role with commercial-table
  access;
- measured connection capacity and latency within the application budgets; and
- tested provider restore plus an independent off-provider logical backup.

A transaction pooler is not valid for migrations or the market-event listener.
It may be evaluated for ordinary request traffic only after the direct endpoint
works and the application identity checks support its connection format.

## Ownership Exceptions

Provider ownership of the database, `public`, or extension objects may be
accepted only after a real-project inventory shows all of the following:

1. Verdaxis retains exact ownership and ACL control over its application tables
   and sequences.
2. Provider-owned roles cannot become an application authentication or
   authorization path.
3. Data API, GraphQL, Realtime, browser keys, and automatic table grants are
   disabled or demonstrably isolated from commercial objects.
4. Every failed self-hosted invariant has an executable compensating check.
5. The exception and residual provider-operator authority receive explicit
   owner approval before staging traffic moves.

Do not modify `bootstrap_roles.sql` or `validate_roles.sql` merely to make a
provider pass. Add a target-specific governed-object attestation only after the
actual privilege inventory is reviewed.

## Stop Conditions

Stop the migration if the provider cannot guarantee verified TLS, direct
session semantics, application-object DDL control, least-privilege runtime and
backup roles, bounded connections, or a tested restore. Also stop if satisfying
the provider requires exposing commercial tables through a browser database
API or broadening the ordinary application role.

Supabase Auth is outside this database privilege decision and remains a
separately approved program.
