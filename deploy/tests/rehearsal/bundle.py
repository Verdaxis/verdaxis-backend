#!/usr/bin/env python3
"""Protected old-release inventory from actual installed files and dependencies."""

import re
import subprocess
from pathlib import Path


def build(s):
    checkout, private = s.CHECKOUT, s.PRIVATE
    files = {}

    def add(kind, path):
        path = Path(path)
        if not path.is_file() or path.is_symlink():
            raise RuntimeError(f"required closure input is not regular: {path}")
        if str(path) in files and files[str(path)] != kind:
            # A native source guard is captured once as its explicit kind.
            return
        files[str(path)] = kind

    canonical = {
        "binary": ".runtime-rust/verdaxis-read",
        "runtime": ".runtime-rust/runtime.env",
        "release": ".runtime-release.env",
        "revision": ".runtime-rust/revision.json",
        "budget": ".runtime-rust/budget.json",
        "policy": f".runtime-rust/{s.ENV}-policy.json",
        "monitor_identity": ".runtime-monitor.env",
        "guard": "scripts/verify_deploy_start_guard.py",
    }
    for kind, tail in canonical.items():
        add(kind, checkout / tail)
    for kind, path in (
        ("artifact", private / "previous/archive.tar"),
        ("provenance", private / "previous/provenance.json"),
        ("signature", private / "previous/provenance.json.sig"),
        ("provenance_key", private / "key.pub.pem"),
        ("caddy", Path("/etc/caddy/Caddyfile")),
        ("credential", Path(f"/etc/verdaxis/kyc-document-keys{s.UNIT_SUFFIX}.json")),
    ):
        add(kind, path)
    # Old .env migration authority is escrowed, not restored. Runtime contains
    # only the app credential, and the capture command checks this distinction.
    add("migrator_credential", checkout / ".env")
    companion = s.state()["companion"]
    monitor_binary = Path(companion["monitor_binary"])
    add("monitor_binary", monitor_binary)
    package_roots = [checkout / "venv"] + [Path(p) for p in companion["package_roots"]]
    for path in (
        Path("/etc/sysusers.d/verdaxis-monitor.conf"),
        Path("/etc/tmpfiles.d/verdaxis-monitor.conf"),
    ):
        add("runtime_package", path)
    # Preserve the installed monitor timer ABI as well as service fragments.
    # Masks are dispositions, not regular payload files.
    for path in Path("/etc/systemd/system").glob("verdaxis-*"):
        if (
            path.is_file()
            and not path.is_symlink()
            and path.suffix in (".service", ".timer")
        ):
            add("unit", path)
    consumers = [
        s.RUST_UNIT,
        f"verdaxis-backend{s.UNIT_SUFFIX}.service",
        "verdaxis-health.service",
        "verdaxis-backup-verify.service",
        f"verdaxis-news-refresh{s.UNIT_SUFFIX}.service",
        f"verdaxis-auth-maintenance{s.UNIT_SUFFIX}.service",
        f"verdaxis-product-analytics-prune{s.UNIT_SUFFIX}.service",
    ]
    durable_outputs = {
        "/var/lib/verdaxis-health/status.json",
        "/var/lib/verdaxis-backup-verify/status.json",
        "/home/verdaxis-prod/backups",
        "/home/verdaxis-prod/backups/status.json",
        "/home/verdaxis-prod/backups/attempts",
    }
    native, aliases, consumer_rows = {}, {}, []

    def native_input(path):
        path = Path(path)
        resolved = path.resolve(strict=True)
        if not resolved.is_file():
            raise RuntimeError("native dependency is not regular")
        if path != resolved:
            aliases[str(path)] = str(resolved)
        native[str(resolved)] = s.digest(resolved)
        # Dynamic loaders report concrete shared objects. Keep each real file,
        # including the actual interpreter binary, in the signed closure.
        output = s.run("ldd", resolved, check=False)
        for match in re.findall(r"(?:=>\s*)?(/[^\s()]+)", output):
            dependency = Path(match)
            if dependency.exists():
                target = dependency.resolve(strict=True)
                native[str(target)] = s.digest(target)
                if dependency != target:
                    aliases[str(dependency)] = str(target)

    for unit in consumers:
        arguments = ["systemctl", "show", unit]
        for prop in s.PROPERTIES:
            arguments.extend(("-p", prop))
        properties = s.run(*arguments)
        path = private / "consumer-properties" / unit
        s.write(path, properties)
        consumer_rows.append({"name": unit, "properties_sha256": s.digest(path)})
        if "FragmentPath=/" not in properties:
            raise RuntimeError("required installed consumer missing")
        for token in sorted(set(re.findall(r"/[A-Za-z0-9_./@:+-]+", properties))):
            target = Path(token)
            if target == checkout:
                continue
            if str(target).startswith(str(checkout) + "/"):
                tracked = (
                    subprocess.run(
                        [
                            "git",
                            "-c",
                            f"safe.directory={checkout}",
                            "-C",
                            str(checkout),
                            "ls-files",
                            "--error-unmatch",
                            str(target.relative_to(checkout)),
                        ],
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        check=False,
                    ).returncode
                    == 0
                )
                if tracked:
                    continue
            if token in durable_outputs:
                continue
            if token.startswith(("/usr/bin/", "/bin/", "/usr/lib/")):
                native_input(target)
            else:
                kind = (
                    "unit"
                    if token.endswith(".service")
                    else "dropin"
                    if token.endswith(".conf")
                    else "runtime_package"
                )
                add(kind, target)
    for root in package_roots:
        for path in sorted(root.rglob("*")):
            if path.is_symlink():
                raise RuntimeError("retained package closure contains links")
            if path.is_file():
                add("runtime_package", path)
    # The stdlib used by the exact old isolated prestarts is also an actual
    # dependency. It is immutable system input, not rewritten by restore.
    stdlib = Path(
        s.run(
            "/usr/bin/python3",
            "-I",
            "-S",
            "-c",
            "import sysconfig; print(sysconfig.get_path('stdlib'))",
        ).strip()
    )
    for path in stdlib.rglob("*"):
        if (
            path.is_file()
            and not path.is_symlink()
            and "site-packages" not in path.parts
            and "dist-packages" not in path.parts
        ):
            native[str(path)] = s.digest(path)
    native_input("/usr/bin/python3")
    if len(files) > 131072 or sum(Path(p).stat().st_size for p in files) > 8 * 1024**3:
        raise RuntimeError("retained package exceeds bounded capture size")
    reader = "verdaxis_bundle_reader" + s.SUFFIX
    password = s.state()["password"]
    s.sql(
        f"CREATE ROLE {reader} LOGIN NOINHERIT PASSWORD '{password}'; GRANT CONNECT ON DATABASE {s.DB} TO {reader}; GRANT USAGE ON SCHEMA public TO {reader}; GRANT SELECT ON ALL TABLES IN SCHEMA public TO {reader};"
    )
    s.write(
        private / "reader.service",
        f"[pre_m_reader]\nhost=127.0.0.1\nport=5432\ndbname={s.DB}\nuser={reader}\npassword={password}\n",
    )
    rows = [{"kind": kind, "path": path} for path, kind in sorted(files.items())]
    inventory = {
        "schema": 1,
        "environment": s.ENV,
        "release_sha": s.OLD,
        "checkout": str(checkout),
        "deploy_uid": Path(checkout).stat().st_uid,
        "backend_unit": s.RUST_UNIT,
        "ready_url": f"https://{s.DOMAIN}/health/ready",
        "final_alembic": "ob_20261031_offboarding",
        "acceptance_key_sha256": s.digest(private / "key.pub.pem"),
        "workflow_ref": "refs/heads/staging",
        "checkout_branch": "staging",
        "database_name": s.DB,
        "reader_role": reader,
        "runtime_role": s.APP,
        "consumer_units": consumer_rows,
        "durable_outputs": sorted(durable_outputs),
        "runtime_package_roots": [str(p) for p in package_roots],
        "native_dependency_files": [
            {"path": p, "sha256": d} for p, d in sorted(native.items())
        ],
        "native_dependency_aliases": [
            {"path": p, "resolved": r} for p, r in sorted(aliases.items())
        ],
        "timers": companion["selected_timers"],
        "jobs": [
            u
            for u in consumers
            if u not in (s.RUST_UNIT, f"verdaxis-backend{s.UNIT_SUFFIX}.service")
        ],
        "files": rows,
    }
    closure = {
        key: inventory[key]
        for key in (
            "consumer_units",
            "native_dependency_files",
            "runtime_package_roots",
            "files",
            "native_dependency_aliases",
        )
    }
    # Match jq -cS serialization, including its final newline, exactly.
    closure_path = private / "closure-inputs.json"
    s.write(private / "closure-unordered.json", closure)
    canonical = s.run("jq", "-cS", ".", private / "closure-unordered.json")
    s.write(closure_path, canonical)
    inventory["dependency_closure_sha256"] = s.digest(closure_path)
    s.write(private / "bundle-inventory.json", inventory)
    s.write(
        private / "closure.json",
        {
            "environment": s.ENV,
            "release_sha": s.OLD,
            "dependency_closure_sha256": inventory["dependency_closure_sha256"],
            "runtime_dependencies": "PASS",
            "credential_sources": "PASS",
            "unit_inputs": "PASS",
        },
    )
    s.sign(private / "closure.json")
    return inventory
