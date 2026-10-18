"""Route adapter for Runner's disposable database/process lifecycle."""
from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

from scripts.rust_contract.phase2_mail import MailStub
from scripts.rust_contract.phase2_scenarios import (
    HTTPCell,
    ScenarioError,
    run_actions,
    run_mixed,
    run_race,
)
from scripts.rust_contract.phase2_seed import (
    APP_ROLE,
    BACKUP_ROLE,
    MIGRATOR_ROLE,
    PASSWORD,
    connect,
    digest,
    identifier,
    sequence_marks,
    snapshot,
)
from scripts.rust_contract.phase2_state import compare_state, validate_cells


@dataclass
class Context:
    root: Path
    python_bin: str
    environment: dict
    database_url: str = ""
    mail: MailStub | None = None


def run_registered(runner, args, selected):
    from scripts.rust_contract.run_disposable import (
        DB_NAME,
        ROOT,
        attest,
        docker,
        ephemeral_port,
        wait_for_port,
    )

    if args.phase2_races and any(scenario.races is None for scenario in selected):
        raise ScenarioError("selected scenario has no registered races")
    port = runner.start_database()
    environment = {**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": runner.sha,
                   "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars", **runner.urls(port),
                   "SQLX_OFFLINE": "true"}
    runner.seed = False
    runner.migrate_and_seed(environment)
    with connect(port, DB_NAME) as source:
        source.execute("INSERT INTO organizations (id,name,domain,type) VALUES ('10000000-0000-4000-8000-000000000001','Phase 2 fixture','phase2.invalid','FUEL_BUYER')")
        source.execute("INSERT INTO users (id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES ('20000000-0000-4000-8000-000000000001','actor@phase2.invalid','not-a-login','BUYER','APPROVED','10000000-0000-4000-8000-000000000001',true,false)")
    policy_dir = "/tmp/phase2-route-policy"
    docker("exec", runner.container, "mkdir", "-p", policy_dir)
    policy_files = ("app_acl_policy.sql", "converge_runtime_object_acls.sql", "bootstrap_roles.sql", "validate_roles.sql")
    for name in policy_files:
        docker("cp", str(ROOT / "deploy/postgres" / name), f"{runner.container}:{policy_dir}/{name}")
    acl = digest({name: (ROOT / "deploy/postgres" / name).read_text() for name in policy_files})

    def clone(source, target):
        with connect(port, "postgres", autocommit=True) as admin:
            admin.execute(f"CREATE DATABASE {identifier(target)} TEMPLATE {identifier(source)}")

    def prepare(database):
        for script in ("bootstrap_roles.sql", "validate_roles.sql"):
            docker("exec", runner.container, "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", database,
                   "-v", f"database_name={database}", "-v", f"app_role={APP_ROLE}",
                   "-v", f"migrator_role={MIGRATOR_ROLE}", "-v", f"backup_role={BACKUP_ROLE}",
                   "-f", f"{policy_dir}/{script}")
        with connect(port, database) as admin:
            admin.execute(f"ALTER ROLE {identifier(APP_ROLE)} PASSWORD '{PASSWORD}'")

    def read(database):
        with connect(port, database) as reader:
            return snapshot(reader)

    reports = []
    with MailStub() as mail:
        for index, scenario in enumerate(selected):
            context = Context(ROOT, runner.python_bin, environment, mail=mail)
            template = f"phase2_route_seed_{index}"
            clone(DB_NAME, template)
            with connect(port, template) as source:
                scenario.setup(source)
            databases = [f"phase2_route_{index}_{label}" for label in ("python", "rust", "mixed")]
            for database in databases:
                clone(template, database)
                prepare(database)
            initial = [read(database) for database in databases]
            if any(compare_state(initial[0], state) for state in initial[1:]):
                raise ScenarioError("cloned route seed differs")
            if scenario.build:
                subprocess.run(scenario.build, cwd=ROOT, env=environment, check=True, timeout=600)
            specs = scenario.servers(context)
            if len(specs) != 2:
                raise ScenarioError("scenario requires Python and Rust server adapters")

            def start_cells(targets, suffix, mixed=False):
                cells = []
                for runtime, database, spec in zip(("python", "rust"), targets, specs):
                    bind_port = ephemeral_port()
                    label = f"{index}-{suffix}-{runtime}"
                    database_url = f"postgresql://{APP_ROLE}:{PASSWORD}@127.0.0.1:{port}/{database}"
                    env = {**environment, **spec.environment, "ENVIRONMENT": "test", "RELEASE_SHA": runner.sha,
                           "DATABASE_URL": database_url.replace("postgresql://", "postgresql+asyncpg://", 1),
                           "PHASE2_DATABASE_URL": database_url, "PHASE2_BIND": f"127.0.0.1:{bind_port}",
                           "DISPOSABLE_TEST_TOKEN": runner.token, "PHASE2_EMIT_EVENTS": "true" if mixed else "false",
                           "PHASE2_MAIL_URL": f"{mail.url}/{label}/emails", "RESEND_API_KEY": "disposable-only",
                           "HTTP_PROXY": "", "HTTPS_PROXY": "", "ALL_PROXY": "", "NO_PROXY": "127.0.0.1"}
                    runner.spawn([part.replace("{port}", str(bind_port)) for part in spec.command], env)
                    wait_for_port(bind_port)
                    endpoint = f"http://127.0.0.1:{bind_port}"
                    attest(endpoint, runner.token)
                    cells.append(HTTPCell(endpoint, runner.token, lambda db=database: read(db),
                                          lambda key=label: mail.ledger(key)))
                return cells

            cells = start_cells(databases[:2], "twin")
            validate_cells({"database": databases[0], "seed": digest(initial[0]), "acl": acl},
                           {"database": databases[1], "seed": digest(initial[1]), "acl": acl}, runner.token, cells[0].endpoint)
            actions = []
            for action in scenario.actions(context):
                mail.respond(status=action.mail_status, body=action.mail_body)
                actions.extend(run_actions([action], cells))
            races = []
            if args.phase2_races:
                schedules = scenario.races(context)
                if not schedules:
                    raise ScenarioError("race mode has no schedules")
                for race in schedules:
                    races.extend(run_race(race, cells))
            mixed = None
            if scenario.mixed:
                if not scenario.dispatch or not scenario.assert_mixed:
                    raise ScenarioError("mixed writer requires dispatcher and state assertions")
                mixed_cells = start_cells([databases[2], databases[2]], "mixed", mixed=True)
                context.database_url = f"postgresql://{APP_ROLE}:{PASSWORD}@127.0.0.1:{port}/{databases[2]}"
                mixed = run_mixed(scenario.mixed(context), mixed_cells, lambda: scenario.dispatch(context))
                mixed_state = read(databases[2])
                scenario.assert_mixed(mixed_state, mixed)
                mixed["state_sha256"] = digest(mixed_state)
            marks = {}
            for database in databases:
                with connect(port, database) as reader:
                    marks[database] = sequence_marks(reader)
            reports.append({"scenario": scenario.name, "group": scenario.group,
                            "scope": "route-contract" if scenario.route_qualification else "harness-only",
                            "route_qualification": scenario.route_qualification, "seed_sha256": digest(initial[0]),
                            "acl_sha256": acl, "actions": actions, "races": races,
                            "mixed_writer_dispatch": mixed, "sequence_high_water": marks})
            # Release pools before the next scenario. Final cleanup still owns failures.
            runner.stop_children()
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps({"source_sha": runner.sha, "scenarios": reports,
                                          "measurements": False}, indent=2) + "\n")
