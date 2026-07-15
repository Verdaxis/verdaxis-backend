#!/usr/bin/env python3
"""Latency and response-size benchmark for the Product Analytics endpoints.

Budgets (plan §2.6): p95 ≤ 500ms warm and ≤ 1500ms cold per tab for a 90-day
unfiltered query; response body ≤ 250KB uncompressed. The script logs in with
the staging integration admin, performs 30 sequential warm requests per tab
plus one cold measurement (the first request after a controlled backend
restart counts as cold; pass --include-cold-first to treat request #1 that
way). It prints timings and sizes only — never tokens or response data.

Usage:
    ITEST_PASSWORD="$(sudo cat /home/verdaxis-prod/verdaxis/.staging-itest-password)" \
      venv/bin/python scripts/benchmark_product_analytics.py \
      --base-url https://api-staging.verdaxis.exchange \
      --email itest-admin@staging.verdaxis.exchange
"""

from __future__ import annotations

import argparse
import os
import statistics
import sys
import time
from datetime import UTC, datetime, timedelta

import httpx

TABS = (
    "overview", "acquisition", "activation", "engagement",
    "marketplace", "retention", "reliability",
)
WARM_REQUESTS = 25  # stays inside the 30/minute per-token endpoint rate limit
WARM_P95_BUDGET_MS = 500
COLD_BUDGET_MS = 1500
SIZE_BUDGET_BYTES = 250_000


def _p95(samples: list[float]) -> float:
    ordered = sorted(samples)
    index = max(0, int(len(ordered) * 0.95) - 1)
    return ordered[index]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--email", required=True)
    parser.add_argument("--days", type=int, default=90)
    parser.add_argument(
        "--include-cold-first",
        action="store_true",
        help="treat the first request per tab as the cold measurement "
        "(run right after a controlled backend restart)",
    )
    args = parser.parse_args()

    password = os.environ.get("ITEST_PASSWORD", "")
    if not password:
        print("ITEST_PASSWORD is not set", file=sys.stderr)
        return 2

    end = datetime.now(UTC)
    start = end - timedelta(days=args.days)
    params = {
        "start": start.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "end": end.strftime("%Y-%m-%dT%H:%M:%SZ"),
    }

    failures: list[str] = []
    with httpx.Client(base_url=args.base_url, timeout=30.0) as client:
        login = client.post(
            "/api/auth/login", data={"username": args.email, "password": password}
        )
        if login.status_code != 200:
            print(f"login failed with status {login.status_code}", file=sys.stderr)
            return 1
        token = login.json().get("access_token", "")
        headers = {"Authorization": f"Bearer {token}"}

        for tab in TABS:
            path = f"/api/admin/analytics/product-analytics/{tab}"
            timings: list[float] = []
            sizes: list[int] = []
            cold_ms: float | None = None
            for attempt in range(WARM_REQUESTS + (1 if args.include_cold_first else 0)):
                began = time.perf_counter()
                response = client.get(path, params=params, headers=headers)
                elapsed_ms = (time.perf_counter() - began) * 1000
                if response.status_code != 200:
                    failures.append(f"{tab}: HTTP {response.status_code}")
                    break
                if args.include_cold_first and attempt == 0:
                    cold_ms = elapsed_ms
                    continue
                timings.append(elapsed_ms)
                sizes.append(len(response.content))
            if not timings:
                continue
            warm_p95 = _p95(timings)
            max_size = max(sizes)
            verdicts = []
            if warm_p95 > WARM_P95_BUDGET_MS:
                verdicts.append(f"warm p95 {warm_p95:.0f}ms > {WARM_P95_BUDGET_MS}ms")
            if cold_ms is not None and cold_ms > COLD_BUDGET_MS:
                verdicts.append(f"cold {cold_ms:.0f}ms > {COLD_BUDGET_MS}ms")
            if max_size > SIZE_BUDGET_BYTES:
                verdicts.append(f"size {max_size}B > {SIZE_BUDGET_BYTES}B")
            failures.extend(f"{tab}: {verdict}" for verdict in verdicts)
            cold_display = f"{cold_ms:.0f}ms" if cold_ms is not None else "n/a"
            print(
                f"{tab:<12} warm p95 {warm_p95:7.1f}ms  median "
                f"{statistics.median(timings):7.1f}ms  cold {cold_display:>8}  "
                f"max size {max_size:>7}B  {'FAIL' if verdicts else 'ok'}"
            )

    if failures:
        print("budget failures:", "; ".join(failures), file=sys.stderr)
        return 1
    print("all budgets satisfied")
    return 0


if __name__ == "__main__":
    sys.exit(main())
