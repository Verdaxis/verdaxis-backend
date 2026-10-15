#!/usr/bin/env bash
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
trap 'docker logs "$container"; docker rm -f "$container" >/dev/null' EXIT
for i in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE rust_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE ROLE rust_member LOGIN NOINHERIT PASSWORD 'disposable_member_pw';
CREATE ROLE rust_group NOLOGIN;
GRANT rust_group TO rust_member;
CREATE ROLE rust_migrator LOGIN NOINHERIT PASSWORD 'disposable_migrator_pw';
CREATE DATABASE verdaxis_rust_test OWNER rust_migrator;
\connect verdaxis_rust_test
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO rust_app, rust_member;
SET ROLE rust_migrator;
CREATE TABLE alembic_version(version_num varchar(64));
INSERT INTO alembic_version VALUES ('kycg_20261014_staging');
SQL
export RUST_TEST_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_MEMBER_DATABASE_URL="postgres://rust_member:disposable_member_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_MIGRATOR_DATABASE_URL="postgres://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/verdaxis_rust_test"
: "${RUST_TEST_PYTHON:?Set an absolute Python path with the project database dependencies}"
export RUST_TEST_PYTHON
export RELEASE_SHA=$(git rev-parse HEAD)
cargo test --manifest-path rs/Cargo.toml --locked --features contract-tests --test runtime_postgres --test admission_postgres -- --test-threads=1
