"""Pinned decision-87 Python reference; no live Python performance inputs."""
from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
from typing import Any

from scripts.rust_contract import workload

ROOT = Path(__file__).resolve().parents[2]
CONFIG_PATH = ROOT / "tests/rust_contract/gate-config.json"


def gate_config() -> dict[str, Any]:
    return json.loads(CONFIG_PATH.read_text())


def load_python_baseline(path: Path | None = None, pin: str | None = None) -> dict[str, Any]:
    """Refuse absent pins, changed bytes and incomplete route evidence."""
    try:
        config = gate_config()
        path = path if path is not None else ROOT / config["python_baseline_path"]
        pin = config["python_baseline_sha256"] if pin is None else pin
        data = path.read_bytes()
        if not pin or hashlib.sha256(data).hexdigest() != pin:
            raise ValueError("Python baseline digest mismatch or missing pin")
        document = json.loads(data)
        manifest = workload.load()
        expected = {r["id"] for r in workload.routes(manifest) + workload.tools(manifest)}
        if set(document["routes"]) != expected:
            raise ValueError("Python baseline route set is incomplete")
        for row in [*document["routes"].values(), {"p95": document["mix_p95"], **document}]:
            if type(row["p95"]) not in (int, float) or not math.isfinite(row["p95"]) or row["p95"] <= 0:
                raise ValueError("invalid Python baseline p95")
            if any(type(row[key]) is not int for key in ("offered", "successful", "errors_5xx", "busy_503")):
                raise ValueError("invalid baseline counts")
            if not 0 < row["successful"] <= row["offered"] or not 0 <= row["busy_503"] <= row["errors_5xx"] <= row["offered"] - row["successful"]:
                raise ValueError("inconsistent baseline counts")
            if row["error_rate"] != row["errors_5xx"] / row["offered"]:
                raise ValueError("inconsistent baseline error rate")
        if document["manifest_digest"] != workload.manifest_digest(manifest):
            raise ValueError("baseline workload manifest changed")
        for key in ("source_sha", "harness_sha", "source_file_sha256"):
            length = 64 if key.endswith("sha256") else 40
            value = document[key]
            if len(value) != length or any(c not in "0123456789abcdef" for c in value):
                raise ValueError("invalid baseline identity")
        if document["identity_attested_by"] != "PM, tracker decision 87" or document["measurement_date"] != "2026-10-18":
            raise ValueError("missing baseline attestation")
        return document
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise ValueError("missing or invalid frozen Python baseline") from error
