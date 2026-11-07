"""Exercise ordered ownership transfer on the disposable systemd runner.

Installation precedes M adoption. Activation requires the current real runtime.
The caller retains the old release bundle before either entry point is used.
"""

from __future__ import annotations

import hashlib
import ipaddress
import json
import os
import pwd
import re
import shutil
import socket
import stat
import time
from pathlib import Path

GROUPS = (
    "news",
    "auth-prune",
    "reminders",
    "onboarding",
    "demo",
    "local-monitor",
    "external-recovery",
)
UNIT_DIRECTORY = Path("/etc/systemd/system")
MONITOR_DIRECTORIES = (
    Path("/var/lib/verdaxis-health"),
    Path("/var/lib/verdaxis-backup-verify"),
)
EXTERNAL_NAMES = (
    "verdaxis.exchange",
    "app.verdaxis.exchange",
    "staging.verdaxis.exchange",
    "api.verdaxis.exchange",
    "api-staging.verdaxis.exchange",
    "deck.verdaxis.exchange",
)
EFFECT_TABLES = (
    "news_items",
    "refresh_sessions",
    "pending_registrations",
    "user_login_days",
    "notifications",
    "organizations",
    "orderbook_orders",
    "trades",
)
PROPERTIES = (
    "LoadState",
    "ActiveState",
    "SubState",
    "Job",
    "InvocationID",
    "Result",
    "ExecMainStatus",
    "ConditionResult",
    "AssertResult",
    "User",
    "Group",
    "FragmentPath",
    "DropInPaths",
    "ExecStart",
    "NeedDaemonReload",
)


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def value(s, unit, name):
    return s.run("systemctl", "show", unit, f"--property={name}", "--value").strip()


def unit_names(environment):
    suffix = "" if environment == "production" else "-staging"
    return {
        "news": [f"verdaxis-news-refresh{suffix}.service"],
        "auth-prune": [
            f"verdaxis-auth-maintenance{suffix}.service",
            f"verdaxis-product-analytics-prune{suffix}.service",
        ],
        "reminders": ["verdaxis-order-expiry-reminders.service"] if not suffix else [],
        "onboarding": ["verdaxis-onboarding-attention.service"] if not suffix else [],
        "demo": [f"verdaxis-demo-activity{suffix}.service"],
        "local-monitor": [
            "verdaxis-health.service",
            "verdaxis-backup-verify.service",
            "verdaxis-monitor-alert@health.service",
            "verdaxis-monitor-alert@backup.service",
            "verdaxis-monitor-alert-reminder.service",
        ],
        "external-recovery": [
            "verdaxis-monitor.service",
            "verdaxis-monitor-verify.service",
        ],
    }


def timer_names(group, services):
    if group == "local-monitor":
        return [
            "verdaxis-health.timer",
            "verdaxis-backup-verify.timer",
            "verdaxis-monitor-alert-reminder.timer",
        ]
    if group == "external-recovery":
        return ["verdaxis-monitor.timer"]
    return [name.removesuffix(".service") + ".timer" for name in services]


def unit_file(unit):
    if "@" in unit:
        unit = unit.split("@", 1)[0] + "@.service"
    return UNIT_DIRECTORY / unit


def source_file(s, unit):
    name = unit_file(unit).name
    if name.startswith(
        ("verdaxis-news-", "verdaxis-auth-", "verdaxis-product-", "verdaxis-order-")
    ):
        directory = "deploy/systemd"
    elif name.startswith("verdaxis-onboarding-") or name in {
        "verdaxis-monitor.service",
        "verdaxis-monitor.timer",
        "verdaxis-monitor-verify.service",
    }:
        directory = "deploy/external_monitor/systemd"
    else:
        directory = "deploy/monitor"
    return s.CHECKOUT / directory / name


def file_record(s, path):
    path = Path(path)
    require(
        path.is_file() and not path.is_symlink(), "expected a regular ownership input"
    )
    metadata = path.stat()
    return {
        "path": str(path),
        "sha256": s.digest(path),
        "uid": metadata.st_uid,
        "gid": metadata.st_gid,
        "mode": f"{stat.S_IMODE(metadata.st_mode):03o}",
    }


def observation(s, unit):
    return {name: value(s, unit, name) for name in PROPERTIES}


def successful_run(s, unit, previous):
    row = observation(s, unit)
    invocation = row["InvocationID"]
    require(
        re.fullmatch(r"[0-9a-f]{32}", invocation) is not None
        and invocation != "0" * 32
        and invocation != previous,
        f"no fresh invocation for {unit}",
    )
    require(
        row["Result"] == "success"
        and row["ExecMainStatus"] == "0"
        and row["ConditionResult"] == "yes"
        and row["AssertResult"] == "yes",
        f"unit did not complete its actual command: {unit}",
    )
    require(
        row["Job"] in ("", "0") and row["ActiveState"] == "inactive",
        f"oneshot did not drain: {unit}",
    )
    require(row["NeedDaemonReload"] == "no", "installed unit cache is stale")
    require(
        unit_file(unit).read_bytes() == source_file(s, unit).read_bytes(),
        "executed unit bytes differ from reviewed source",
    )
    return row


def guard_runner(s):
    require(
        os.geteuid() == 0
        and os.environ.get("CI") == "true"
        and os.environ.get("GITHUB_ACTIONS") == "true"
        and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted",
        "W2 requires root on the disposable hosted runner",
    )
    require(
        Path("/proc/1/comm").read_text().strip() == "systemd", "systemd is not PID 1"
    )
    require(s.ENV in ("production", "staging"), "invalid environment")
    expected = (
        Path("/home/verdaxis-prod/verdaxis")
        / ("prod" if s.ENV == "production" else "staging")
        / "be"
    )
    require(
        s.CHECKOUT == expected and s.CHECKOUT.resolve() == expected,
        "W2 requires the canonical disposable checkout",
    )
    marker = Path("/run/verdaxis-disposable-rehearsal")
    require(
        marker.is_file() and str(expected) in marker.read_text(),
        "disposable runner marker differs",
    )
    require(
        re.fullmatch(r"[0-9a-f]{40}", s.SHA) is not None,
        "invalid current source identity",
    )


def quiesce(s, group, services, *, stop_timers=True):
    timers = timer_names(group, services)
    if stop_timers:
        for timer in timers:
            s.run("systemctl", "stop", timer)
    for _ in range(120):
        busy = [
            unit
            for unit in services
            if value(s, unit, "ActiveState") not in ("inactive", "failed")
            or value(s, unit, "Job") not in ("", "0")
        ]
        if not busy:
            break
        time.sleep(1)
    else:
        raise RuntimeError(
            "native work did not drain; retained launchers remain fenced"
        )
    if stop_timers:
        require(
            all(value(s, timer, "ActiveState") == "inactive" for timer in timers),
            "a direct scheduler is still active",
        )
    return {timer: observation(s, timer) for timer in timers}


def pending_callers(s, launchers):
    paths = set(map(str, launchers))
    observed = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            arguments = (entry / "cmdline").read_bytes().split(b"\x00")
        except FileNotFoundError:
            continue
        matches = sorted(
            paths.intersection(
                argument.decode(errors="replace") for argument in arguments
            )
        )
        require(not matches, "an inventoried direct launcher is still running")
        observed.append(int(entry.name))
    schedulers = {}
    for directory in (Path("/etc/cron.d"), Path("/var/spool/cron/crontabs")):
        if not directory.exists():
            continue
        for path in directory.iterdir():
            if not path.is_file():
                continue
            text = path.read_text()
            require(
                not any(launcher in text for launcher in paths),
                "a direct cron caller remains enabled",
            )
            schedulers[str(path)] = s.digest(path)
    return {"process_count": len(observed), "scheduler_sha256": schedulers}


def table_snapshot(s):
    records = {}
    for table in EFFECT_TABLES:
        # Hash row text privately. Only counts and hashes reach public evidence.
        query = (
            "SELECT json_build_object('count', count(*), 'rows', "
            "coalesce(string_agg(row_text, E'\\n' ORDER BY row_text COLLATE \"C\"), '')) "
            f"FROM (SELECT row_to_json(t)::text AS row_text FROM public.{table} t) rows;"
        )
        record = json.loads(s.sql(query))
        records[table] = {
            "count": record["count"],
            "sha256": hashlib.sha256(record["rows"].encode()).hexdigest(),
        }
    queries = {
        "news_url": "SELECT count(*) FROM (SELECT url FROM news_items GROUP BY url HAVING count(*) > 1) d",
        "expiry_notice": "SELECT count(*) FROM (SELECT recipient_id, data::jsonb->>'order_key', data::jsonb->>'expiry_epoch' FROM notifications WHERE type::text = 'ORDER_UPDATE' AND data::jsonb ? 'expiry_epoch' GROUP BY 1,2,3 HAVING count(*) > 1) d",
        "demo_coverage": "SELECT count(*) FROM (SELECT organization_id, idempotency_operation, idempotency_key FROM orderbook_orders WHERE provenance::text = 'DEMO' AND idempotency_key IS NOT NULL GROUP BY 1,2,3 HAVING count(*) > 1) d",
        "demo_tick": "SELECT count(*) FROM (SELECT buyer_id, seller_id, created_at FROM trades WHERE buyer_provenance::text = 'DEMO' AND seller_provenance::text = 'DEMO' GROUP BY 1,2,3 HAVING count(*) > 1) d",
    }
    counts = {name: int(s.sql(query).strip()) for name, query in queries.items()}
    require(
        all(count == 0 for count in counts.values()),
        "persisted duplicate job effects found",
    )
    return {"tables": records, "duplicate_counts": counts}


def journals(s, *, released=False):
    rows = {}
    for check, parent in zip(("health", "backup"), MONITOR_DIRECTORIES):
        lock = parent / f".{check}.journal.lock"
        delivery_lock = Path("/var/lib/verdaxis-monitor-alert") / f".{check}.lock"
        require(
            lock.is_file() and delivery_lock.is_file(), "alert stable lock is absent"
        )
        pending = list((parent / "alert-handoff").glob("*.json"))
        require(not pending, "alert journal is not drained")
        delivery = Path("/var/lib/verdaxis-monitor-alert") / f"{check}.delivery.json"
        if delivery.is_file():
            require(
                json.loads(delivery.read_text()).get("head") is None,
                "alert delivery is unresolved",
            )
        require(
            stat.S_IMODE(delivery_lock.stat().st_mode)
            == (0o600 if released else 0o400),
            "alert delivery ownership mode differs",
        )
        acl = s.run(
            "getfacl", "--absolute-names", "--omit-header", parent / "alert-handoff"
        )
        require(
            "user:verdaxis-monitor-alert:rwx" in acl
            and "default:user:verdaxis-monitor-alert:r--" in acl,
            "effective alert handoff ACL differs",
        )
        s.run("runuser", "-u", "verdaxis-monitor-alert", "--", "test", "-r", lock)
        s.run(
            "runuser",
            "-u",
            "verdaxis-monitor-alert",
            "--",
            "test",
            "-w",
            parent / "alert-handoff",
        )
        rows[check] = {
            "journal_lock_inode": lock.stat().st_ino,
            "delivery_lock_inode": delivery_lock.stat().st_ino,
            "delivery_lock_mode": f"{stat.S_IMODE(delivery_lock.stat().st_mode):03o}",
            "pending_count": len(pending),
            "acl_sha256": hashlib.sha256(acl.encode()).hexdigest(),
        }
    return rows


def retirement(s):
    rows = {}
    for name in ("verdaxis-recover.service", "verdaxis-codex-diagnose.service"):
        rows[name] = {
            "load": value(s, name, "LoadState"),
            "active": value(s, name, "ActiveState"),
        }
        require(
            rows[name]["load"] in ("masked", "not-found")
            and rows[name]["active"] in ("inactive", "failed"),
            "retired recovery is loadable or active",
        )
    for name in ("caddy.service", "postgresql@17-main.service"):
        target = UNIT_DIRECTORY / f"{name}.d/10-verdaxis-restart.conf"
        source = (
            s.CHECKOUT
            / "deploy/systemd/overrides"
            / f"{name}.d/10-verdaxis-restart.conf"
        )
        require(
            target.read_bytes() == source.read_bytes()
            and value(s, name, "Restart") == "on-failure",
            "reviewed restart control is not loaded",
        )
        rows[name] = {"restart": value(s, name, "Restart"), "sha256": s.digest(target)}
    rows[s.RUST_UNIT] = {
        key: value(s, s.RUST_UNIT, key)
        for key in ("WatchdogUSec", "NotifyAccess", "Restart")
    }
    require(
        rows[s.RUST_UNIT]
        == {"WatchdogUSec": "1min", "NotifyAccess": "main", "Restart": "on-failure"},
        "real Rust watchdog replacement differs",
    )
    return rows


def install_retirement(s):
    # Companion owns the saved old recovery bytes and the persistent masks.
    for name in ("caddy.service", "postgresql@17-main.service"):
        source = (
            s.CHECKOUT
            / "deploy/systemd/overrides"
            / f"{name}.d/10-verdaxis-restart.conf"
        )
        target = UNIT_DIRECTORY / f"{name}.d/10-verdaxis-restart.conf"
        target.parent.mkdir(exist_ok=True)
        shutil.copyfile(source, target)
        target.chmod(0o644)
    s.run("systemctl", "daemon-reload")
    return retirement(s)


def install_network_guard(s):
    # Preserve hosted-runner upload access. Root jobs and every runtime identity
    # can use only loopback. Leave the guard installed until the VM is discarded.
    identities = sorted(
        {0}
        | {
            entry.pw_uid
            for entry in pwd.getpwall()
            if entry.pw_name.startswith("verdaxis") or entry.pw_name == "caddy"
        }
    )
    tables = json.loads(s.run("nft", "--json", "list", "tables"))
    require(
        not any(
            row.get("table", {}).get("name") == "verdaxis_rehearsal"
            for row in tables["nftables"]
        ),
        "disposable network table already exists",
    )
    users = ", ".join(map(str, identities))
    script = (
        "table inet verdaxis_rehearsal {\n"
        " chain output { type filter hook output priority 0; policy accept;\n"
        f'  oifname != "lo" meta skuid {{ {users} }} reject\n'
        " }\n}\n"
    )
    s.run("nft", "-f", "-", data=script.encode())
    # Remove proxy routing from the system manager. A proxy could otherwise
    # escape origin checks through a numeric remote endpoint.
    proxy_names = (
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    )
    s.run("systemctl", "unset-environment", *proxy_names)
    return identities


def network_guard(s):
    resolved = {}
    for name in EXTERNAL_NAMES:
        addresses = sorted(
            {
                item[4][0]
                for item in socket.getaddrinfo(name, 443, type=socket.SOCK_STREAM)
            }
        )
        require(
            addresses
            and all(ipaddress.ip_address(address).is_loopback for address in addresses),
            "external monitor origin is not confined to loopback",
        )
        resolved[name] = addresses
    rules = s.run("nft", "--json", "list", "table", "inet", "verdaxis_rehearsal")
    ruleset = json.loads(rules)
    require(
        isinstance(ruleset.get("nftables"), list),
        "disposable output network guard is absent",
    )
    chains = [row["chain"] for row in ruleset["nftables"] if "chain" in row]
    require(
        len(chains) == 1
        and chains[0].get("hook") == "output"
        and chains[0].get("policy") == "accept",
        "disposable output network chain differs",
    )
    entries = [row["rule"] for row in ruleset["nftables"] if "rule" in row]
    require(len(entries) == 1, "disposable output network rule count differs")
    expressions = entries[0]["expr"]
    require(
        any("reject" in row for row in expressions),
        "disposable network rule does not reject",
    )
    matches = [row["match"] for row in expressions if "match" in row]
    require(
        any(
            row.get("left") == {"meta": {"key": "oifname"}}
            and row.get("op") == "!="
            and row.get("right") == "lo"
            for row in matches
        ),
        "network guard does not restrict nonloopback",
    )
    expected_uids = sorted(
        {0}
        | {
            entry.pw_uid
            for entry in pwd.getpwall()
            if entry.pw_name.startswith("verdaxis") or entry.pw_name == "caddy"
        }
    )
    uid_matches = [
        row for row in matches if row.get("left") == {"meta": {"key": "skuid"}}
    ]
    require(
        len(uid_matches) == 1
        and uid_matches[0].get("op") == "=="
        and sorted(uid_matches[0].get("right", {}).get("set", [])) == expected_uids,
        "network guard does not cover all runtime identities",
    )
    return {
        "origins": resolved,
        "runtime_uids": expected_uids,
        "ruleset_sha256": hashlib.sha256(rules.encode()).hexdigest(),
    }


def evidence(s, context, groups, runs):
    payload = {
        "schema_version": 1,
        "environment": s.ENV,
        "source_ref": s.SHA,
        "binary_sha256": context["binary_sha256"],
        "recorded_at": int(time.time()),
        "inventory_sha256": s.digest(context["inventory"]),
        "groups": groups,
        "runs": runs,
        "host_ownership": {
            "host": socket.gethostname(),
            "accepted": True,
            "inventory_sha256": context["host_inventory_sha256"],
        },
    }
    if context.get("retirement"):
        proof = json.dumps(retirement(s), sort_keys=True).encode()
        payload["recovery_retirement"] = {
            "accepted": True,
            "proof_sha256": hashlib.sha256(proof).hexdigest(),
        }
    path = Path(context["directory"]) / "evidence.json"
    s.write(path, payload)
    require(
        path.stat().st_uid == 0 and stat.S_IMODE(path.stat().st_mode) == 0o600,
        "private W2 evidence ownership differs",
    )
    return path


def action(s, context, name, *, group=None, runs=None):
    path = evidence(s, context, context["groups"], runs or {})
    command = [
        "bash",
        s.CHECKOUT / "scripts/rust_w2_handover.sh",
        "--action",
        name,
        "--environment",
        s.ENV,
        "--source-ref",
        s.SHA,
        "--binary-sha256",
        context["binary_sha256"],
        "--host-scope",
        "host",
        "--evidence",
        path,
    ]
    if group is not None:
        command.extend(["--group", group, "--timer-inventory", context["inventory"]])
    output = s.run(*command)
    s.write(
        Path(context["directory"])
        / f"{context['sequence']:03d}-{name}-{group or 'all'}.log",
        output,
    )
    context["sequence"] += 1


def prepare_alert_environment(s):
    path = Path("/etc/verdaxis-monitor/alert.env")
    require(
        not path.exists(), "disposable alert settings already exist; retain their owner"
    )
    # No destination credentials are present. Healthy observations drain without sending.
    # Failure observations must still fail rather than acquire invented receipts.
    s.write(
        path,
        "# Disposable monitor: no external notification destinations configured.\n",
    )
    s.run("chown", "verdaxis-monitor-alert:verdaxis-monitor-alert", path)
    s.run("runuser", "-u", "verdaxis-monitor-alert", "--", "test", "-r", path)


def install(s):
    """Capture old ABI, fence its real launchers, then install without activation."""
    guard_runner(s)
    directory = s.PRIVATE / "w2"
    directory.mkdir(mode=0o700)
    context = {"directory": str(directory), "sequence": 0, "groups": {}}
    launchers = s.state()["companion"]["launchers"]
    require(
        set(launchers) == set(GROUPS),
        "retained launcher inventory does not cover every group",
    )
    old_launchers = {
        name: file_record(s, name) for names in launchers.values() for name in names
    }
    s.write(directory / "old-launchers.json", old_launchers)
    s.run(
        "git",
        "-C",
        s.CHECKOUT,
        "-c",
        f"safe.directory={s.CHECKOUT}",
        "merge",
        "--ff-only",
        s.SHA,
    )
    require(
        not s.run(
            "git",
            "-C",
            s.CHECKOUT,
            "-c",
            f"safe.directory={s.CHECKOUT}",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ).strip(),
        "candidate source is not clean",
    )
    original = {}
    for group, services in unit_names(s.ENV).items():
        original[group] = {}
        for unit in services + timer_names(group, services):
            path = unit_file(unit)
            original[group][unit] = {
                "file": file_record(s, path) if path.exists() else None,
                "properties": observation(s, unit),
            }
        fences = []
        require(launchers[group], "group has no real retained launcher")
        for name in launchers[group]:
            record = file_record(s, name)
            require(
                record == old_launchers[name],
                "candidate checkout changed a retained direct launcher ABI",
            )
            require(
                record["mode"] in ("644", "640", "600", "755", "750", "700"),
                "retained launcher has unsupported permissions",
            )
            if Path(name).is_relative_to(s.CHECKOUT):
                tracked = s.run(
                    "git",
                    "-C",
                    s.CHECKOUT,
                    "-c",
                    f"safe.directory={s.CHECKOUT}",
                    "ls-files",
                    "--stage",
                    "--",
                    name,
                )
                require(
                    not tracked.startswith("100755"),
                    "executable source launcher needs its installed ABI fence",
                )
            fences.append(
                {
                    "path": record["path"],
                    "sha256": record["sha256"],
                    "original_uid": record["uid"],
                    "original_gid": record["gid"],
                    "original_mode": record["mode"],
                }
            )
        # Exercise a real previously-active calendar timer. Its installed
        # expression is unchanged; starting a timer does not start its oneshot
        # unless the real schedule is due. Drain any due admitted work first.
        if group == "news":
            timer = timer_names(group, services)[0]
            s.run("systemctl", "start", timer)
            require(
                value(s, timer, "ActiveState") == "active",
                "saved calendar timer did not become active",
            )
        require(
            all(
                value(s, timer, "ActiveState") in ("active", "inactive")
                for timer in timer_names(group, services)
            ),
            "old timer state is not observable",
        )
        drained = quiesce(s, group, services, stop_timers=False)
        for fence in fences:
            os.chown(fence["path"], 0, 0)
            os.chmod(fence["path"], 0)
        callers = pending_callers(s, [row["path"] for row in fences])
        context["groups"][group] = {
            "direct_launchers_fenced": True,
            "pending_callers_reconciled": True,
            "fences": fences,
            "drained_timers": drained,
            "callers": callers,
        }
    s.write(directory / "old-units.json", original)
    host_inventory = {
        "units": s.run("systemctl", "list-unit-files", "--no-legend", "verdaxis-*"),
        "timers": s.run("systemctl", "list-timers", "--all", "--no-legend"),
        "users": s.run("getent", "passwd"),
        "acls": s.run("getfacl", "--absolute-names", *MONITOR_DIRECTORIES),
        "original_units": original,
    }
    s.write(directory / "host-inventory.json", host_inventory)
    context["host_inventory_sha256"] = s.digest(directory / "host-inventory.json")
    binary = s.PRIVATE / "current/verified/verdaxis-read"
    context["binary_sha256"] = s.digest(binary)
    require(
        context["binary_sha256"] == s.state()["artifacts"]["current"]["binary_sha256"],
        "verified current binary identity differs",
    )
    target = s.CHECKOUT / ".runtime-rust/verdaxis-read"
    pending = target.with_name("verdaxis-read.w2-pending")
    # Rename preserves the old running process inode and the captured old bundle.
    shutil.copyfile(binary, pending)
    pending.chmod(0o755)
    with pending.open("rb") as descriptor:
        os.fsync(descriptor.fileno())
    os.replace(pending, target)
    timers = []
    for group, services in unit_names(s.ENV).items():
        if group in ("local-monitor", "external-recovery"):
            continue
        for timer in timer_names(group, services):
            timers.append(
                {
                    "name": timer,
                    "disposition": "active",
                    "sha256": s.digest(source_file(s, timer)),
                }
            )
    inventory = directory / "timers.json"
    s.write(
        inventory,
        {
            "schema_version": 1,
            "environment": s.ENV,
            "source_ref": s.SHA,
            "timers": timers,
        },
    )
    context["inventory"] = str(inventory)
    action(s, context, "install")
    prepare_alert_environment(s)
    install_network_guard(s)
    context["retirement"] = install_retirement(s)
    state_directory = Path("/var/lib/verdaxis/w2") / s.ENV
    require(
        (state_directory / "install-phase").read_text().strip() == "installed",
        "immutable W2 installation did not complete",
    )
    for group in GROUPS:
        require(
            (state_directory / group / "phase").read_text().strip() == "captured",
            "installation unexpectedly activated a group",
        )
    s.write(directory / "context.json", context)
    return context


def rollback_observations(s, context, group):
    state = Path("/var/lib/verdaxis/w2") / s.ENV / group
    rows = {}
    for path in (state / "previous").iterdir():
        target = UNIT_DIRECTORY / path.name
        require(
            target.read_bytes() == path.read_bytes(),
            "group rollback did not restore saved unit bytes",
        )
        rows[path.name] = s.digest(target)
    absent = state / "absent"
    if absent.exists():
        for name in absent.read_text().splitlines():
            require(
                not (UNIT_DIRECTORY / name).exists(),
                "previously absent unit survived rollback",
            )
    for path in (state / "dropins").glob("*/*.conf"):
        require(
            (UNIT_DIRECTORY / path.relative_to(state / "dropins")).read_bytes()
            == path.read_bytes(),
            "saved drop-in ABI was not restored",
        )
    for fence in context["groups"][group]["fences"]:
        row = file_record(s, fence["path"])
        require(
            (row["uid"], row["gid"], row["mode"], row["sha256"])
            == (
                fence["original_uid"],
                fence["original_gid"],
                fence["original_mode"],
                fence["sha256"],
            ),
            "retained launcher permissions were not restored exactly",
        )
    return {"unit_sha256": rows, "phase": (state / "phase").read_text().strip()}


def real_market_snapshot(s):
    result = {}
    selections = {
        "organizations": "SELECT * FROM organizations WHERE provenance::text <> 'DEMO'",
        "orders": "SELECT * FROM orderbook_orders WHERE provenance::text <> 'DEMO'",
        "trades": "SELECT * FROM trades WHERE buyer_provenance::text <> 'DEMO' OR seller_provenance::text <> 'DEMO'",
    }
    for name, query in selections.items():
        serialized = s.sql(
            "SELECT coalesce(json_agg(row_to_json(t) ORDER BY row_to_json(t)::text COLLATE \"C\"),'[]'::json)::text FROM ("
            + query
            + ") t;"
        )
        result[name] = hashlib.sha256(serialized.encode()).hexdigest()
    return result


def known_demo_baseline(s):
    # Economic fixtures are allowed only in the DEMO partition. No real-market
    # row may be introduced to make a passive observation pass.
    count = int(
        s.sql(
            "SELECT (SELECT count(*) FROM orderbook_orders WHERE provenance::text <> 'DEMO') + (SELECT count(*) FROM trades WHERE buyer_provenance::text <> 'DEMO' OR seller_provenance::text <> 'DEMO');"
        ).strip()
    )
    require(count == 0, "unexpected non-demo economic fixture")
    return True


def runtime_probes(s, group, phase):
    from probes import probe

    path = s.PUBLIC / f"w2-{group}-{phase}-probes.json"
    known_demo_baseline(s)
    result = probe(s.CHECKOUT, s.ENV, s.SHA, s.PRIVATE, path, require_empty=False)
    require(
        all(result.get(name) == "PASS" for name in ("auth", "session", "passive_harm")),
        "group runtime acceptance probes did not pass",
    )
    return {"path": path.name, "sha256": s.digest(path), "checks": result}


def activate(s, context):
    """Run, accept, roll back and reacquire each group after real M readiness."""
    guard_runner(s)
    s.process_identity(s.SHA, context["binary_sha256"])
    confinement = network_guard(s)
    all_runs, rows, rollbacks = {}, {}, {}
    state = Path("/var/lib/verdaxis/w2") / s.ENV
    for group, services in unit_names(s.ENV).items():
        before = table_snapshot(s)
        real_before = real_market_snapshot(s)
        previous = {unit: value(s, unit, "InvocationID") for unit in services}
        if group == "demo":
            require(
                all(
                    (state / earlier / "phase").read_text().strip() == "accepted"
                    for earlier in GROUPS[:4]
                ),
                "demo preceded market-group acceptance",
            )
            context["groups"][group]["market_groups_accepted"] = True
        action(s, context, "advance", group=group)
        observed = {unit: successful_run(s, unit, previous[unit]) for unit in services}
        all_runs.update({unit: row["InvocationID"] for unit, row in observed.items()})
        after = table_snapshot(s)
        require(
            real_market_snapshot(s) == real_before,
            "group changed non-demo economic rows",
        )
        alert_before = journals(s) if group == "local-monitor" else None
        context["groups"][group]["no_duplicate_effect"] = True
        action(s, context, "accept", group=group, runs=all_runs)
        saved_active = (state / group / "active-timers").read_text().splitlines()
        require(
            all(value(s, timer, "ActiveState") == "active" for timer in saved_active),
            "accepted group did not restore its observed active timers",
        )
        if group == "news":
            require(
                saved_active == timer_names(group, services),
                "previously-active calendar timer was not captured",
            )
        accepted_probes = runtime_probes(s, group, "accepted")
        accepted_snapshot = table_snapshot(s)
        action(s, context, "accept", group=group, runs=all_runs)
        require(
            table_snapshot(s) == accepted_snapshot,
            "repeat acceptance changed persisted effects",
        )
        require(
            all(
                value(s, unit, "InvocationID") == row["InvocationID"]
                for unit, row in observed.items()
            ),
            "repeat acceptance reran a native command",
        )
        timer_drain = quiesce(s, group, services)
        context["groups"][group]["rollback"] = {
            "native_launchers_disabled": True,
            "direct_schedulers_disabled": True,
            "permission_restore_accepted": True,
        }
        if group in ("local-monitor", "external-recovery"):
            journals(s)
        rollback_before = table_snapshot(s)
        action(s, context, "rollback", group=group, runs=all_runs)
        restored = rollback_observations(s, context, group)
        rollback_after = table_snapshot(s)
        require(
            restored["phase"] == "rolled-back" and rollback_after == rollback_before,
            "group rollback rewound shared database effects",
        )
        if group in ("local-monitor", "external-recovery"):
            released = journals(s, released=True)
            if alert_before:
                require(
                    all(
                        released[check]["delivery_lock_inode"]
                        == alert_before[check]["delivery_lock_inode"]
                        for check in released
                    ),
                    "alert ownership release replaced a stable lock",
                )
            restored["alert_release"] = released
        rollback_probes = runtime_probes(s, group, "rolled-back")
        quiesce(s, group, services)
        rollback_invocations = {
            unit: value(s, unit, "InvocationID") for unit in services
        }
        action(s, context, "advance", group=group, runs=all_runs)
        rerun = {
            unit: successful_run(s, unit, rollback_invocations[unit])
            for unit in services
        }
        alert_reacquire = {}
        if group == "external-recovery":
            # External rollback releases both journals. Reacquire through the
            # real dispatcher units, never by changing the stable lock mode here.
            for check in ("health", "backup"):
                unit = f"verdaxis-monitor-alert@{check}.service"
                prior = value(s, unit, "InvocationID")
                s.run("systemctl", "start", unit)
                alert_reacquire[unit] = successful_run(s, unit, prior)
            journals(s)
        all_runs.update({unit: row["InvocationID"] for unit, row in rerun.items()})
        all_runs.update(
            {unit: row["InvocationID"] for unit, row in alert_reacquire.items()}
        )
        repeat_snapshot = table_snapshot(s)
        action(s, context, "accept", group=group, runs=all_runs)
        require(
            (state / group / "phase").read_text().strip() == "accepted",
            "group reacquisition was not accepted",
        )
        rows[group] = {
            "services": services,
            "applicability": "installed"
            if services
            else "no environment-specific service",
            "before": before,
            "after_first_run": after,
            "after_reacquire": repeat_snapshot,
            "first_runs": observed,
            "reacquired_runs": rerun,
            "alert_reacquire": alert_reacquire,
            "accepted_probes": accepted_probes,
            "rollback_probes": rollback_probes,
            "reacquired_probes": runtime_probes(s, group, "reacquired"),
            "journal": journals(s)
            if group in ("local-monitor", "external-recovery")
            else None,
        }
        rollbacks[group] = {
            "drained_timers": timer_drain,
            "restored": restored,
            "database_before": rollback_before,
            "database_after": rollback_after,
        }
    summary = {
        "scope": "disposable-candidate",
        "environment": s.ENV,
        "source_ref": s.SHA,
        "binary_sha256": context["binary_sha256"],
        "group_order": list(GROUPS),
        "groups": rows,
        "rollbacks": rollbacks,
        "recovery_retirement": retirement(s),
        "network": confinement,
        "timer_inventory": json.loads(Path(context["inventory"]).read_text()),
        "timer_inventory_sha256": s.digest(context["inventory"]),
    }
    s.write(s.PUBLIC / "groups.json", summary)
    s.write(
        s.PUBLIC / "units.json",
        {
            "scope": "disposable-candidate",
            "environment": s.ENV,
            "source_ref": s.SHA,
            "binary_sha256": context["binary_sha256"],
            "runs": all_runs,
            "units": {unit: observation(s, unit) for unit in all_runs},
        },
    )
    return summary


def ensure_native(s):
    """Reinstall reviewed native units after a protected old-bundle restore."""
    guard_runner(s)
    expected = s.state()["artifacts"]["current"]["binary_sha256"]
    require(
        s.digest(s.CHECKOUT / ".runtime-rust/verdaxis-read") == expected,
        "forward reinstall requires the verified current executable",
    )
    command = [
        "bash",
        s.CHECKOUT / "scripts/install_systemd_units.sh",
        "--environment",
        s.ENV,
        "--source-ref",
        s.SHA,
        "--binary-sha256",
        expected,
        "--host-scope",
        "host",
    ]
    s.run(*command, "--dry-run")
    s.run(*command, "--apply")
    install_retirement(s)
    require(
        s.digest("/usr/local/libexec/verdaxis/verdaxis-read") == expected,
        "forward host executable differs",
    )
    return {"binary_sha256": expected, "recovery_retirement": retirement(s)}


def run_window(s):
    """Install W2, let the driver adopt M, then exercise ordered activation."""
    context = install(s)
    deployment = s.deploy_current(expected_current="ob_20261031_offboarding")
    result = activate(s, context)
    result["deployment"] = deployment
    return result
