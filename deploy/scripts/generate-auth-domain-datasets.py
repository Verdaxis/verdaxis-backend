"""Vendor the registration domain datasets for the Rust runtime (Phase 10 Task 9).

The Python projection (scripts/prepare_rust_runtime.py::phase3_settings) reads
the installed packages disposable-email-domains and free-email-domains. The Rust
runtime has no Python environment, so this script writes the same compact JSON
once into the repository, with a manifest that records the package versions,
the counts and the SHA-256 digest that the Rust loader checks.

Usage (with a Python that has the requirements.txt pins installed, run with -I):
    python -I deploy/scripts/generate-auth-domain-datasets.py          # write
    python -I deploy/scripts/generate-auth-domain-datasets.py --check  # compare

--check writes nothing and exits 1 when the committed files differ from what the
installed packages give.
"""

import argparse
import hashlib
import importlib.metadata
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATASETS_PATH = "deploy/rust/auth-domain-datasets.json"
MANIFEST_PATH = "deploy/rust/auth-domain-datasets.manifest.json"
PACKAGES = ("disposable-email-domains", "free-email-domains")
FORM = (
    'compact JSON {"public": sorted whitelist, "disposable": sorted blocklist}, '
    "as scripts/prepare_rust_runtime.py::phase3_settings projects AUTH_DOMAIN_DATASETS"
)


def pinned_versions() -> dict:
    """The exact requirements.txt pins; the installed packages must match them."""
    pins = {}
    for line in (ROOT / "requirements.txt").read_text().splitlines():
        name, separator, version = line.partition("==")
        if separator and name.strip() in PACKAGES:
            pins[name.strip()] = version.strip()
    for package in PACKAGES:
        installed = importlib.metadata.version(package)
        if pins.get(package) != installed:
            raise SystemExit(
                f"{package}: installed {installed}, pinned {pins.get(package)}"
            )
    return pins


def build() -> tuple[bytes, bytes]:
    from disposable_email_domains import blocklist
    from free_email_domains import whitelist

    # The same expression as phase3_settings, so the bytes are identical.
    datasets = {"public": sorted(whitelist), "disposable": sorted(blocklist)}
    datasets_bytes = json.dumps(datasets, separators=(",", ":")).encode()
    manifest = {
        "counts": {key: len(values) for key, values in datasets.items()},
        "file": DATASETS_PATH,
        "form": FORM,
        "sha256": hashlib.sha256(datasets_bytes).hexdigest(),
        "source": pinned_versions(),
    }
    manifest_bytes = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
    return datasets_bytes, manifest_bytes


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    expected = dict(zip((DATASETS_PATH, MANIFEST_PATH), build()))
    if arguments.check:
        stale = [
            path
            for path, data in expected.items()
            if (ROOT / path).read_bytes() != data
        ]
        for path in stale:
            print(f"{path} differs from the installed packages", file=sys.stderr)
        return 1 if stale else 0
    for path, data in expected.items():
        (ROOT / path).write_bytes(data)
    return 0


if __name__ == "__main__":
    sys.exit(main())
