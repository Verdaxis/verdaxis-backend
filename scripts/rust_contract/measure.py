"""Measure the ported-route workload: memory, CPU, and end-to-end latency.

One disposable environment, one seeded workload, two runtimes, identical
traffic. The service processes run inside per-service systemd user units so the
cgroup counters (`memory.current`, `memory.peak`, `memory.stat`, `cpu.stat`)
belong to exactly one runtime and never mix with the load generator or the
database.

Every decision that shapes a reported number is a pure function and is unit
tested without a host: percentile selection, coverage conclusions, cgroup
counter parsing, and the core-equivalent and ratio arithmetic. The run loop
itself is exercised only on the test host, under the operator's wrapper.

The measured load stays at the workload manifest's 1, 5 and 10 requests per
second. A route that cannot reach the manifest's observation floor inside the
run is reported inconclusive with the number still needed -- never a pass.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]

from scripts.rust_contract import run_disposable as runner  # noqa: E402
from scripts.rust_contract import workload  # noqa: E402


# ---------------------------------------------------------------- pure helpers


def percentile(sorted_values: list[float], q: float) -> float:
    """The nearest-rank percentile of an ascending list.

    `q` is a fraction in (0, 1]. The index is `ceil(q * n) - 1`, the same rule
    `measure_lists.py` uses, so the p95 here is the earlier p95's exact
    definition. An empty list has no percentile and is refused.
    """
    if not sorted_values:
        raise ValueError("no observations")
    if not 0.0 < q <= 1.0:
        raise ValueError("the quantile must be in (0, 1]")
    return sorted_values[math.ceil(q * len(sorted_values)) - 1]


def summarize(samples: list[float]) -> dict[str, float]:
    """n, p50, p95, p99 and the spread for one cell of observations."""
    if not samples:
        raise ValueError("no observations to summarize")
    ordered = sorted(samples)
    return {
        "n": len(ordered),
        "p50": percentile(ordered, 0.50),
        "p95": percentile(ordered, 0.95),
        "p99": percentile(ordered, 0.99),
        "min": ordered[0],
        "max": ordered[-1],
    }


def weighted_mix(
    per_route_p50: dict[str, float],
    per_route_p95: dict[str, float],
    per_route_p99: dict[str, float],
    weights: dict[str, int],
) -> dict[str, float]:
    """The declared representative-mix aggregate latency.

    The mix is the weight-normalized mean of the per-route quantiles over the
    routes that carry a representative weight. It is a declared aggregation of
    measured per-route numbers, never a separate unmeasured run.
    """
    total = sum(weights.values())
    if total <= 0:
        raise ValueError("the representative weights must be positive")
    return {
        quantile: sum(weights[rid] * values[rid] for rid in weights) / total
        for quantile, values in (
            ("p50", per_route_p50),
            ("p95", per_route_p95),
            ("p99", per_route_p99),
        )
    }


def coverage(counts: dict[str, int], required: int) -> dict[str, Any]:
    """Per route: the observed samples and how many more are still needed."""
    routes = {}
    inconclusive = []
    for route_id, observed in sorted(counts.items()):
        if observed >= required:
            routes[route_id] = {"observations": observed, "outcome": "ok"}
        else:
            routes[route_id] = {
                "observations": observed,
                "needed": required - observed,
                "outcome": "inconclusive",
            }
            inconclusive.append(route_id)
    return {"routes": routes, "inconclusive": inconclusive}


def parse_memory_stat(text: str) -> dict[str, int]:
    """The anonymous and file bytes from a cgroup v2 `memory.stat`."""
    fields: dict[str, int] = {}
    for line in text.splitlines():
        name, _, value = line.partition(" ")
        if name in {"anon", "file"}:
            fields[name] = int(value)
    if set(fields) != {"anon", "file"}:
        raise ValueError("memory.stat is missing anon or file")
    return fields


def parse_cpu_usage_usec(text: str) -> int:
    """The cumulative `usage_usec` from a cgroup v2 `cpu.stat`."""
    for line in text.splitlines():
        name, _, value = line.partition(" ")
        if name == "usage_usec":
            return int(value)
    raise ValueError("cpu.stat is missing usage_usec")


def cpu_core_equivalents(delta_usage_usec: int, delta_elapsed_usec: int) -> float:
    """CPU cost as core-equivalents over one interval."""
    if delta_elapsed_usec <= 0:
        raise ValueError("the elapsed interval must be positive")
    return delta_usage_usec / delta_elapsed_usec


def memory_ratio(python_bytes: int, rust_bytes: int) -> float:
    """Rust memory as a fraction of Python's. A zero baseline is refused."""
    if python_bytes <= 0:
        raise ValueError("the Python baseline must be positive")
    return rust_bytes / python_bytes


def require_no_regression(summary: list[dict[str, Any]]) -> list[str]:
    """Name every route whose Rust p95 exceeds its Python p95.

    An empty list means no measured regression. A missing counterpart is
    reported, not ignored.
    """
    indexed = {(row["route_id"], row["target"]): row for row in summary}
    problems = []
    for (route_id, target) in sorted(indexed):
        if target != "rust":
            continue
        python = indexed.get((route_id, "python"))
        rust = indexed[(route_id, "rust")]
        if python is None:
            problems.append(f"{route_id}: no Python counterpart")
        elif rust["p95"] > python["p95"]:
            problems.append(
                f"{route_id}: Rust p95 {rust['p95']:.4f}s > Python {python['p95']:.4f}s"
            )
    return problems


# ------------------------------------------------------------- run-only pieces


@dataclass
class ServiceHandle:
    """One runtime's processes and the cgroup that accounts for them."""

    name: str
    unit: str = ""
    cgroup: str = ""
    child: subprocess.Popen | None = None
    pids: list[int] = None  # type: ignore[assignment]

    def __post_init__(self) -> None:
        if self.pids is None:
            self.pids = []


def _cgroup_root() -> str:
    own = Path("/proc/self/cgroup").read_text().split("::", 1)[-1].strip()
    return f"/sys/fs/cgroup{own}"


class Measurement(runner.Runner):
    """Run both runtimes on one seeded workload and collect the evidence."""

    def __init__(
        self,
        size: str,
        *,
        output_dir: Path,
        warmup_seconds: int,
        windows: list[tuple[float, int]],
        repetitions: int,
        keep: bool = False,
    ) -> None:
        super().__init__(
            "all", seed=True, keep=keep, seed_mode="corpus", mixed_caddy=True, release=True
        )
        self.size = size
        self.output_dir = output_dir
        self.warmup_seconds = warmup_seconds
        self.windows = windows
        self.repetitions = repetitions
        self.manifest = workload.load()
        self.counts = workload.sizes(self.manifest)[size]
        self.schedule = workload.weighted_schedule(self.manifest, "weight_baseline")
        self.cases = self._case_table()
        self.services: list[ServiceHandle] = []

    # -- workload plumbing

    def _case_table(self) -> dict[str, dict[str, Any]]:
        table = {}
        for route in workload.routes(self.manifest):
            table[route["id"]] = {
                "path": route["path"],
                "query": route["measure_query"],
                "method": "GET",
            }
        for tool in workload.tools(self.manifest):
            table[tool["id"]] = {
                "path": "/mcp",
                "query": "",
                "method": "POST",
                "body": json.dumps(tool["body"]).encode(),
                "content_type": "application/json",
            }
        return table

    def _seed_workload(self, env: dict[str, str]) -> None:
        subprocess.run(
            [self.python_bin, "-m", "scripts.rust_contract.workload_seed", "--size", self.size],
            cwd=ROOT,
            env={**env, "PYTHONPATH": str(ROOT)},
            check=True,
        )

    # -- service control

    def start_service(
        self,
        name: str,
        argv: list[str],
        env: dict[str, str],
        *,
        systemd: bool = True,
    ) -> ServiceHandle:
        """Start one runtime's processes and return its accounting handle."""
        handle = ServiceHandle(name=name)
        if systemd and shutil.which("systemd-run"):
            handle.unit = f"t14a-{self.size}-{name}-{os.getpid()}-{len(self.services)}"
            command = [
                "systemd-run",
                "--user",
                "--unit",
                handle.unit,
                "--collect",
                "--quiet",
                "-p",
                f"WorkingDirectory={ROOT}",
            ]
            for key, value in env.items():
                command += ["-E", f"{key}={value}"]
            command += argv
            subprocess.run(command, check=True)
            show = subprocess.run(
                ["systemctl", "--user", "show", handle.unit, "-p", "ControlGroup", "--value"],
                capture_output=True,
                text=True,
                check=True,
            )
            # ControlGroup is an absolute path under /sys/fs/cgroup.
            handle.cgroup = "/sys/fs/cgroup" + show.stdout.strip()
        else:
            handle.child = subprocess.Popen(
                argv, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
            )
        self.services.append(handle)
        return handle

    def stop_service(self, handle: ServiceHandle) -> None:
        if handle.unit:
            subprocess.run(
                ["systemctl", "--user", "stop", handle.unit],
                check=False,
                capture_output=True,
            )
        elif handle.child is not None:
            handle.child.terminate()
            try:
                handle.child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                handle.child.kill()
                handle.child.wait()
        if handle in self.services:
            self.services.remove(handle)

    def _unit_pids(self, handle: ServiceHandle) -> list[int]:
        if not handle.cgroup:
            return [handle.child.pid] if handle.child else []
        try:
            text = Path(handle.cgroup, "cgroup.procs").read_text()
        except OSError:
            return []
        return [int(line) for line in text.split() if line]

    def read_counters(self, handle: ServiceHandle) -> dict[str, Any]:
        """One second's memory and CPU counters for a running service unit."""
        reading: dict[str, Any] = {"t": time.monotonic(), "unit": handle.unit}
        if handle.cgroup:
            try:
                reading["memory_current"] = int(
                    Path(handle.cgroup, "memory.current").read_text().strip()
                )
                reading["memory_peak"] = int(
                    Path(handle.cgroup, "memory.peak").read_text().strip()
                )
                reading.update(
                    parse_memory_stat(Path(handle.cgroup, "memory.stat").read_text())
                )
                reading["cpu_usage_usec"] = parse_cpu_usage_usec(
                    Path(handle.cgroup, "cpu.stat").read_text()
                )
            except (OSError, ValueError) as error:
                reading["error"] = str(error)
        pss = 0
        rss = 0
        count = 0
        for pid in self._unit_pids(handle):
            try:
                text = Path(f"/proc/{pid}/smaps_rollup").read_text()
            except OSError:
                continue
            for line in text.splitlines():
                if line.startswith("Pss:"):
                    pss += int(line.split()[1]) * 1024
                elif line.startswith("Rss:"):
                    rss += int(line.split()[1]) * 1024
            count += 1
        reading["pss"] = pss
        reading["rss"] = rss
        reading["processes"] = count
        return reading

    # -- traffic

    def _request(self, port: int, case: dict[str, Any]) -> tuple[float, int]:
        """One end-to-end request through Caddy; returns (seconds, status)."""
        url = f"http://127.0.0.1:{port}{case['path']}"
        if case["query"]:
            url += "?" + case["query"]
        headers = {"x-request-id": "t14a-measure", "host": "127.0.0.1"}
        data = None
        if case["method"] == "POST":
            data = case["body"]
            headers["content-type"] = case["content_type"]
        request = urllib.request.Request(url, data=data, headers=headers, method=case["method"])
        started = time.perf_counter()
        try:
            with urllib.request.urlopen(request, timeout=65) as response:
                response.read()
                status = response.status
        except urllib.error.HTTPError as error:
            status = error.code
        except (urllib.error.URLError, OSError, TimeoutError):
            status = 0
        return time.perf_counter() - started, status

    def load_series(self, port: int, seconds: int, rps: float) -> list[dict[str, Any]]:
        """A fixed-rate offered-load window; queuing is included in the latency."""
        total = max(1, int(seconds * rps))
        interval = 1.0 / rps
        records: list[dict[str, Any]] = []
        lock = threading.Lock()
        concurrency = max(2, int(rps * 2))

        def one(index: int) -> None:
            kind, identifier = self.schedule[index % len(self.schedule)]
            elapsed, status = self._request(port, self.cases[identifier])
            with lock:
                records.append(
                    {"route_id": identifier, "kind": kind, "seconds": elapsed, "status": status}
                )

        with ThreadPoolExecutor(max_workers=concurrency) as pool:
            started = time.monotonic()
            for index in range(total):
                target = started + index * interval
                now = time.monotonic()
                if target > now:
                    time.sleep(target - now)
                pool.submit(one, index)
        return records

    # -- orchestration

    def _scrape_loop(
        self, handle: ServiceHandle, stop: threading.Event, out: list[dict[str, Any]]
    ) -> None:
        while not stop.is_set():
            out.append(self.read_counters(handle))
            stop.wait(1.0)

    def _start_caddy(self, upstream: str) -> tuple[subprocess.Popen, int]:
        caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
        if not caddy:
            caddy = str(ROOT / "work/rust-phase1/bin/caddy")
        port = runner.ephemeral_port()
        env = {
            **os.environ,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "CADDY_PORT": str(port),
            "PYTHON_UPSTREAM": upstream,
            **{f"{group}_UPSTREAM": upstream for group in ("SIMPLE", "ORDERBOOK", "PRICES", "CURVES", "MCP")},
        }
        config = str(ROOT / "tests/rust_contract/Caddyfile")
        subprocess.run([caddy, "adapt", "--config", config, "--validate"], cwd=ROOT, env=env, check=True)
        child = subprocess.Popen(
            [caddy, "run", "--config", config], cwd=ROOT, env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        runner.wait_for_port(port)
        return child, port

    def run(self) -> None:
        """Build, seed once, then run both runtimes in alternating order."""
        subprocess.run(self.build_command(), cwd=ROOT, env={**os.environ, "RELEASE_SHA": self.sha}, check=True)
        subprocess.run(
            ["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "--release", "-p", "verdaxis-read"],
            cwd=ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha},
            check=True,
        )
        port = self.start_database()
        base_env = {
            **os.environ,
            "ENVIRONMENT": "test",
            "RELEASE_SHA": self.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DISPOSABLE_TEST_TOKEN": self.token,
            "RUST_CONTRACT_CLOCK": runner.CORPUS_CLOCK,
            "KYC_EXECUTION_GATE_ENABLED": "true",
            "RUST_CORPUS_MANIFEST": str(ROOT / "work/rust-phase1/task4b-fixtures.json"),
            **self.urls(port),
        }
        self.migrate_and_seed(base_env)
        self._seed_workload(base_env)
        runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres", "-d", runner.DB_NAME,
            "-v", "ON_ERROR_STOP=1", "-c", "ANALYZE",
        )

        python_port = runner.ephemeral_port()
        rust_port = runner.ephemeral_port()
        rust_config = json.dumps(
            {
                "environment": "test",
                "kyc_execution_gate_enabled": True,
                "release_sha": self.sha,
                "migration_revision": runner.MIGRATION_REVISION,
                "database_url": base_env["RUST_DATABASE_URL"],
                "db_session_time_zone": runner.ORACLE_TIMEZONE,
                "bind": f"127.0.0.1:{rust_port}",
                "pool_max": 9,
                "python_budget": 52,
                "configured_max_connections": 100,
            }
        )

        evidence: dict[str, Any] = {
            "size": self.size,
            "counts": self.counts,
            "source_sha": self.sha,
            "python_workers": 4,
            "rust_processes": 1,
            "rates_rps": [rate for _, rate in self.windows],
            "warmup_seconds": self.warmup_seconds,
            "repetitions": [],
            "systemd_units": bool(shutil.which("systemd-run")),
        }
        order = [("python", python_port), ("rust", rust_port)]
        for repetition in range(self.repetitions):
            # Alternate which runtime goes first, so a warm cache never favours
            # one side across every repetition.
            sequence = order if repetition % 2 == 0 else order[::-1]
            rep: dict[str, Any] = {"index": repetition, "order": [n for n, _ in sequence], "cells": {}}
            for target, port in sequence:
                handle = self._start_service_unit(target, python_port, rust_port, rust_config, base_env)
                try:
                    rep["cells"][target] = self._measure_cell(handle, port)
                finally:
                    self.stop_service(handle)
            evidence["repetitions"].append(rep)
        out = self.output_dir / f"measure-{self.size}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(evidence, indent=2) + "\n")
        print(f"MEASUREMENT_JSON {out}")

    def _start_service_unit(
        self, target: str, python_port: int, rust_port: int, rust_config: str, base_env: dict[str, str]
    ) -> ServiceHandle:
        if target == "python":
            return self.start_service(
                "python",
                [
                    str(ROOT / "venv/bin/python"), "-m", "uvicorn",
                    "tests.rust_contract.server:factory", "--factory",
                    "--workers", "4", "--host", "127.0.0.1", "--port", str(python_port),
                    "--log-level", "warning",
                ],
                {**base_env, "PYTHONPATH": str(ROOT), "UVICORN_BIND_PORT": str(python_port)},
            )
        return self.start_service(
            "rust",
            [str(ROOT / "rs/target/release/verdaxis-read"), "--bind", f"127.0.0.1:{rust_port}"],
            {**base_env, "RUST_CONFIG": rust_config},
        )

    def _measure_cell(self, handle: ServiceHandle, port: int) -> dict[str, Any]:
        """Warm up, then offer the declared rates and record every sample."""
        ready_port = port
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{ready_port}/health/live", timeout=2).read()
                break
            except (urllib.error.URLError, OSError):
                time.sleep(0.5)
        else:
            raise runner.RunnerError(f"{handle.name} never became ready")

        self.load_series(ready_port, self.warmup_seconds, 5.0)
        samples: list[dict[str, Any]] = []
        counters: list[dict[str, Any]] = []
        stop = threading.Event()
        scraper = threading.Thread(target=self._scrape_loop, args=(handle, stop, counters), daemon=True)
        scraper.start()
        caddy, caddy_port = self._start_caddy(f"127.0.0.1:{port}")
        try:
            for seconds, rate in self.windows:
                samples += self.load_series(caddy_port, seconds, rate)
        finally:
            stop.set()
            scraper.join(timeout=5)
            caddy.terminate()
            caddy.wait(timeout=10)
        return {"samples": samples, "counters": counters}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    import tomllib

    document = tomllib.loads(args.config.read_text())
    measurement = Measurement(
        str(document["size"]),
        output_dir=args.output,
        warmup_seconds=int(document.get("warmup_seconds", 30)),
        windows=[(float(w["seconds"]), float(w["rps"])) for w in document["windows"]],
        repetitions=int(document.get("repetitions", 2)),
    )
    try:
        measurement.run()
    finally:
        measurement.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
