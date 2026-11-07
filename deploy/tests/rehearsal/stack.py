#!/usr/bin/env python3
"""Real disposable stack driver. No command substitutes or application mocks."""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path

from migrated_schema import REVISION, schema_sql

ROOT = Path(os.environ["REHEARSAL_SOURCE_DIR"])
PRIVATE = Path(os.environ["PRIVATE"])
PUBLIC = Path(os.environ["PUBLIC"])
ENV = os.environ["REHEARSAL_ENVIRONMENT"]
SHA = os.environ["CURRENT_SHA"]
OLD = "0920be378ebbc42e88afba455d9521d60d49845a"
CHECKOUT = (
    Path("/home/verdaxis-prod/verdaxis")
    / ("prod" if ENV == "production" else "staging")
    / "be"
)
SUFFIX = "" if ENV == "production" else "_staging"
UNIT_SUFFIX = "" if ENV == "production" else "-staging"
RUST_UNIT = f"verdaxis-rust{UNIT_SUFFIX}.service"
APP = "verdaxis_app" + SUFFIX
MIGRATOR = "verdaxis_migrator" + SUFFIX
BACKUP = "verdaxis_backup" + SUFFIX
DB = "verdaxis_prod" if ENV == "production" else "verdaxis_staging"
DOMAIN = (
    "api.verdaxis.exchange" if ENV == "production" else "api-staging.verdaxis.exchange"
)
PORT = 8010 if ENV == "production" else 8011
GROUPS = (
    "news",
    "auth-prune",
    "reminders",
    "onboarding",
    "demo",
    "local-monitor",
    "external-recovery",
)
PROPERTIES = (
    "FragmentPath",
    "DropInPaths",
    "ExecStart",
    "ExecStartPre",
    "EnvironmentFiles",
    "Environment",
    "PassEnvironment",
    "SetCredential",
    "SetCredentialEncrypted",
    "LoadCredential",
    "LoadCredentialEncrypted",
    "User",
    "Group",
    "WorkingDirectory",
)


def run(*args, env=None, data=None, check=True, cwd=None):
    result = subprocess.run(
        [str(a) for a in args],
        input=data,
        capture_output=True,
        env=env,
        cwd=cwd,
        check=False,
        timeout=1800,
    )
    if check and result.returncode:
        # Protected logs can contain credentials; never send this to public output.
        sys.stderr.buffer.write(result.stderr)
        raise RuntimeError(f"command failed ({result.returncode}): {args[0]}")
    return result.stdout.decode()


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, value, mode=0o600):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    content = (
        json.dumps(value, sort_keys=True, indent=2) + "\n"
        if not isinstance(value, str)
        else value
    )
    path.write_text(content)
    path.chmod(mode)


def state():
    return json.loads((PRIVATE / "state.json").read_text())


def save(value):
    write(PRIVATE / "state.json", value)


def sign(path):
    run(
        "openssl",
        "pkeyutl",
        "-sign",
        "-rawin",
        "-inkey",
        PRIVATE / "key.pem",
        "-in",
        path,
        "-out",
        str(path) + ".sig",
    )


def sql(query, database=DB):
    return run(
        "runuser",
        "-u",
        "postgres",
        "--",
        "psql",
        "-XqAt",
        "-v",
        "ON_ERROR_STOP=1",
        "-d",
        database,
        data=query.encode(),
    )


def value(unit, prop):
    return run("systemctl", "show", unit, f"--property={prop}", "--value").strip()


def ready(expected):
    for _ in range(90):
        try:
            observed = json.loads(
                run(
                    "curl",
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "5",
                    f"http://127.0.0.1:{PORT}/health/ready",
                )
            )
            if observed == {
                "status": "ok",
                "db": "ok",
                "environment": ENV,
                "release_sha": expected,
            }:
                return observed
        except (RuntimeError, ValueError):
            pass
        time.sleep(1)
    raise RuntimeError("exact real Rust readiness did not arrive")


def process_identity(expected, expected_digest):
    pid = int(value(RUST_UNIT, "MainPID"))
    if pid <= 1 or digest(f"/proc/{pid}/exe") != expected_digest:
        raise RuntimeError("actual systemd process executable digest differs")
    version = json.loads(
        run(CHECKOUT / ".runtime-rust/verdaxis-read", "--version-json")
    )
    if version["release_sha"] != expected:
        raise RuntimeError("compiled process release differs")
    return {
        "pid": pid,
        "invocation_id": value(RUST_UNIT, "InvocationID"),
        "binary_sha256": expected_digest,
        "ready": ready(expected),
    }


def prepare():
    run("useradd", "--create-home", "--shell", "/usr/sbin/nologin", "verdaxis-prod")
    if not Path("/etc/passwd").read_text().find("caddy:") >= 0:
        run(
            "useradd",
            "--system",
            "--create-home",
            "--home-dir",
            "/var/lib/caddy",
            "--shell",
            "/usr/sbin/nologin",
            "caddy",
        )
    CHECKOUT.parent.mkdir(parents=True)
    origin = CHECKOUT.parent.parent / "rehearsal-origin.git"
    run(
        "git",
        "-c",
        f"safe.directory={ROOT}",
        "clone",
        "--bare",
        "--no-local",
        ROOT,
        origin,
    )
    run("git", "-C", origin, "update-ref", "refs/heads/staging", SHA)
    run("chown", "-R", "verdaxis-prod:verdaxis-prod", origin)
    run(
        "git", "-c", f"safe.directory={origin}", "clone", "--no-local", origin, CHECKOUT
    )
    run("git", "-C", CHECKOUT, "checkout", "-B", "staging", OLD)
    # The disposable immutable origin is owned by the real deploy identity.
    run("git", "-C", CHECKOUT, "config", "remote.origin.url", str(origin))
    run(
        "git",
        "-C",
        CHECKOUT,
        "config",
        "remote.origin.fetch",
        "+refs/heads/*:refs/remotes/origin/*",
    )
    run("git", "-C", CHECKOUT, "fetch", "origin", f"{SHA}:refs/remotes/origin/staging")
    for directory in (".runtime-rust", ".runtime-deploy"):
        (CHECKOUT / directory).mkdir(mode=0o700)
    for tail in ("lock", "restart.lock"):
        write(CHECKOUT / f".runtime-deploy/{ENV}.{tail}", "")
    run("openssl", "genpkey", "-algorithm", "ED25519", "-out", PRIVATE / "key.pem")
    run(
        "openssl",
        "pkey",
        "-in",
        PRIVATE / "key.pem",
        "-pubout",
        "-out",
        PRIVATE / "key.pub.pem",
    )
    write(
        "/run/verdaxis-disposable-rehearsal",
        f"{os.environ['GITHUB_RUN_ID']}:{CHECKOUT}\n",
        0o644,
    )
    for directory in (Path("/etc/verdaxis"), Path("/var/lib/verdaxis")):
        directory.mkdir(mode=0o755 if str(directory) == "/etc/verdaxis" else 0o700)
    shutil.copyfile(
        PRIVATE / "key.pub.pem", "/etc/verdaxis/artifact-provenance.pub.pem"
    )
    Path("/etc/verdaxis/artifact-provenance.pub.pem").chmod(0o644)
    commands = []
    for action in ("restart", "stop", "disable", "enable"):
        for unit in (RUST_UNIT, f"verdaxis-backend{UNIT_SUFFIX}.service"):
            commands.append(f"/usr/bin/systemctl {action} {unit}")
    write(
        "/etc/sudoers.d/verdaxis-rehearsal",
        "verdaxis-prod ALL=(root) NOPASSWD: " + ", ".join(commands) + "\n",
        0o440,
    )
    run("visudo", "-cf", "/etc/sudoers.d/verdaxis-rehearsal")
    artifacts = {}
    for name, source, release in (
        ("previous", Path(os.environ["REHEARSAL_PREVIOUS_ARTIFACT_DIR"]), OLD),
        ("current", Path(os.environ["REHEARSAL_CURRENT_ARTIFACT_DIR"]), SHA),
    ):
        metadata = json.loads((source / "artifact-metadata.json").read_text())
        binary = source / "verdaxis-read"
        archives = list(source.glob("verdaxis-read-*.tar"))
        if (
            len(archives) != 1
            or metadata["release_sha"] != release
            or digest(binary) != metadata["binary"]["sha256"]
        ):
            raise RuntimeError("release artifact identity differs")
        binary.chmod(0o755)
        version = json.loads(run(binary, "--version-json"))
        pin = "ob_20261031_offboarding" if name == "previous" else "sqlx_1"
        if (
            version["release_sha"] != release
            or pin not in version["migration_revision_allowlist"]
        ):
            raise RuntimeError("compiled artifact pin differs")
        workflow_ref = (
            "refs/heads/staging"
            if name == "previous"
            else os.environ["REHEARSAL_WORKFLOW_REF"]
        )
        for subject, proof in ((binary, "binary.json"), (archives[0], "archive.json")):
            if not (source / "verification" / proof).is_file():
                raise RuntimeError("original GitHub signature verification missing")
            bundle = source / "attestations" / ("sha256:" + digest(subject) + ".jsonl")
            run(
                "gh",
                "attestation",
                "verify",
                subject,
                "--bundle",
                bundle,
                "--repo",
                "Verdaxis/verdaxis-backend",
                "--source-digest",
                release,
                "--source-ref",
                workflow_ref,
                "--signer-digest",
                release,
                "--cert-identity",
                "https://github.com/Verdaxis/verdaxis-backend/.github/workflows/rust-read-ci.yml@"
                + workflow_ref,
                "--deny-self-hosted-runners",
            )
        target = PRIVATE / name
        target.mkdir()
        shutil.copyfile(archives[0], target / "archive.tar")
        record = {
            "version": 1,
            "repository": "Verdaxis/verdaxis-backend",
            "workflow_path": ".github/workflows/rust-read-ci.yml",
            "workflow_ref": workflow_ref,
            "source_sha": release,
            "target": "x86_64-unknown-linux-gnu",
            "archive_sha256": digest(target / "archive.tar"),
            "binary_sha256": digest(binary),
            "attested_subjects": ["verdaxis-read", "archive"],
            "gh_attestation_verified": True,
            "attested_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
        # Current candidate authority is limited to this disposable runner. It is
        # never a release-key signature or an installed-host acceptance record.
        write(target / "provenance.json", record)
        sign(target / "provenance.json")
        output = target / "verified"
        output.mkdir()
        run(
            "bash",
            ROOT / "deploy/scripts/verify-artifact.sh",
            "--archive",
            target / "archive.tar",
            "--source-sha",
            release,
            "--output-dir",
            output,
            "--provenance-record",
            target / "provenance.json",
            "--provenance-signature",
            target / "provenance.json.sig",
            "--provenance-public-key",
            PRIVATE / "key.pub.pem",
            "--workflow-ref",
            workflow_ref,
        )
        artifacts[name] = {
            "binary_sha256": digest(output / "verdaxis-read"),
            "archive_sha256": digest(target / "archive.tar"),
        }
    password = secrets.token_hex(24)
    sql(
        f"CREATE ROLE {APP} LOGIN NOINHERIT PASSWORD '{password}'; CREATE ROLE {MIGRATOR} LOGIN NOINHERIT PASSWORD '{password}'; CREATE ROLE {BACKUP} LOGIN NOINHERIT PASSWORD '{password}';",
        "postgres",
    )
    sql(f"CREATE DATABASE {DB} OWNER {MIGRATOR};", "postgres")
    sql(f"CREATE EXTENSION postgis; ALTER SCHEMA public OWNER TO {MIGRATOR};")
    app_url = f"postgresql://{APP}:{password}@127.0.0.1:5432/{DB}"
    migration_url = f"postgresql://{MIGRATOR}:{password}@127.0.0.1:5432/{DB}"
    jwt_secret = secrets.token_hex(32)
    run(
        "runuser",
        "-u",
        "postgres",
        "--",
        "psql",
        "-Xq",
        "--single-transaction",
        "-v",
        "ON_ERROR_STOP=1",
        "-d",
        DB,
        data=schema_sql(ROOT, MIGRATOR),
    )
    run(
        "runuser",
        "-u",
        "postgres",
        "--",
        "psql",
        "-X",
        "--single-transaction",
        "-v",
        "ON_ERROR_STOP=1",
        "-d",
        DB,
        "-v",
        f"database_name={DB}",
        "-v",
        f"app_role={APP}",
        "-v",
        f"migrator_role={MIGRATOR}",
        "-v",
        f"backup_role={BACKUP}",
        "-f",
        ROOT / "deploy/postgres/converge_runtime_object_acls.sql",
    )
    if sql("SELECT version_num FROM alembic_version;").strip() != REVISION:
        raise RuntimeError("seeded schema is not final offboarding")
    # Copy the actual retained package, resolving links at copy time. No new
    # executable or library is manufactured to satisfy the closure checker.
    shutil.copytree(ROOT / "venv", CHECKOUT / "venv", symlinks=False)
    for path in (CHECKOUT / "venv").rglob("*"):
        if path.is_file():
            path.chmod(0o700 if os.access(path, os.X_OK) else 0o600)
    runtime_env = (
        f"ENVIRONMENT={ENV}\nRELEASE_SHA={OLD}\nDATABASE_URL={app_url.replace('postgresql:', 'postgresql+asyncpg:')}\nMIGRATOR_DATABASE_URL={migration_url.replace('postgresql:', 'postgresql+asyncpg:')}\n"
        f"DATABASE_HOST=127.0.0.1\nDATABASE_NAME={DB}\nDATABASE_USER={APP}\nDATABASE_PASSWORD={password}\n"
        f"JWT_SECRET={jwt_secret}\nJWT_ISSUER=verdaxis-rehearsal\nJWT_AUDIENCE=verdaxis-rehearsal\n"
        f"FRONTEND_URL=https://{DOMAIN}\nEMAIL_FROM=rehearsal@example.invalid\nRESEND_API_KEY=disposable-never-send\n"
        "KYC_EXECUTION_GATE_ENABLED=false\nRUST_CACHE_LISTENER=0\n"
        "DB_SERVICE_COUNT=2\nUVICORN_WORKERS=4\nDB_POOL_SIZE=2\nDB_MAX_OVERFLOW=1\nDB_RESERVED_CONNECTIONS=20\nDB_MAX_CONNECTIONS=100\n"
    )
    write(CHECKOUT / ".env", runtime_env)
    shutil.copyfile(
        PRIVATE / "previous/verified/verdaxis-read",
        CHECKOUT / ".runtime-rust/verdaxis-read",
    )
    (CHECKOUT / ".runtime-rust/verdaxis-read").chmod(0o755)
    run(
        ROOT / "venv/bin/python",
        CHECKOUT / "scripts/prepare_rust_budget.py",
        "--environment-file",
        CHECKOUT / ".env",
        "--release-sha",
        OLD,
        "--stream-listener",
        "1",
        "--output",
        CHECKOUT / ".runtime-rust/budget.json",
        cwd=CHECKOUT,
    )
    run(
        ROOT / "venv/bin/python",
        CHECKOUT / "scripts/prepare_rust_runtime.py",
        "--environment-file",
        CHECKOUT / ".env",
        "--release-sha",
        OLD,
        "--environment",
        ENV,
        "--source-root",
        CHECKOUT,
        "--migration-revision",
        "ob_20261031_offboarding",
        "--output-dir",
        CHECKOUT / ".runtime-rust",
        cwd=CHECKOUT,
    )
    write(
        CHECKOUT / ".runtime-release.env",
        f"ENVIRONMENT={ENV}\nRELEASE_SHA={OLD}\nMIGRATION_REVISION=ob_20261031_offboarding\n",
    )
    write(CHECKOUT / ".runtime-monitor.env", f"ENVIRONMENT={ENV}\nRELEASE_SHA={OLD}\n")
    write(
        CHECKOUT / ".runtime-rust/revision.json",
        {"release_sha": OLD, "revision": "ob_20261031_offboarding"},
    )
    shutil.copyfile(
        CHECKOUT / f"deploy/rust/{ENV}-policy.json",
        CHECKOUT / f".runtime-rust/{ENV}-policy.json",
    )
    write(f"/etc/verdaxis/kyc-document-keys{UNIT_SUFFIX}.json", "", 0o400)
    for source in (CHECKOUT / "deploy/systemd").glob("verdaxis-*"):
        if source.suffix in (".service", ".timer"):
            shutil.copyfile(source, Path("/etc/systemd/system") / source.name)
    run("chown", "-R", "verdaxis-prod:verdaxis-prod", CHECKOUT)
    run("chown", "verdaxis-prod:verdaxis-prod", CHECKOUT.parent, CHECKOUT.parent.parent)
    run(
        "chmod",
        "0750",
        "/home/verdaxis-prod",
        CHECKOUT.parent.parent,
        CHECKOUT.parent,
        CHECKOUT,
    )
    run(
        "setfacl",
        "-m",
        "u:caddy:--x",
        "/home/verdaxis-prod",
        CHECKOUT.parent.parent,
        CHECKOUT.parent,
        CHECKOUT,
    )
    run("systemctl", "daemon-reload")
    run("systemctl", "start", RUST_UNIT)
    observation = process_identity(OLD, artifacts["previous"]["binary_sha256"])
    from companion import prepare as prepare_companion

    companion = prepare_companion(
        ROOT,
        PRIVATE,
        CHECKOUT,
        ENV,
        OLD,
        PRIVATE / "previous/verified/verdaxis-read",
        password,
    )
    save(
        {
            "artifacts": artifacts,
            "password": password,
            "old_process": observation,
            "companion": companion,
        }
    )
    from caddy import prepare as prepare_caddy

    prepare_caddy(sys.modules[__name__])
    from probes import seed

    seed(DB, PRIVATE)
    schedules = run("bash", ROOT / "deploy/tests/rehearsal/schedules.sh")
    write(PRIVATE / "schedules-before.json", schedules)
    write(
        PUBLIC / "artifacts.json",
        {
            "scope": "disposable-candidate",
            "previous_release_sha": OLD,
            "current_release_sha": SHA,
            "original_previous_run": "42034562253",
            "artifacts": artifacts,
            "old_process": observation,
        },
    )


def inventory():
    observations = {}
    observations["units"] = run(
        "systemctl", "list-unit-files", "--no-pager", "--no-legend", "verdaxis-*"
    )
    observations["timers"] = run(
        "systemctl", "list-timers", "--all", "--no-pager", "--no-legend"
    )
    observations["dropins"] = {
        str(p): digest(p)
        for p in Path("/etc/systemd/system").glob("*.d/*.conf")
        if p.is_file()
    }
    observations["cron"] = {
        str(p): digest(p)
        for parent in ("/etc/cron.d", "/var/spool/cron/crontabs")
        for p in Path(parent).glob("*")
        if p.is_file()
    }
    observations["usr_local"] = {
        str(p): digest(p)
        for parent in ("/usr/local/sbin", "/usr/local/libexec")
        for p in Path(parent).rglob("*")
        if p.is_file() and not p.is_symlink()
    }
    observations["processes"] = run("ps", "-eo", "pid=,uid=,comm=")
    observations["backup"] = {
        str(p): digest(p)
        for p in Path("/etc/systemd/system").glob("*backup*")
        if p.is_file()
    }
    required = (
        "units",
        "timers",
        "dropins",
        "cron",
        "usr_local",
        "backup",
        "processes",
    )
    if any(key not in observations for key in required):
        raise RuntimeError("inventory category collection incomplete")
    if not all(
        observations[key]
        for key in ("units", "timers", "cron", "usr_local", "backup", "processes")
    ):
        raise RuntimeError("required installed disposable inventory is empty")
    observations["schedules"] = json.loads(
        (PRIVATE / "schedules-before.json").read_text()
    )
    observations["collected_at"] = int(time.time())
    observations["scope"] = "disposable-candidate"
    observations["environment"] = ENV
    write(PRIVATE / "host-inventory.json", observations)
    sign(PRIVATE / "host-inventory.json")
    run(
        "openssl",
        "pkeyutl",
        "-verify",
        "-pubin",
        "-rawin",
        "-inkey",
        PRIVATE / "key.pub.pem",
        "-in",
        PRIVATE / "host-inventory.json",
        "-sigfile",
        PRIVATE / "host-inventory.json.sig",
    )
    # Public inventory contains hashes and structural counts, not raw cron,
    # process arguments, credentials, or unit environment material.
    write(
        PUBLIC / "inventory.json",
        {
            "scope": "disposable-candidate",
            "environment": ENV,
            "inventory_sha256": digest(PRIVATE / "host-inventory.json"),
            "signature_sha256": digest(PRIVATE / "host-inventory.json.sig"),
            "signer_sha256": digest(PRIVATE / "key.pub.pem"),
            "coverage": {
                key: {"collected": True, "count": len(observations[key])}
                for key in required
            },
            "schedule_baseline_sha256": digest(PRIVATE / "schedules-before.json"),
            "collected_at": observations["collected_at"],
        },
    )


def build_bundle_inventory():
    from bundle import build

    return build(sys.modules[__name__])


def bundle_arguments(action, inventory=False, acceptance=None):
    arguments = [
        "bash",
        PRIVATE / "trusted/scripts/pre-m-bundle.sh",
        action,
        "--environment",
        ENV,
        "--release-sha",
        OLD,
        "--bundle",
        PRIVATE / f"pre-M-{ENV}-{OLD}",
        "--reader-service-file",
        PRIVATE / "reader.service",
        "--acceptance-public-key",
        PRIVATE / "key.pub.pem",
    ]
    if inventory:
        arguments += [
            "--inventory",
            PRIVATE / "bundle-inventory.json",
            "--manifest-sha256",
            digest(PRIVATE / "bundle-inventory.json"),
        ]
    else:
        arguments += ["--manifest-sha256", state()["manifest_sha256"]]
    record = Path(acceptance) if acceptance else PRIVATE / "closure.json"
    arguments += [
        "--acceptance",
        record,
        "--acceptance-signature",
        str(record) + ".sig",
    ]
    return arguments


def bundle_command(action, inventory=False, acceptance=None, kill_point=None):
    environment = dict(os.environ)
    if kill_point:
        environment["PRE_M_KILL_POINT"] = kill_point
    return run(*bundle_arguments(action, inventory, acceptance), env=environment)


def deploy_current(expected_current="ob_20261031_offboarding", kill_point=None):
    # The exact reviewed helper performs the real adoption, ACL convergence,
    # runtime publication and systemd restart; this producer signs only after
    # concrete HTTP, process, monitor and passive observations have succeeded.
    allowed = (
        "CI",
        "GITHUB_ACTIONS",
        "RUNNER_ENVIRONMENT",
        "GITHUB_RUN_ID",
        "REHEARSAL_WORKFLOW_REF",
        "REHEARSAL_CHECKOUT",
        "REHEARSAL_PUBLIC_KEY",
        "VERDAXIS_DISPOSABLE_DEPLOY",
    )
    environment = {key: os.environ[key] for key in allowed}
    environment["PATH"] = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    transfer = (
        Path("/var/lib") / f"verdaxis-rehearsal-transfer-{os.environ['GITHUB_RUN_ID']}"
    )
    transfer.mkdir(mode=0o755, exist_ok=True)
    for name in ("archive.tar", "provenance.json", "provenance.json.sig"):
        target = transfer / name
        shutil.copyfile(PRIVATE / "current" / name, target)
        target.chmod(0o644)
    acceptance = transfer / "deploy-acceptance.json"
    for path in (acceptance, Path(str(acceptance) + ".sig")):
        path.unlink(missing_ok=True)
    environment.update(
        APPROVED_RELEASE_SHA=SHA,
        MIGRATION_APPROVED_SOURCE_SHA=SHA,
        MIGRATION_EXPECTED_CURRENT_REVISION=expected_current,
        MIGRATION_TARGET_REVISION="sqlx_1",
        RUST_ARTIFACT_ARCHIVE=str(transfer / "archive.tar"),
        RUST_PROVENANCE_RECORD=str(transfer / "provenance.json"),
        RUST_PROVENANCE_SIGNATURE=str(transfer / "provenance.json.sig"),
        DEPLOY_ACCEPTANCE_RECORD=str(acceptance),
        DEPLOY_ACCEPTANCE_SIGNATURE=str(acceptance) + ".sig",
        DEPLOY_PUBLICATION_LOG=str(
            CHECKOUT / ".runtime-deploy/deploy-publications.log"
        ),
        HEALTH_ATTEMPTS="180",
        HEALTH_RETRY_DELAY="2",
    )
    if kill_point:
        environment["DEPLOY_KILL_POINT"] = kill_point
    command = (
        ["runuser", "-u", "verdaxis-prod", "--", "env", "-i"]
        + [f"{key}={item}" for key, item in environment.items()]
        + ["bash", str(CHECKOUT / "scripts/deploy.sh")]
    )
    run(*command, "--dry-run")
    output = PRIVATE / "deploy-output.log"
    with output.open("wb") as stream:
        process = subprocess.Popen(
            command,
            stdout=stream,
            stderr=subprocess.STDOUT,
            env=environment,
        )
        deadline = time.monotonic() + 600
        summary = None
        while process.poll() is None and time.monotonic() < deadline:
            text = output.read_text(errors="replace")
            nonce = __import__("re").search(
                r"^DEPLOY_ACCEPTANCE_NONCE=([a-f0-9]{64})$", text, __import__("re").M
            )
            if nonce and not acceptance.exists():
                from caddy import activate

                activate(sys.modules[__name__])
                observation = process_identity(
                    SHA, state()["artifacts"]["current"]["binary_sha256"]
                )
                from probes import probe
                from w2 import known_demo_baseline

                known_demo_baseline(sys.modules[__name__])
                summary = probe(
                    CHECKOUT,
                    ENV,
                    SHA,
                    PRIVATE,
                    PUBLIC / f"forward-probes-{time.time_ns()}.json",
                    require_empty=False,
                )
                if any(
                    summary[key] != "PASS"
                    for key in ("auth", "session", "passive_harm")
                ):
                    process.terminate()
                    process.wait(timeout=30)
                    raise RuntimeError("forward HTTP/session/passive acceptance failed")
                publish_monitor_identity(SHA)
                monitors = monitor_runs()
                record = {
                    "auth": True,
                    "session": True,
                    "monitor": True,
                    "passive_harm": True,
                    "binary_sha256": observation["binary_sha256"],
                    "deploy_nonce": nonce.group(1),
                    "environment": ENV,
                    "expected_current": expected_current,
                    "release_sha": SHA,
                    "target": "sqlx_1",
                }
                write(acceptance, record, 0o644)
                sign(acceptance)
                Path(str(acceptance) + ".sig").chmod(0o644)
            time.sleep(0.25)
        if process.poll() is None:
            process.kill()
            process.wait()
            raise RuntimeError("deploy exceeded bounded acceptance wait")
        if kill_point:
            side, name = kill_point.split(":", 1)
            publication_log = CHECKOUT / ".runtime-deploy/deploy-publications.log"
            last = (
                publication_log.read_text().splitlines()[-1]
                if publication_log.exists()
                else ""
            )
            if (
                process.returncode not in (-9, 137)
                or last != f"{side} {name} {ENV} {SHA}"
            ):
                raise RuntimeError(
                    "requested deploy boundary did not kill actual helper"
                )
            return {
                "kill_point": kill_point,
                "exit_status": process.returncode,
                "killed_at_observed_boundary": True,
            }
        if process.returncode or summary is None:
            raise RuntimeError("real deploy did not reach signed acceptance")
    if (CHECKOUT / f".runtime-deploy/{ENV}.state").exists():
        raise RuntimeError("accepted deploy retained blocked state")
    return {"process": observation, "probes": summary, "monitors": monitors}


def publish_monitor_identity(release):
    path = Path("/etc/verdaxis-monitor/runtime-identities") / f"{ENV}.identity"
    temporary = path.with_suffix(".pending")
    write(temporary, f"ENVIRONMENT={ENV}\nRELEASE_SHA={release}\n", 0o644)
    os.replace(temporary, path)


def monitor_runs():
    observations = []
    for unit in ("verdaxis-health.service", "verdaxis-backup-verify.service"):
        before = value(unit, "InvocationID")
        run("systemctl", "start", unit)
        invocation = value(unit, "InvocationID")
        if (
            invocation == before
            or value(unit, "Result") != "success"
            or value(unit, "ExecMainStatus") != "0"
        ):
            raise RuntimeError(
                "real monitor did not complete a fresh successful invocation"
            )
        observations.append({"unit": unit, "invocation_id": invocation})
    return observations


def capture():
    # The trusted recovery utility and its siblings remain outside source swaps.
    trusted = PRIVATE / "trusted"
    (trusted / "scripts").mkdir(parents=True)
    (trusted / "rust").mkdir()
    for name in ("pre-m-bundle.sh", "verify-artifact.sh"):
        shutil.copyfile(ROOT / "deploy/scripts" / name, trusted / "scripts" / name)
    shutil.copyfile(
        ROOT / "deploy/rust/catalog-fingerprint.sql",
        trusted / "rust/catalog-fingerprint.sql",
    )
    build_bundle_inventory()
    bundle_command("capture", inventory=True)
    manifest = PRIVATE / f"pre-M-{ENV}-{OLD}" / "manifest.json"
    saved = state()
    saved["manifest_sha256"] = digest(manifest)
    save(saved)
    bundle_command("verify")


def window():
    from w2 import run_window

    run_window(sys.modules[__name__])


def rollback():
    inventory = json.loads((PRIVATE / "bundle-inventory.json").read_text())
    for unit in inventory["consumer_units"]:
        if unit["name"] != RUST_UNIT:
            run("systemctl", "stop", unit["name"])
    output = bundle_command("restore")
    write(PRIVATE / "restore-output.log", output)
    observation = process_identity(
        OLD, state()["artifacts"]["previous"]["binary_sha256"]
    )
    from probes import probe
    from w2 import known_demo_baseline

    known_demo_baseline(sys.modules[__name__])
    summary = probe(
        CHECKOUT,
        ENV,
        OLD,
        PRIVATE,
        PUBLIC / f"rollback-probes-{time.time_ns()}.json",
        require_empty=False,
    )
    if any(summary[key] != "PASS" for key in ("auth", "session", "passive_harm")):
        raise RuntimeError("actual old HTTP/session/passive acceptance failed")
    publish_monitor_identity(OLD)
    monitors = monitor_runs()
    nonce = (CHECKOUT / f".runtime-deploy/{ENV}.rollback-nonce").read_text().strip()
    # Catalog and audit are checked by the independent read-only reader in
    # restore/accept. An actual migration credential route must be absent.
    if (CHECKOUT / ".env").exists():
        raise RuntimeError("old migrator credential route was restored")
    version = json.loads(
        run(CHECKOUT / ".runtime-rust/verdaxis-read", "--version-json")
    )
    if "ob_20261031_offboarding" not in version["migration_revision_allowlist"]:
        raise RuntimeError("old executable cannot admit retained audit pin")
    record = {
        "environment": ENV,
        "release_sha": OLD,
        "manifest_sha256": state()["manifest_sha256"],
        "nonce": nonce,
        "invocation_id": observation["invocation_id"],
        "auth": "PASS",
        "session": "PASS",
        "monitor": "PASS",
        "passive": "PASS",
        "old_compiled_pin": "PASS",
        "migration_freeze": "PASS",
        "monitor_invocations": monitors,
    }
    write(PRIVATE / "rollback-acceptance.json", record)
    sign(PRIVATE / "rollback-acceptance.json")
    bundle_command("accept", acceptance=PRIVATE / "rollback-acceptance.json")
    if (CHECKOUT / f".runtime-deploy/{ENV}.state").exists():
        raise RuntimeError("old release acceptance did not clear blocked state")
    write(
        PUBLIC / "rollback.json",
        {
            "scope": "disposable-candidate",
            "release_sha": OLD,
            "process": observation,
            "manifest_sha256": state()["manifest_sha256"],
            "probes": summary,
            "monitor_invocations": monitors,
            "migration_credentials_absent": True,
        },
    )


def interruptions():
    # Discover the real publication set from a complete current -> old restore.
    # Every replay begins from the same accepted current deployment. Numbered
    # boundaries are checked against their destination, not assumed constants.
    forward()
    inventory = json.loads((PRIVATE / "bundle-inventory.json").read_text())
    for row in inventory["consumer_units"]:
        if row["name"] != RUST_UNIT:
            run("systemctl", "stop", row["name"])
    log = PRIVATE / "restore-discovery.log"
    arguments = bundle_arguments("restore")
    with log.open("wb") as stream:
        result = subprocess.run(
            [str(a) for a in arguments],
            stdout=stream,
            stderr=subprocess.STDOUT,
            check=False,
            env=dict(os.environ),
        )
    if result.returncode:
        raise RuntimeError("real publication discovery restore failed")
    destinations = __import__("re").findall(
        r"^PUBLICATION before destination=(.+)$", log.read_text(), __import__("re").M
    )
    if not destinations:
        raise RuntimeError("no real restore publication boundaries observed")
    rollback()
    cells = []
    for index, destination in enumerate(destinations, 1):
        for side in ("before", "after"):
            forward()
            for row in inventory["consumer_units"]:
                if row["name"] != RUST_UNIT:
                    run("systemctl", "stop", row["name"])
            environment = dict(os.environ, PRE_M_KILL_POINT=f"{side}:{index}")
            cell_log = PRIVATE / f"restore-kill-{side}-{index}.log"
            with cell_log.open("wb") as stream:
                result = subprocess.run(
                    [str(a) for a in bundle_arguments("restore")],
                    stdout=stream,
                    stderr=subprocess.STDOUT,
                    check=False,
                    env=environment,
                )
            observed = cell_log.read_text()
            expected_line = (
                f"DISPOSABLE_PUBLICATION {side}:{index} destination={destination}"
            )
            if result.returncode != -9 or expected_line not in observed:
                raise RuntimeError(
                    "actual restore boundary differs from discovered matrix"
                )
            # A killed restore must not let systemd start any mixed package.
            attempted = subprocess.run(
                ["systemctl", "restart", RUST_UNIT],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            coherent_process = None
            if attempted.returncode == 0 or value(RUST_UNIT, "ActiveState") == "active":
                # Before the first blocked-state publication, the complete
                # accepted current package remains a legitimate start target.
                if side != "before" or index != 1:
                    raise RuntimeError(
                        "interrupted restore admitted a process after blocking"
                    )
                coherent_process = process_identity(
                    SHA, state()["artifacts"]["current"]["binary_sha256"]
                )
            else:
                run("systemctl", "reset-failed", RUST_UNIT)
            rollback()
            cells.append(
                {
                    "side": side,
                    "index": index,
                    "destination_sha256": hashlib.sha256(
                        destination.encode()
                    ).hexdigest(),
                    "killed": True,
                    "mixed_start_refused": coherent_process is None,
                    "coherent_start": coherent_process,
                    "actual_old_replay_accepted": True,
                    "protected_log_sha256": digest(cell_log),
                }
            )
            write(
                PUBLIC / "interruptions-progress.json",
                {
                    "scope": "disposable-candidate",
                    "completed": cells,
                    "total": 2 * len(destinations),
                },
            )
    deploy_cells = []
    for name in (
        "cache-revoke",
        "source",
        "budget",
        "release",
        "monitor",
        "policy",
        "revision",
        "runtime",
        "cache-bind",
        "binary",
        "authorization",
        "clear",
    ):
        for side in ("before", "after"):
            forward()
            result = deploy_current("sqlx_1", f"{side}:{name}")
            if name != "clear" or side != "after":
                attempted = subprocess.run(
                    ["systemctl", "restart", RUST_UNIT],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                )
                if attempted.returncode == 0:
                    raise RuntimeError("interrupted deploy admitted a restart")
            else:
                process_identity(SHA, state()["artifacts"]["current"]["binary_sha256"])
            rollback()
            deploy_cells.append(
                {
                    "point": f"{side}:{name}",
                    "killed": result["killed_at_observed_boundary"],
                    "replay_accepted": True,
                }
            )
    write(
        PUBLIC / "interruptions.json",
        {
            "scope": "disposable-candidate",
            "restore_publications": len(destinations),
            "restore_cells": cells,
            "deploy_cells": deploy_cells,
            "expected_restore_cells": 2 * len(destinations),
            "expected_deploy_cells": 24,
        },
    )


def forward():
    # Restore intentionally withdraws .env. Recreate authority only from the
    # protected disposable escrow before a separately approved forward deploy.
    inventory = json.loads((PRIVATE / f"pre-M-{ENV}-{OLD}/manifest.json").read_text())
    record = next(
        row
        for row in inventory["files"]
        if row["kind"] == "migrator_credential"
        and row["path"] == str(CHECKOUT / ".env")
    )
    shutil.copyfile(
        PRIVATE / f"pre-M-{ENV}-{OLD}/payload" / record["id"], CHECKOUT / ".env"
    )
    (CHECKOUT / ".env").chmod(int(record["mode"], 8))
    os.chown(CHECKOUT / ".env", record["uid"], record["gid"])
    run("systemctl", "stop", RUST_UNIT)
    run(
        "git",
        "-c",
        f"safe.directory={CHECKOUT}",
        "-C",
        CHECKOUT,
        "checkout",
        "--detach",
        SHA,
    )
    run(
        "git",
        "-c",
        f"safe.directory={CHECKOUT}",
        "-C",
        CHECKOUT,
        "checkout",
        "-B",
        "staging",
        SHA,
    )
    active = CHECKOUT / ".runtime-rust/verdaxis-read"
    temporary = active.with_suffix(".forward-pending")
    shutil.copyfile(PRIVATE / "current/verified/verdaxis-read", temporary)
    temporary.chmod(0o755)
    owner = CHECKOUT.stat()
    os.chown(temporary, owner.st_uid, owner.st_gid)
    os.replace(temporary, active)
    from w2 import ensure_native

    ensure_native(sys.modules[__name__])
    run("systemctl", "daemon-reload")
    deploy_current("sqlx_1")


if __name__ == "__main__":
    actions = {
        name: globals()[name]
        for name in (
            "prepare",
            "inventory",
            "capture",
            "window",
            "rollback",
            "interruptions",
            "forward",
        )
    }
    if len(sys.argv) != 2 or sys.argv[1] not in actions:
        raise SystemExit(2)
    actions[sys.argv[1]]()
