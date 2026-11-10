#!/usr/bin/env bash
# Run the auth, stream, public-cache and alert PostgreSQL targets on one
# disposable PostgreSQL container.
#
# These targets read their database URLs from the environment. Their database
# cases are marked `#[ignore]`, or they return early as a pass when the URL is
# missing, so the workspace runners never exercise them. This runner builds the
# two databases they expect from the SQL baseline with the deployed role split,
# seeds each target's fixture rows as the schema owner, and runs each target
# with its own filters, `--ignored`/`--include-ignored` flags and variables.
# Every run must pass exactly the expected number of tests and ignore none.
#
# Docker runs locally through Colima. The container binds a Docker-assigned
# loopback port and is removed on exit. Never point this at staging or prod.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'

export SQLX_OFFLINE=true
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
export ENVIRONMENT="${ENVIRONMENT:-test}"
export JWT_SECRET="${JWT_SECRET:-test-secret-key-for-testing-minimum-32-chars}"

# The targets name these roles and databases: auth cases assert that they run
# as verdaxis_app_test, and the cache cases use two separate incarnations.
analytics_database=verdaxis_analytics_test
market_database=verdaxis_market_integrity_test
app_role=verdaxis_app_test
migrator_role=verdaxis_migrator_test
backup_role=verdaxis_backup_test
password=disposable_pw

container=''
run_log=''
cleanup() {
  # An EXIT trap replaces the script's exit status with the status of its own
  # last command. Capture the real status first and restore it afterwards.
  local status=$?
  if [[ -n "$container" ]]; then
    if (( status != 0 )); then docker logs "$container" || true; fi
    # -v removes the anonymous data volume; a forced remove skips the --rm cleanup.
    docker rm -f -v "$container" >/dev/null || true
  fi
  if [[ -n "$run_log" ]]; then rm -f "$run_log"; fi
  exit "$status"
}
trap cleanup EXIT

container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD="$password" \
  -p 127.0.0.1::5432 "$image")
ready=0
for _ in {1..120}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[[ "$ready" == 1 ]] || { echo 'database did not become ready' >&2; exit 2; }
port=$(docker port "$container" 5432/tcp)
port=${port##*:}

# Run SQL from standard input in one transaction, as the superuser or as the
# schema owner. Fixture rows are always written by the owner, never the app.
admin_sql() {
  docker exec -i "$container" psql -X -q -U postgres -d "$1" -v ON_ERROR_STOP=1 --single-transaction
}
owner_sql() {
  docker exec -i "$container" psql -X -q -U "$migrator_role" -d "$1" -v ON_ERROR_STOP=1 --single-transaction
}

database_url() {
  local role=$1 database=$2
  printf 'postgresql://%s:%s@127.0.0.1:%s/%s' "$role" "$password" "$port" "$database"
}
app_url=$(database_url "$app_role" "$analytics_database")
owner_url=$(database_url "$migrator_role" "$analytics_database")
admin_url=$(database_url postgres "$analytics_database")
market_owner_url=$(database_url "$migrator_role" "$market_database")

# Apply the deployed role policy: roles, database ACLs and, through its
# include, the runtime object ACL convergence.
docker exec "$container" mkdir -p /tmp/role-policy
docker cp deploy/postgres/. "$container":/tmp/role-policy/ >/dev/null
apply_role_policy() {
  local database=$1 policy=$2
  docker exec "$container" psql -X -q -U postgres -d "$database" -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role="$app_role" \
    -v migrator_role="$migrator_role" -v backup_role="$backup_role" \
    -f "/tmp/role-policy/$policy" >/dev/null
}

# The image's init script loads extra PostGIS extensions into POSTGRES_DB, and
# the baseline fingerprint covers extensions, so both databases are created
# here. CREATE DATABASE cannot run inside the transaction that admin_sql opens.
for database in "$analytics_database" "$market_database"; do
  docker exec "$container" psql -X -q -U postgres -v ON_ERROR_STOP=1 -c "CREATE DATABASE $database"
  admin_sql "$database" <<'SQL'
CREATE EXTENSION IF NOT EXISTS postgis;
SQL
  apply_role_policy "$database" bootstrap_roles.sql
done
admin_sql postgres <<SQL
ALTER ROLE $app_role PASSWORD '$password';
ALTER ROLE $migrator_role PASSWORD '$password';
ALTER ROLE $backup_role PASSWORD '$password';
SQL
# Build each schema from the SQL baseline as the migrator, then converge the
# runtime ACLs on the new objects.
for database in "$analytics_database" "$market_database"; do
  RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="$(database_url "$migrator_role" "$database")" \
    cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test group_phase10 \
    phase10_migrate_postgres::runner_database_adopts_to_the_baseline -- --ignored
  apply_role_policy "$database" bootstrap_roles.sql
done

run_log=$(mktemp "${TMPDIR:-/tmp}/rust-auth-streams-cache.XXXXXX")
summary=()

# Usage: run_cargo_test LABEL EXPECTED_PASSED [NAME=VALUE ...] -- CARGO_TEST_ARGS...
# Runs one `cargo test` for verdaxis-read with the given variables. The run must
# exit 0, pass exactly EXPECTED_PASSED tests and report no ignored test, so a
# renamed case, a lost filter or a missing `--ignored` flag fails here.
run_cargo_test() {
  local label=$1 expected=$2
  shift 2
  local assignments=()
  while [[ $1 != -- ]]; do
    assignments+=("$1")
    shift
  done
  shift
  printf '\n== %s\n' "$label"
  set +e
  env "${assignments[@]}" cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read "$@" \
    2>&1 | tee "$run_log"
  local status=${PIPESTATUS[0]}
  set -e
  if (( status != 0 )); then
    echo "$label: cargo test exited $status" >&2
    exit "$status"
  fi
  local passed
  passed=$(awk '/^test result: /{sum += $4} END{print sum + 0}' "$run_log")
  if (( passed != expected )); then
    echo "$label: expected $expected passed tests, got $passed" >&2
    exit 1
  fi
  if grep -E '^test result: ' "$run_log" | grep -Ev ' 0 failed; 0 ignored;' >/dev/null; then
    echo "$label: a test failed or was ignored" >&2
    exit 1
  fi
  summary+=("$label: $passed passed")
}

# Fail unless the last run printed this exact text, such as a coverage line.
require_output() {
  if ! grep -F -- "$1" "$run_log" >/dev/null; then
    echo "missing expected test output: $1" >&2
    exit 1
  fi
}

# Empty the tables that account and organization fixtures share, as each
# seeded suite did before it wrote its own rows.
truncate_account_tables() {
  owner_sql "$1" <<'SQL'
TRUNCATE TABLE commissions, orders, public_listings, trades, orderbook_orders,
  users, organizations, products, delivery_points RESTART IDENTITY CASCADE;
SQL
}

# Empty the outbox and reset the replay barrier. The deployed row trigger
# refuses deleting unsequenced rows, so only this owner transaction disables it;
# application and prune paths never bypass it.
clear_retention() {
  owner_sql "$analytics_database" <<'SQL'
ALTER TABLE public.market_event_outbox DISABLE TRIGGER trg_market_event_outbox_replay_floor;
DELETE FROM public.market_event_outbox;
ALTER TABLE public.market_event_outbox ENABLE ALWAYS TRIGGER trg_market_event_outbox_replay_floor;
DELETE FROM public.market_event_replay_state;
INSERT INTO public.market_event_replay_state (singleton, discarded_through_seq) VALUES (true, 0);
SQL
}

# Record a drained rust_active retention owner, so the deployed trigger lets
# the owner delete sequenced outbox rows, then clear the retained history.
reset_retention_ownership() {
  owner_sql "$analytics_database" <<'SQL'
DELETE FROM public.market_event_retention_control;
INSERT INTO public.market_event_retention_control
  (singleton, mode, ownership_epoch, environment, release_sha, selector_config_digest, drain_completed_at)
VALUES (true, 'rust_active', 1, 'test', repeat('a', 40), repeat('b', 64), now());
SQL
  clear_retention
}

# Usage: insert_outbox_event SEQUENCE PARTICIPANT_ORG [PAYLOAD_JSON]
insert_outbox_event() {
  local sequence=$1 participant=$2 payload=${3:-'{"schema_version": 1}'}
  owner_sql "$analytics_database" <<SQL
INSERT INTO public.market_event_outbox
  (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload, created_at, stream_seq)
SELECT event_id, 'trade_created', 'trade', event_id::text,
       CAST('["$participant"]' AS json), CAST('$payload' AS json), now(), $sequence
FROM (SELECT gen_random_uuid() AS event_id) AS new_event;
SQL
}

# --- Alerts -------------------------------------------------------------------

alert_org=00000000-0000-4000-8000-000000000201
alert_foreign_org=00000000-0000-4000-8000-000000000202
alert_product=00000000-0000-4000-8000-000000000203
alert_point=00000000-0000-4000-8000-000000000204
alert_actors=(
  00000000-0000-4000-8000-000000000211
  00000000-0000-4000-8000-000000000212
  00000000-0000-4000-8000-000000000213
  00000000-0000-4000-8000-000000000214
)
truncate_account_tables "$analytics_database"
owner_sql "$analytics_database" <<SQL
INSERT INTO organizations (id, name, type) VALUES
  ('$alert_org', 'Alert component owner', 'FUEL_BUYER'),
  ('$alert_foreign_org', 'Alert foreign owner', 'FUEL_BUYER');
INSERT INTO products (id, name, fuel_type, fuel_grade, is_active)
  VALUES ('$alert_product', 'Alert legacy fixture', 'TEST', 'TEST', false);
INSERT INTO delivery_points (id, name, region, is_active)
  VALUES ('$alert_point', 'Alert fixture point', 'TEST', false);
INSERT INTO users (id, email, password_hash, role, status, organization_id, email_verified, must_change_password)
SELECT actor.id, 'alert-' || actor.id || '@example.test', 'unused', 'BUYER', 'APPROVED',
       actor.organization_id, true, false
FROM (VALUES ('${alert_actors[0]}'::uuid, '$alert_org'::uuid),
             ('${alert_actors[1]}'::uuid, '$alert_org'::uuid),
             ('${alert_actors[2]}'::uuid, '$alert_foreign_org'::uuid),
             ('${alert_actors[3]}'::uuid, NULL::uuid)) AS actor(id, organization_id);
SQL
alert_actor_list=$(IFS=,; printf '%s' "${alert_actors[*]}")
run_cargo_test crud_alerts 1 \
  ALERT_TEST_DATABASE_URL="$app_url" ALERT_TEST_ORGANIZATION="$alert_org" \
  ALERT_TEST_FOREIGN="$alert_foreign_org" ALERT_TEST_PRODUCT="$alert_product" \
  ALERT_TEST_POINT="$alert_point" ALERT_TEST_ACTORS="$alert_actor_list" \
  -- --test group_crud crud_alerts::postgres_crud -- --nocapture

# --- Auth ---------------------------------------------------------------------

auth_store_env=(AUTH_STORE_TEST_DATABASE_URL="$app_url" AUTH_STORE_TEST_OWNER_URL="$owner_url")
auth_email_env=(AUTH_EMAIL_TEST_DATABASE_URL="$app_url" AUTH_EMAIL_TEST_OWNER_URL="$owner_url")
run_cargo_test auth_revocation_postgres 5 "${auth_store_env[@]}" \
  -- --test group_auth auth_revocation_postgres:: -- --include-ignored
run_cargo_test auth_store_postgres 12 "${auth_store_env[@]}" \
  -- --test group_auth auth_store_postgres:: -- --include-ignored
run_cargo_test auth_email_verification_postgres 6 "${auth_email_env[@]}" \
  -- --features contract-tests --test auth_email_verification_postgres -- --include-ignored
run_cargo_test auth_invitation_accept_postgres 8 "${auth_email_env[@]}" \
  -- --features contract-tests --test auth_invitation_accept_postgres -- --include-ignored --test-threads=1
run_cargo_test auth_login_postgres 7 \
  AUTH_LOGIN_DATABASE_URL="$app_url" AUTH_LOGIN_OWNER_URL="$owner_url" \
  -- --features contract-tests --test group_auth auth_login_postgres:: -- --include-ignored --test-threads=1
run_cargo_test auth_password_reset_postgres 4 "${auth_email_env[@]}" \
  -- --features contract-tests --test auth_password_reset_postgres -- --include-ignored
# The race schedules depend on interleaving, so they repeat three times.
for attempt in 1 2 3; do
  run_cargo_test "auth_races_postgres and auth_coexistence_postgres, run $attempt" 16 "${auth_email_env[@]}" \
    -- --features contract-tests --test group_auth \
    -- auth_races_postgres:: auth_coexistence_postgres:: --include-ignored --test-threads=1
done
run_cargo_test auth_refresh_postgres 7 \
  AUTH_REFRESH_DATABASE_URL="$app_url" AUTH_REFRESH_OWNER_URL="$owner_url" \
  -- --features contract-tests --test auth_refresh_postgres -- --include-ignored --test-threads=1
run_cargo_test auth_registration_postgres 13 "${auth_email_env[@]}" \
  -- --features contract-tests --test auth_registration_postgres -- --include-ignored --test-threads=1
run_cargo_test auth_password_change 1 "${auth_store_env[@]}" \
  -- --test auth_password_change -- --ignored
run_cargo_test auth_stream 1 "${auth_store_env[@]}" \
  -- --test auth_stream -- --ignored

# --- Authenticated transactions -------------------------------------------------

# These cases share one seeded account. Its id is fixed; the auth cases move it
# onto the subject of their signed fixture token. Reseed before each target so
# no target depends on the state that another one left.
interop_env=(AUTH_INTEROP_APP_URL="$app_url" AUTH_INTEROP_ADMIN_URL="$owner_url" REQUIRE_AUTH_INTEROP=1)
seed_interop_account() {
  truncate_account_tables "$analytics_database"
  owner_sql "$analytics_database" <<'SQL'
INSERT INTO users (id, email, password_hash, role, status, email_verified, must_change_password)
VALUES ('20000000-0000-4000-8000-000000000001', 'interop@example.test', 'unused', 'BUYER',
        'APPROVED', false, false);
SQL
}
seed_interop_account
run_cargo_test crud_watchlist_reads 1 "${interop_env[@]}" \
  -- --test group_crud crud_watchlist_reads::database_detail_is_actor_scoped_and_does_not_mutate_rows
seed_interop_account
run_cargo_test 'auth_transactions and auth_transaction_panic' 5 "${interop_env[@]}" \
  -- --test group_auth -- auth_transactions:: auth_transaction_panic:: --test-threads=1
seed_interop_account
run_cargo_test phase5_authenticated_boundary 1 "${interop_env[@]}" \
  -- --test phase5_authenticated_boundary phase5_transaction_cases_scoped_actor_read

# --- Stream admission -----------------------------------------------------------

# A fixed issuance instant, 1792497600.123456 seconds, so that password-change
# and revocation cutoffs can be seeded at exact microseconds around it.
stream_issued_at_us=1792497600123456
stream_issued_at="to_timestamp(1792497600) + interval '123456 microseconds'"
stream_before_issue="to_timestamp(1792497600) + interval '123455 microseconds'"
stream_approved_org=00000000-0000-4000-8000-000000000301
stream_other_org=00000000-0000-4000-8000-000000000302
stream_pending_org=00000000-0000-4000-8000-000000000303
stream_auth_env=(
  STREAM_AUTH_TEST_DATABASE_URL="$app_url"
  STREAM_AUTH_TEST_LOCK_DATABASE_URL="$owner_url"
  STREAM_AUTH_TEST_ISSUED_AT_US="$stream_issued_at_us"
  STREAM_AUTH_TEST_APPROVED_ORG="$stream_approved_org"
  STREAM_AUTH_TEST_OTHER_ORG="$stream_other_org"
  STREAM_AUTH_TEST_PENDING_ORG="$stream_pending_org"
  STREAM_AUTH_TEST_MISSING_USER=00000000-0000-4000-8000-000000000399
)
# One row per seeded user: name, identifier, organization, status, must change
# password, KYC status, password changed at, authentication revoked at.
stream_users="
APPROVED        00000000-0000-4000-8000-000000000311 $stream_approved_org APPROVED false PENDING  NULL NULL
KYC_REJECTED    00000000-0000-4000-8000-000000000312 $stream_approved_org APPROVED false REJECTED NULL NULL
REJECTED        00000000-0000-4000-8000-000000000313 $stream_approved_org REJECTED false PENDING  NULL NULL
PENDING         00000000-0000-4000-8000-000000000314 $stream_approved_org PENDING  false PENDING  NULL NULL
MUST_CHANGE     00000000-0000-4000-8000-000000000315 $stream_approved_org APPROVED true  PENDING  NULL NULL
OTHER_ORG       00000000-0000-4000-8000-000000000316 $stream_other_org    APPROVED false PENDING  NULL NULL
MOVED           00000000-0000-4000-8000-000000000317 $stream_other_org    APPROVED false PENDING  NULL NULL
NO_ORGANIZATION 00000000-0000-4000-8000-000000000318 NULL                 APPROVED false PENDING  NULL NULL
CUTOFF_EQUAL    00000000-0000-4000-8000-000000000319 $stream_approved_org APPROVED false PENDING  ISSUED NULL
CUTOFF_BEFORE   00000000-0000-4000-8000-000000000320 $stream_approved_org APPROVED false PENDING  BEFORE NULL
REVOKED         00000000-0000-4000-8000-000000000321 $stream_approved_org APPROVED false PENDING  NULL ISSUED
UNAPPROVED_ORG  00000000-0000-4000-8000-000000000322 $stream_pending_org  APPROVED false PENDING  NULL NULL
"
# Render one optional seeded value as SQL: NULL, an identifier or an instant.
sql_value() {
  case $1 in
    NULL) printf 'NULL' ;;
    ISSUED) printf '%s' "$stream_issued_at" ;;
    BEFORE) printf '%s' "$stream_before_issue" ;;
    *) printf "'%s'" "$1" ;;
  esac
}
stream_user_rows=()
while read -r name user organization status must_change kyc changed revoked; do
  [[ -n "$name" ]] || continue
  stream_auth_env+=("STREAM_AUTH_TEST_${name}_USER=$user")
  stream_user_rows+=("('$user', 'stream-admission-$user@example.test', 'unused', 'BUYER', '$status', $(sql_value "$organization"), true, $must_change, '$kyc', $(sql_value "$changed"), $(sql_value "$revoked"))")
done <<< "$stream_users"
truncate_account_tables "$analytics_database"
owner_sql "$analytics_database" <<SQL
INSERT INTO organizations (id, name, type, verification_status) VALUES
  ('$stream_approved_org', 'Stream admission approved', 'FUEL_BUYER', 'APPROVED'),
  ('$stream_other_org', 'Stream admission second approved', 'FUEL_BUYER', 'APPROVED'),
  ('$stream_pending_org', 'Stream admission pending', 'FUEL_BUYER', 'PENDING');
INSERT INTO users (id, email, password_hash, role, status, organization_id, email_verified,
  must_change_password, kyc_status, password_changed_at, authentication_revoked_at)
VALUES $(IFS=,; printf '%s' "${stream_user_rows[*]}");
SQL
run_cargo_test streams_auth 1 "${stream_auth_env[@]}" \
  -- --features contract-tests --test group_streams -- --ignored --exact streams_auth::postgres_admission --nocapture
# Four admitted, nine refused and one unavailable store read.
require_output 'stream admission postgres coverage: 14 checks'
# Transaction-local bounds, the lock bound, recovery after it, the acquisition
# deadline and two pooled reads that return their connection.
require_output 'stream read bounds postgres coverage: 5 checks'

# --- Stream dispatch ------------------------------------------------------------

# Each case starts from an empty outbox; the first needs only the app role.
clear_retention
run_cargo_test 'streams_dispatch committed_visibility_and_rollback' 1 STREAM_DISPATCH_APP_URL="$app_url" \
  -- --features contract-tests --test group_streams streams_dispatch::postgres::committed_visibility_and_rollback \
  -- --exact --ignored --test-threads=1
for case_name in retention_snapshot_resets_existing_subscribers \
  legacy_event_bytes_below_limit legacy_event_bytes_at_limit legacy_event_bytes_above_limit \
  legacy_page_bytes_below_limit legacy_page_bytes_at_limit legacy_page_bytes_above_limit; do
  clear_retention
  run_cargo_test "streams_dispatch $case_name" 1 \
    STREAM_DISPATCH_APP_URL="$app_url" STREAM_DISPATCH_OWNER_URL="$owner_url" \
    -- --features contract-tests --test group_streams "streams_dispatch::postgres::$case_name" \
    -- --exact --ignored --test-threads=1
done

# --- Stream election ------------------------------------------------------------

# assignment_waits_for_shared_row needs a row that another session updates when
# the sequencer assigns its marked outbox row.
owner_sql "$analytics_database" <<SQL
CREATE TABLE phase6_election_shared_row (singleton boolean PRIMARY KEY, generation bigint);
INSERT INTO phase6_election_shared_row VALUES (true, 0);
GRANT SELECT, UPDATE ON phase6_election_shared_row TO $app_role;
CREATE FUNCTION phase6_election_shared_row_update() RETURNS trigger
LANGUAGE plpgsql AS \$\$ BEGIN
    UPDATE phase6_election_shared_row SET generation = generation + 1;
    RETURN NEW;
END \$\$;
CREATE TRIGGER phase6_election_shared_row_update
AFTER UPDATE ON market_event_outbox FOR EACH ROW
WHEN (NEW.aggregate_id = 'assignment_waits_for_shared_row')
EXECUTE FUNCTION phase6_election_shared_row_update();
SQL
run_cargo_test streams_election 19 STREAM_ELECTION_APP_URL="$app_url" \
  -- --features contract-tests --test group_streams streams_election:: -- --include-ignored --test-threads=1
owner_sql "$analytics_database" <<'SQL'
DROP TRIGGER phase6_election_shared_row_update ON market_event_outbox;
DROP FUNCTION phase6_election_shared_row_update();
DROP TABLE phase6_election_shared_row;
SQL

# --- Stream HTTP route ----------------------------------------------------------

http_org=00000000-0000-4000-8000-000000000401
http_other_org=00000000-0000-4000-8000-000000000402
http_pending_org=00000000-0000-4000-8000-000000000403
http_member=00000000-0000-4000-8000-000000000411
http_pending_member=00000000-0000-4000-8000-000000000412
truncate_account_tables "$analytics_database"
owner_sql "$analytics_database" <<SQL
INSERT INTO organizations (id, name, type, verification_status) VALUES
  ('$http_org', 'Stream fixture $http_org', 'FUEL_BUYER', 'APPROVED'),
  ('$http_other_org', 'Stream fixture $http_other_org', 'FUEL_BUYER', 'APPROVED'),
  ('$http_pending_org', 'Stream fixture $http_pending_org', 'FUEL_BUYER', 'PENDING');
INSERT INTO users (id, email, password_hash, role, status, organization_id, email_verified, must_change_password)
VALUES
  ('$http_member', 'stream-$http_member@example.test', 'unused', 'BUYER', 'APPROVED', '$http_org', true, false),
  ('$http_pending_member', 'stream-$http_pending_member@example.test', 'unused', 'BUYER', 'APPROVED', '$http_pending_org', true, false);
SQL
run_cargo_test streams_http 2 \
  STREAM_HTTP_APP_URL="$app_url" STREAM_HTTP_OWNER_URL="$owner_url" \
  STREAM_HTTP_USER="$http_member" STREAM_HTTP_ORG="$http_org" STREAM_HTTP_OTHER_ORG="$http_other_org" \
  STREAM_HTTP_PENDING_USER="$http_pending_member" STREAM_HTTP_PENDING_ORG="$http_pending_org" \
  -- --features contract-tests --test group_streams streams_http:: -- --ignored --test-threads=1 --nocapture
for marker in \
  'header cursor replay is scoped to the organization' \
  'live delivery follows replay without foreign events' \
  'capacity is one in-band control; a dropped body frees its slot' \
  'alias path, header cursor precedence and heartbeat' \
  'query cursor replay' \
  'account and organization refusals precede headers' \
  'an unanswered account store is 503 before headers'; do
  require_output "stream http postgres: $marker"
done

# --- Stream runtime restarts ----------------------------------------------------

clear_retention
run_cargo_test streams_coexistence 1 \
  STREAM_RUNTIME_APP_URL="$app_url" STREAM_RUNTIME_ADMIN_URL="$admin_url" \
  -- --features contract-tests --test group_streams streams_coexistence:: -- --ignored --nocapture
for cycle in 1 2 3; do
  require_output "cycle=$cycle "
done

# --- Stream replay --------------------------------------------------------------

replay_org=00000000-0000-0000-0000-000000000007
replay_user=00000000-0000-4000-8000-000000000501
clear_retention
truncate_account_tables "$analytics_database"
owner_sql "$analytics_database" <<SQL
INSERT INTO organizations (id, name, type, verification_status)
  VALUES ('$replay_org', 'Replay fixture', 'FUEL_BUYER', 'APPROVED');
INSERT INTO users (id, email, password_hash, role, status, organization_id, email_verified, must_change_password)
  VALUES ('$replay_user', 'replay-$replay_user@example.test', 'unused', 'BUYER', 'APPROVED', '$replay_org', true, false);
SQL
run_cargo_test streams_replay 1 \
  STREAM_REPLAY_APP_URL="$app_url" STREAM_REPLAY_OWNER_URL="$owner_url" STREAM_REPLAY_USER="$replay_user" \
  -- --features contract-tests --test group_streams -- --ignored --exact streams_replay::postgres_replay_snapshots --nocapture
for marker in \
  '601 authorized rows with foreign holes and released connections' \
  'prune after captured page and before next snapshot' \
  'empty retained table keeps floor and equal cursor' \
  'current account cutoff terminates live delivery'; do
  require_output "$marker"
done

# --- Stream replay store --------------------------------------------------------

# The store cases read as the app role what the owner seeded for each case.
store_org=00000000-0000-4000-8000-0000000000a1
store_foreign_org=00000000-0000-4000-8000-0000000000b2
run_store_case() {
  run_cargo_test "streams_store $1" 1 STREAMS_STORE_TEST_DATABASE_URL="$app_url" \
    -- --features contract-tests --test group_streams "streams_store::$1" -- --exact --nocapture
}
reset_retention_ownership
run_store_case postgres_empty_store_boundary

reset_retention_ownership
insert_outbox_event 10 "$store_org" '{"schema_version": 1, "_public_market_invalidation": true}'
insert_outbox_event 20 "$store_foreign_org"
insert_outbox_event 30 "$store_org"
run_store_case postgres_sequence_holes_and_scope

reset_retention_ownership
owner_sql "$analytics_database" <<'SQL'
DELETE FROM public.market_event_replay_state;
SQL
run_store_case postgres_missing_state_is_unavailable

reset_retention_ownership
for sequence in 5 15 25; do
  insert_outbox_event "$sequence" "$store_org"
done
run_store_case postgres_prefix_retained
owner_sql "$analytics_database" <<'SQL'
DELETE FROM public.market_event_outbox WHERE stream_seq IN (5, 15);
SQL
run_store_case postgres_prefix_discarded
owner_sql "$analytics_database" <<'SQL'
DELETE FROM public.market_event_outbox;
SQL
run_store_case postgres_empty_retained_history_boundary

# A forged event name and an oversized participant list are both invalid rows.
reset_retention_ownership
insert_outbox_event 10 "$store_org"
owner_sql "$analytics_database" <<'SQL'
UPDATE public.market_event_outbox SET event_type = E'trade_created\nforged' WHERE stream_seq = 10;
SQL
run_store_case postgres_invalid_row_is_classified
reset_retention_ownership
insert_outbox_event 10 "$store_org"
owner_sql "$analytics_database" <<SQL
UPDATE public.market_event_outbox
SET participant_org_ids = json_build_array('$store_org', repeat('x', 300000))
WHERE stream_seq = 10;
SQL
run_store_case postgres_invalid_row_is_classified

reset_retention_ownership
run_store_case postgres_pool_acquisition_is_bounded

# --- Stream runtime lifecycle ---------------------------------------------------

run_cargo_test streams_runtime 2 \
  STREAM_RUNTIME_APP_URL="$app_url" STREAM_RUNTIME_OWNER_URL="$owner_url" STREAM_RUNTIME_ADMIN_URL="$admin_url" \
  -- --features contract-tests --test group_streams streams_runtime:: -- --ignored --test-threads=1 --nocapture

# --- Watchlist guard ------------------------------------------------------------

# One REAL organization with an open public BID and a cancelled BID of one
# active product and delivery point, so only status decides what is public.
guard_org=00000000-0000-4000-8000-000000000601
guard_owner=00000000-0000-4000-8000-000000000602
guard_public_order=00000000-0000-4000-8000-000000000603
guard_closed_order=00000000-0000-4000-8000-000000000604
guard_product=00000000-0000-4000-8000-000000000605
guard_point=00000000-0000-4000-8000-000000000606
truncate_account_tables "$analytics_database"
owner_sql "$analytics_database" <<SQL
INSERT INTO products (id, name, fuel_type, fuel_grade, is_active)
  VALUES ('$guard_product', 'Guard methanol fixture', 'Methanol', 'Guard fixture', true);
INSERT INTO delivery_points (id, name, region, timezone, is_active)
  VALUES ('$guard_point', 'Guard point fixture', 'Guard region', 'UTC', true);
INSERT INTO organizations (id, name, type) VALUES ('$guard_org', 'Guard organization fixture', 'FUEL_BUYER');
-- Company approval is the trust decision that classifies it as REAL.
UPDATE organizations SET verification_status = 'APPROVED' WHERE id = '$guard_org';
DO \$\$ BEGIN
  IF (SELECT provenance FROM organizations WHERE id = '$guard_org') <> 'REAL' THEN
    RAISE EXCEPTION 'the approved guard organization is not REAL';
  END IF;
END \$\$;
INSERT INTO users (id, email, password_hash, role, status, organization_id, email_verified, must_change_password)
  VALUES ('$guard_owner', 'guard-fixture@example.test', 'unused', 'BUYER', 'APPROVED', '$guard_org', true, false);
INSERT INTO orderbook_orders (id, organization_id, product_id, delivery_point_id, provenance, side,
  quantity_mt, remaining_quantity_mt, price_per_mt_usd, availability_window, status)
VALUES
  ('$guard_public_order', '$guard_org', '$guard_product', '$guard_point', 'REAL', 'BID', 10, 10, 500, 'SPOT', 'OPEN'),
  ('$guard_closed_order', '$guard_org', '$guard_product', '$guard_point', 'REAL', 'BID', 10, 10, 500, 'SPOT', 'CANCELLED');
SQL
guard_market=$(printf '{"organization":"%s","owner":"%s","public_order":"%s","closed_order":"%s","product":"%s","point":"%s","product_name":"Guard methanol fixture","fuel_type":"Methanol","fuel_grade":"Guard fixture","point_name":"Guard point fixture","region":"Guard region"}' \
  "$guard_org" "$guard_owner" "$guard_public_order" "$guard_closed_order" "$guard_product" "$guard_point")
run_cargo_test streams_watchlist_guard 7 \
  WATCHLIST_GUARD_APP_URL="$app_url" WATCHLIST_GUARD_FIXTURE_URL="$owner_url" WATCHLIST_GUARD_MARKET="$guard_market" \
  -- --features contract-tests --test group_streams streams_watchlist_guard:: -- --include-ignored --test-threads=1

# --- Public read cache ----------------------------------------------------------

run_cargo_test cache_events_postgres 4 PUBLIC_CACHE_TEST_DATABASE_URL="$owner_url" \
  -- --test group_market_reads cache_events_postgres::
# Two migrated incarnations on one server; the prior-incarnation cases need both.
run_cargo_test trading_cache_postgres 18 \
  PUBLIC_CACHE_TEST_DATABASE_URL="$owner_url" PUBLIC_CACHE_TEST_SECOND_DATABASE_URL="$market_owner_url" \
  -- --test group_market_reads trading_cache_postgres:: -- --test-threads=1

# The route cases read the first active product and delivery point and need
# one open order: a REAL buyer's BID for Bio Methanol at Singapore.
route_buyer_org=00000000-0000-4000-8000-000000000701
route_buyer=00000000-0000-4000-8000-000000000702
route_order=00000000-0000-4000-8000-000000000703
route_product=9510c713-6e39-5080-add3-0a7c29657b79
route_point=73835e92-820e-584b-8280-bb61c63aa28e
owner_sql "$market_database" <<SQL
TRUNCATE TABLE trades, orderbook_orders, organizations, products, delivery_points RESTART IDENTITY CASCADE;
INSERT INTO products (id, name, fuel_type, fuel_grade, unit, min_lot_size, is_active)
  VALUES ('$route_product', 'Bio Methanol', 'Methanol', 'Bio', 'MT', 200.00, true);
INSERT INTO delivery_points (id, name, region, timezone, is_active)
  VALUES ('$route_point', 'Singapore', 'Asia', 'Asia/Singapore', true);
INSERT INTO organizations (id, name, type) VALUES ('$route_buyer_org', 'Route buyer fixture', 'FUEL_BUYER');
UPDATE organizations SET verification_status = 'APPROVED' WHERE id = '$route_buyer_org';
INSERT INTO users (id, email, password_hash, role, status, organization_id, email_verified, kyc_status)
  VALUES ('$route_buyer', 'buyer-fixture@route.test', 'unused', 'BUYER', 'APPROVED', '$route_buyer_org', true, 'APPROVED');
INSERT INTO orderbook_orders (id, organization_id, product_id, delivery_point_id, provenance, side,
  quantity_mt, remaining_quantity_mt, price_per_mt_usd, availability_window, status)
VALUES ('$route_order', '$route_buyer_org', '$route_product', '$route_point', 'REAL', 'BID', 300, 300, 700, 'SPOT', 'OPEN');
SQL
run_cargo_test 'trading_cache_postgres public routes' 2 ROUTE_CACHE_TEST_DATABASE_URL="$market_owner_url" \
  -- --test group_market_reads trading_cache_postgres::public_ -- --test-threads=1

run_cargo_test cache_failover_postgres 6 \
  PUBLIC_CACHE_TEST_DATABASE_URL="$owner_url" PUBLIC_CACHE_TEST_SECOND_DATABASE_URL="$market_owner_url" \
  -- --test group_market_reads cache_failover_postgres:: -- --test-threads=1

printf '\nAll auth, stream, cache and alert PostgreSQL targets passed:\n'
printf '  %s\n' "${summary[@]}"
