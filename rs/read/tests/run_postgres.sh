#!/usr/bin/env bash
# Run the whole Rust workspace against a disposable PostgreSQL container, then
# the migrated market-write component targets on a second migrated database.
#
# Docker runs locally through Colima. The container binds a Docker-assigned
# loopback port and is removed on exit. Never point this at staging or prod.
#
# Optional environment:
#   RUN_POSTGRES_GROUPS  space-separated test groups to run (default: all).
#                        CI runs each group in its own shard; every group
#                        builds the same databases first.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'

export SQLX_OFFLINE=true

# workspace:         the default workspace cases on the minimal fixture;
# market_settlement: market-write targets for trades, settlement and races;
# market_orders:     market-write targets for order creation and changes;
# schema:            Phase 4 authority suites, baseline, adoption and ACLs.
all_groups='workspace market_settlement market_orders schema'
groups=${RUN_POSTGRES_GROUPS:-$all_groups}
for group in $groups; do
  case " $all_groups " in
    *" $group "*) ;;
    *) echo "unknown test group: $group (known: $all_groups)" >&2; exit 2 ;;
  esac
done
selected() {
  case " $groups " in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

container=''
log=''
cleanup() {
  # An EXIT trap replaces the script's exit status with the status of its own
  # last command. Capture the real status first and restore it after the
  # container and scratch log are gone, so no failure is reported as success.
  local status=$?
  if [[ -n "$container" ]]; then
    docker logs "$container" || true
    # -v removes the anonymous data volume; a forced remove skips the --rm cleanup.
    docker rm -f -v "$container" >/dev/null || true
  fi
  if [[ -n "$log" ]]; then rm -f "$log"; fi
  exit "$status"
}
trap cleanup EXIT

container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
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
CREATE ROLE rust_backup NOLOGIN;
-- Phase 10 Task 3 target-attestation accept cases need the deployed role names.
CREATE ROLE verdaxis_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE ROLE verdaxis_app_staging LOGIN NOINHERIT PASSWORD 'disposable_staging_pw';
-- Phase 10 Tasks 5 and 6: the production migrator name owns the baseline and
-- adoption databases, so the catalog fingerprint's owner projection matches.
CREATE ROLE verdaxis_migrator LOGIN NOINHERIT PASSWORD 'disposable_verdaxis_migrator_pw';
-- The M adoption binds schema ownership to the adopting environment's
-- migrator name; the market fixtures below are staging-shaped.
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT PASSWORD 'disposable_migrator_pw';
CREATE DATABASE verdaxis_rust_test OWNER rust_migrator;
-- A second database proves the attestation refuses a correct role on the
-- wrong database name.
CREATE DATABASE verdaxis_other_test OWNER rust_migrator;
\connect verdaxis_rust_test
CREATE EXTENSION postgis;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO rust_app, rust_member;
SET ROLE rust_migrator;
CREATE TABLE alembic_version(version_num varchar(64));
INSERT INTO alembic_version VALUES ('kycg_20261014_staging');
SQL
export RUST_TEST_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_rust_test"
# Demand cases require an explicit disposable _test URL and create only connection-local temporary tables.
export RUST_DEMAND_TEST_DATABASE_URL="$RUST_TEST_DATABASE_URL"
export RUST_TEST_MEMBER_DATABASE_URL="postgres://rust_member:disposable_member_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_MIGRATOR_DATABASE_URL="postgres://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_PRODUCTION_DATABASE_URL="postgres://verdaxis_app:disposable_app_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_STAGING_DATABASE_URL="postgres://verdaxis_app_staging:disposable_staging_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_WRONG_DATABASE_URL="postgres://verdaxis_app:disposable_app_pw@127.0.0.1:$port/verdaxis_other_test"
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
export ENVIRONMENT="${ENVIRONMENT:-test}"
export JWT_SECRET="${JWT_SECRET:-test-secret-key-for-testing-minimum-32-chars}"

# The market-write component suites need the fully migrated schema and the
# converged runtime grants, not the minimal workspace fixture above. Two
# databases share one container; each target receives its own runtime and
# admin URLs on the same numeric-loopback endpoint.
market_image_role_setup() {
  local database=$1
  docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<SQL
CREATE DATABASE $database OWNER verdaxis_migrator_staging;
\connect $database
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
GRANT USAGE ON SCHEMA public TO rust_app;
SQL
}
for database in verdaxis_trades_test verdaxis_create_test verdaxis_amend_test verdaxis_confirm_test verdaxis_publish_test verdaxis_settlement_test verdaxis_freeze_test verdaxis_freeze_enforcement_test verdaxis_races_test verdaxis_producer_test verdaxis_residual_test verdaxis_serving_test verdaxis_inventory_test verdaxis_support_test; do
  market_image_role_setup "$database"
done
export RUST_TEMPLATE_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/postgres"
for target in TRADES CREATE AMEND CONFIRM PUBLISH SETTLEMENT FREEZE FREEZE_ENFORCEMENT RACES PRODUCER RESIDUAL SERVING INVENTORY SUPPORT; do
  database="verdaxis_$(printf '%s' "$target" | tr '[:upper:]' '[:lower:]')_test"
  export "RUST_${target}_TEMPLATE_DATABASE=$database"
done
export RUST_TRADES_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_trades_test"
export RUST_TRADES_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_trades_test"
export RUST_CREATE_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_create_test"
export RUST_CREATE_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_create_test"
export RUST_AMEND_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_amend_test"
export RUST_AMEND_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_amend_test"
export RUST_CONFIRM_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_confirm_test"
export RUST_CONFIRM_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_confirm_test"
export RUST_FREEZE_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_freeze_test"
export RUST_FREEZE_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_freeze_test"
export RUST_FREEZE_ENFORCEMENT_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_freeze_enforcement_test"
export RUST_FREEZE_ENFORCEMENT_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_freeze_enforcement_test"
export RUST_SETTLEMENT_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_settlement_test"
export RUST_SETTLEMENT_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_settlement_test"
export RUST_PUBLISH_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_publish_test"
export RUST_PUBLISH_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_publish_test"
export RUST_RACES_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_races_test"
export RUST_RACES_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_races_test"
export RUST_PRODUCER_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_producer_test"
export RUST_PRODUCER_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_producer_test"
# The residual-writer target clones this template per case; its fixtures run
# there as the database owner role.
export RUST_RESIDUAL_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_residual_test"
export RUST_RESIDUAL_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_residual_test"
export RUST_RESIDUAL_OWNER_DATABASE_URL="postgres://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/verdaxis_residual_test"
# The market serving target clones this template per case.
export RUST_SERVING_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_serving_test"
export RUST_SERVING_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_serving_test"
export RUST_INVENTORY_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_inventory_test"
export RUST_INVENTORY_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_inventory_test"
export RUST_SUPPORT_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_support_test"
export RUST_SUPPORT_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_support_test"
for database in verdaxis_trades_test verdaxis_create_test verdaxis_amend_test verdaxis_confirm_test verdaxis_publish_test verdaxis_settlement_test verdaxis_freeze_test verdaxis_freeze_enforcement_test verdaxis_races_test verdaxis_producer_test verdaxis_residual_test verdaxis_serving_test verdaxis_inventory_test verdaxis_support_test; do
  export PGHOST=127.0.0.1 PGPORT="$port" PGUSER=verdaxis_migrator_staging \
    PGPASSWORD=disposable_migrator_pw PGDATABASE="$database"
  # Initialize the baseline and sqlx history before runtime ACL convergence.
  RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/$database" \
    cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test group_phase10 \
    phase10_migrate_postgres::runner_database_adopts_to_the_baseline -- --ignored
  psql -X --single-transaction -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role=rust_app \
    -v migrator_role=verdaxis_migrator_staging -v backup_role=rust_backup \
    -f deploy/postgres/converge_runtime_object_acls.sql
done
export RUST_TEMPLATE_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/postgres"
export RUST_TRADES_TEMPLATE_DATABASE=verdaxis_trades_test
export RUST_CONFIRM_TEMPLATE_DATABASE=verdaxis_confirm_test
export RUST_SETTLEMENT_TEMPLATE_DATABASE=verdaxis_settlement_test
export RUST_FREEZE_TEMPLATE_DATABASE=verdaxis_freeze_test
export RUST_FREEZE_ENFORCEMENT_TEMPLATE_DATABASE=verdaxis_freeze_enforcement_test
unset DATABASE_URL MIGRATOR_DATABASE_URL PGHOST PGPORT PGUSER PGPASSWORD PGDATABASE

if selected workspace; then
  log=$(mktemp "${TMPDIR:-/tmp}/rust-postgres-tests.XXXXXX")
  set +e
  # The Task 6 visibility, trade-evidence and market-policy matrices, the
  # Task 9/10 old-shape/new-shape equivalence proofs, and the five Phase 5
  # compliance overlay populated-fixture cases need a seeded fixture catalog
  # that this database does not carry. These cases are named here rather than
  # silently skipped: rs/read/tests/run_market_contract.sh
  # runs all five against their own disposable database, refusing a green
  # result when the fixture is missing. `--skip` matches test function names,
  # not test file names, so every test is listed by its name.
  cargo test --manifest-path rs/Cargo.toml --locked --workspace --features contract-tests \
    -- --test-threads=1 \
    --skip runtime_documents_and_history_are_append_only \
    --skip runtime_cannot_write_provenance_or_grace \
    --skip organization_approval_trigger_reads_back_real_and_vd001 \
    --skip retained_documents_block_user_and_org_delete \
    --skip rust_service_credential_absence_does_not_fail_readiness \
    --skip benchmark_key_first_matches_key_join_last \
    --skip biofuel_supplier_metadata_clause_hides_only_failing_asks \
    --skip eligibility_sets_match_legacy_for_the_curve_relation \
    --skip eligibility_sets_match_legacy_predicate_for_every_order \
    --skip compliance_overlay_postgres_cases \
    --skip compliance_overlay_authenticated_cases \
    --skip compliance_overlay_timeout_cases \
    --skip compliance_overlay_snapshot_cases \
    --skip compliance_overlay_index_cases \
    --skip curve_table_keyed_candidates_match_the_full_relation \
    --skip curve_table_equivalence_detects_a_dropped_key \
    --skip curve_slice_keyed_candidates_match_the_full_relation \
    --skip curve_board_keyed_candidates_keep_null_window_keys \
    --skip mcp_market_slice_keyed_candidates_match_the_full_relation \
    --skip keyed_join_would_duplicate_rows \
    --skip corpus_clock_maps_to_the_pinned_singapore_date \
    --skip orderbook_aggregate_narrow_projection_keeps_every_group \
    --skip orderbook_product_counts_narrow_projection_keeps_every_count \
    --skip orderbook_distinct_labels_keep_order_and_nulls \
    --skip orderbook_map_recent_asks_keep_one_row_per_point \
    --skip orderbook_listings_pages_match_the_full_ranking \
    --skip corpus_carries_the_rows_these_cases_compare \
    --skip opposing_exact_slices_keep_prices_and_ordered_hash \
    --skip opposing_null_point_prefilter_keeps_the_tail_match \
    --skip aggregate_groups_null_region_and_counts_duplicates \
    --skip inspect_users_masks_email_by_default \
    --skip inspect_users_show_emails_requires_the_explicit_flag \
    --skip inspect_users_matches_fixed_rows \
    --skip inspect_blockers_matches_fixed_counts \
    --skip inspect_users_is_read_only_and_never_writes_a_status_file \
    --skip inspect_blockers_exits_2_when_any_count_is_nonzero \
    --skip inspect_json_is_one_object_with_stable_keys_on_a_real_database \
    --skip the_reader_role_is_enough_for_both_commands 2>&1 | tee "$log"
  cargo_status=${PIPESTATUS[0]}
  set -e
  if (( cargo_status != 0 )); then
    exit "$cargo_status"
  fi
  # A green cargo run that executed no test is a false pass; refuse it.
  if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
    echo "no Rust test ran: refusing a green result" >&2
    exit 1
  fi
fi

# Run the named market-write test modules with their ignored database cases.
# Each module executes on its own migrated disposable database. A green run
# that executed no test is a false pass, so refuse it.
run_market_modules() {
  local group=$1
  shift
  local market_log market_status
  market_log=$(mktemp "${TMPDIR:-/tmp}/rust-postgres-$group.XXXXXX")
  set +e
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests \
    --test group_market_write -- --include-ignored --test-threads=1 \
    "$@" 2>&1 | tee "$market_log"
  market_status=${PIPESTATUS[0]}
  set -e
  if (( market_status != 0 )); then
    exit "$market_status"
  fi
  if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$market_log"; then
    echo "no $group test ran: refusing a green result" >&2
    exit 1
  fi
  rm -f "$market_log"
}

if selected market_settlement; then
  run_market_modules market_settlement \
    market_write_pg_trades:: market_write_pg_settlement:: \
    market_write_pg_trade_membership:: market_write_pg_races:: \
    market_write_pg_residual:: market_write_pg_support:: \
    market_write_serving::
fi
if selected market_orders; then
  run_market_modules market_orders \
    market_write_pg_create:: market_write_pg_amend:: \
    market_write_pg_confirm:: market_write_pg_inventory:: \
    market_write_pg_publish:: \
    market_write_pg_freeze:: market_write_pg_freeze_enforcement:: \
    market_write_pg_producer:: market_write_pg_producer_visibility::
fi

if selected schema; then
  # Phase 4 authority/review suites mark database cases explicitly; execute them
  # here as well as the default workspace cases, on the same disposable endpoint.
  # Module filters apply to every selected binary, so the targets in their own
  # binary run without filters and the grouped modules run separately.
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests \
    --test phase4_analytics_views --test phase4_analytics_legacy --test phase4_organizations \
    -- --include-ignored --test-threads=1
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests \
    --test group_phase4_phase5 -- --include-ignored --test-threads=1 \
    phase4_admission:: phase4_races::
  # Phase 10 Task 6: restore the baseline without sqlx history for adoption cases.
  # The production migrator owns every object in the template.
  adoption_head=$(python3 -c 'import json; print(json.load(open("deploy/rust/sqlx-baseline.json"))["alembic_head"])')
  docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE DATABASE verdaxis_adoption_template OWNER verdaxis_migrator;
\connect verdaxis_adoption_template
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator;
SQL
  # pg_dump renders varchar IN-lists as text-cast arrays. The shared helper
  # reparses only those CHECK/index expressions as IN-lists to reproduce the
  # source catalog form required by the compiled adoption fingerprint. Native
  # templates keep the frozen restore form; no migration or fingerprint policy
  # is changed here.
  python3 deploy/tests/migrated-catalog-sql.py rs/read/migrations/1_baseline.sql \
    | docker exec -i "$container" psql -U verdaxis_migrator -d verdaxis_adoption_template \
    -X -v ON_ERROR_STOP=1 -q > /dev/null
  # Adoption validates this source revision and writes sqlx history itself.
  docker exec -i "$container" psql -U verdaxis_migrator -d verdaxis_adoption_template \
    -X -v ON_ERROR_STOP=1 -v adoption_head="$adoption_head" <<'SQL'
CREATE TABLE public.alembic_version(version_num varchar(32) NOT NULL PRIMARY KEY);
INSERT INTO public.alembic_version VALUES (:'adoption_head');
SQL
  export RUST_ADOPTION_TEMPLATE_DATABASE=verdaxis_adoption_template
  export RUST_ADOPTION_ALEMBIC_HEAD="$adoption_head"

  # Phase 10 Task 21: the signal ingestion template, built as on a deployed host:
  # the baseline applied by the staging migrator, then the real ACL convergence
  # with the operator role. Each ingestion case clones it as verdaxis_staging.
  # The staging migrator role is created once, above, for every template.
  docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_backup_staging LOGIN NOINHERIT PASSWORD 'disposable_staging_backup_pw';
CREATE ROLE verdaxis_operator_staging LOGIN NOINHERIT PASSWORD 'disposable_operator_pw';
CREATE DATABASE verdaxis_ingest_template OWNER verdaxis_migrator_staging;
\connect verdaxis_ingest_template
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
SQL
  docker exec -i "$container" psql -U verdaxis_migrator_staging -d verdaxis_ingest_template \
    -v ON_ERROR_STOP=1 -q < rs/read/migrations/1_baseline.sql > /dev/null
  docker exec "$container" mkdir -p /tmp/acl-bundle
  docker cp deploy/postgres/. "$container":/tmp/acl-bundle/
  docker exec "$container" psql -U postgres -d verdaxis_ingest_template -X -q --single-transaction \
    -v ON_ERROR_STOP=1 -v database_name=verdaxis_ingest_template -v app_role=verdaxis_app_staging \
    -v migrator_role=verdaxis_migrator_staging -v backup_role=verdaxis_backup_staging \
    -v operator_role=verdaxis_operator_staging -f /tmp/acl-bundle/converge_runtime_object_acls.sql > /dev/null
  export RUST_INGEST_TEMPLATE_DATABASE=verdaxis_ingest_template
  # The ingestion cases rerun the convergence through this container's psql.
  export RUST_POSTGRES_CONTAINER="$container"

  # Phase 10 Tasks 5 and 6: the frozen sqlx baseline restores into fresh
  # databases, and adoption runs on clones of the template above; each case
  # creates and drops its own database. Task 9: `ops preflight` attests a
  # verdaxis_staging database that its case creates and drops.
  # The runner-only adoption helper already ran above with each explicit migrator
  # URL. This group has no runner URL; keep the helper out of its broad selection.
  baseline_log=$(mktemp "${TMPDIR:-/tmp}/rust-postgres-baseline.XXXXXX")
  set +e
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests \
    --test group_phase10 -- --include-ignored --skip runner_database_adopts_to_the_baseline \
    --test-threads=1 \
    phase10_baseline_postgres:: phase10_migrate_postgres:: \
    phase10_ops_preflight_postgres:: phase10_ingest_postgres:: \
    phase10_gena_postgres:: phase10_canary_postgres:: phase10_cache_postgres:: \
    2>&1 | tee "$baseline_log"
  baseline_status=${PIPESTATUS[0]}
  set -e
  if (( baseline_status != 0 )); then
    exit "$baseline_status"
  fi
  if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$baseline_log"; then
    echo "no baseline or migrate test ran: refusing a green result" >&2
    exit 1
  fi
  # Phase 10 Task 6: the sqlx history ACL on its own disposable server.
  bash deploy/tests/sqlx-control-acl.sh
fi
