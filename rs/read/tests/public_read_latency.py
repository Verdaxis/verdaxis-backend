"""Bounded sequential read regression against an attested disposable server.

This is not a benchmark. The unchanged integration client has a ten-second
request deadline; these small-corpus reads must finish within that limit.
"""
import json
import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
import httpx

from scripts.rust_contract.run_disposable import attest


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--map-only", action="store_true")
    args = parser.parse_args()
    url = os.environ["RUST_CONTRACT_URL"]
    attest(url, os.environ["DISPOSABLE_TEST_TOKEN"])
    failures = []
    with httpx.Client(timeout=10, trust_env=False) as client:
        for path in (
            "/api/orderbook",
            "/api/orderbook/bids",
            "/api/orderbook/aggregated",
            "/api/orderbook/regions",
            "/api/orderbook/fuel-types",
            "/api/orderbook/aggregated?region=Busan",
            "/api/orderbook/products",
            "/api/orderbook/regions",
            "/api/orderbook/fuel-types",
            "/api/orderbook/products",
            "/api/orderbook/regions",
            "/api/orderbook/fuel-types",
        ):
            if args.map_only and path in ("/api/orderbook", "/api/orderbook/bids"):
                continue
            start = time.monotonic()
            try:
                response = client.get(url + path)
                status = response.status_code
                if status != 200:
                    failures.append(f"{path}: HTTP {status}")
            except httpx.TimeoutException:
                status = "timeout"
                failures.append(f"{path}: exceeded integration deadline")
            print(json.dumps({"path": path, "status": status,
                              "seconds": round(time.monotonic() - start, 3)}), flush=True)
    assert not failures, failures


if __name__ == "__main__":
    main()
