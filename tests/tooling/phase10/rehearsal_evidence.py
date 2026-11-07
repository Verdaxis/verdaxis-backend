"""Validate portable, non-secret evidence from the disposable rehearsal."""

import hashlib
import json
import re
from pathlib import Path

REQUIRED_CHECKS = (
    "host_inventory_is_complete_and_signed",
    "host_inventory_covers_units_timers_dropins_cron_usr_local_backup_and_processes",
    "nightly_backup_and_staging_pull_schedules_are_unchanged",
    "rehearsal_runs_every_installed_unit_once",
    "rehearsal_deletes_python_and_nothing_restarts",
    "rehearsal_rollback_is_one_change_per_group",
    "release_bundle_rollback_starts_the_actual_pre_m_binary",
    "rollback_interruption_at_each_publication_never_starts_a_mixed_bundle",
    "no_python_process_remains_after_the_rehearsal",
    "every_required_scenario_has_evidence",
)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate evidence key")
        result[key] = value
    return result


def validate_evidence(directory, *, environment, release_sha):
    """Require all checks and digest-bound files; source plans are not results."""
    root = Path(directory).resolve(strict=True)
    result_path = root / "results.json"
    if result_path.is_symlink():
        raise ValueError("results must be a regular file")
    if not result_path.is_file() or not 0 < result_path.stat().st_size <= 1024 * 1024:
        raise ValueError("results must be a bounded nonempty file")
    result = json.loads(result_path.read_text(), object_pairs_hook=unique_object)
    if not isinstance(result, dict):
        raise ValueError("results must be an object")  # noqa: TRY004 - malformed evidence is one refusal class
    if (
        result.get("schema_version") != 1
        or result.get("scope") != "disposable-candidate"
    ):
        raise ValueError("invalid rehearsal evidence scope or version")
    if (
        environment not in {"staging", "production"}
        or result.get("environment") != environment
    ):
        raise ValueError("rehearsal environment mismatch")
    if (
        not re.fullmatch(r"[0-9a-f]{40}", release_sha)
        or result.get("release_sha") != release_sha
    ):
        raise ValueError("rehearsal release mismatch")
    rows = result.get("checks")
    if not isinstance(rows, list) or len(rows) != len(REQUIRED_CHECKS):
        raise ValueError("required rehearsal scenario has no evidence")
    names = [row.get("name") for row in rows if isinstance(row, dict)]
    if names != list(REQUIRED_CHECKS):
        raise ValueError("required rehearsal scenario has no evidence")
    run_id = result.get("run_id")
    if not isinstance(run_id, str) or not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("invalid rehearsal run identity")
    observations = set()
    for row in rows:
        if row.get("status") != "PASS":
            raise ValueError(f"rehearsal check did not pass: {row['name']}")
        files = row.get("evidence")
        if not isinstance(files, list) or not files:
            raise ValueError(
                f"required rehearsal scenario has no evidence: {row['name']}"
            )
        seen = set()
        for item in files:
            if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
                raise ValueError("invalid evidence reference")
            relative = item["path"]
            if not isinstance(relative, str) or not relative or "\\" in relative:
                raise ValueError("invalid evidence path")
            path = Path(relative)
            if path.is_absolute() or any(
                part in {".", ".."} for part in relative.split("/")
            ):
                raise ValueError("evidence path escapes its directory")
            if relative in seen:
                raise ValueError("duplicate evidence reference")
            seen.add(relative)
            candidate = root
            for part in path.parts:
                candidate = candidate / part
                if candidate.is_symlink():
                    raise ValueError("evidence symlinks are not accepted")
            candidate.resolve(strict=True).relative_to(root)
            if (
                not candidate.is_file()
                or candidate == result_path
                or not 0 < candidate.stat().st_size <= 16 * 1024 * 1024
            ):
                raise ValueError("evidence must be a bounded nonempty observation file")
            expected = item["sha256"]
            if not isinstance(expected, str) or not re.fullmatch(
                r"[0-9a-f]{64}", expected
            ):
                raise ValueError("invalid evidence digest")
            if hashlib.sha256(candidate.read_bytes()).hexdigest() != expected:
                raise ValueError("evidence digest mismatch")
            if relative in observations:
                raise ValueError("each scenario needs a distinct observation")
            observations.add(relative)
            observation = json.loads(
                candidate.read_text(), object_pairs_hook=unique_object
            )
            if not isinstance(observation, dict) or any(
                observation.get(key) != value
                for key, value in {
                    "schema_version": 1,
                    "check": row["name"],
                    "environment": environment,
                    "release_sha": release_sha,
                    "run_id": run_id,
                    "outcome": "PASS",
                    "producer": "deploy/tests/full-rehearsal.sh",
                }.items()
            ):
                raise ValueError("observation identity or outcome differs")
            references = observation.get("observations")
            if not isinstance(references, list) or not references:
                raise ValueError("observation has no supporting evidence")
            for reference in references:
                validate_support(root, reference, candidate, result_path)
    return result


def validate_support(root, reference, observation, results):
    if not isinstance(reference, dict) or set(reference) != {"path", "sha256"}:
        raise ValueError("invalid observation support reference")
    relative = reference["path"]
    if not isinstance(relative, str) or not relative or "\\" in relative:
        raise ValueError("invalid support path")
    path = Path(relative)
    if path.is_absolute() or any(
        part in {".", "..", ""} for part in relative.split("/")
    ):
        raise ValueError("support path escapes evidence directory")
    candidate = root
    for part in path.parts:
        candidate /= part
        if candidate.is_symlink():
            raise ValueError("support symlinks are not accepted")
    candidate.resolve(strict=True).relative_to(root)
    if (
        candidate in {observation, results}
        or not candidate.is_file()
        or not 0 < candidate.stat().st_size <= 16 * 1024 * 1024
    ):
        raise ValueError("support must be a bounded observation file")
    expected = reference["sha256"]
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("invalid support digest")
    if hashlib.sha256(candidate.read_bytes()).hexdigest() != expected:
        raise ValueError("support digest differs")


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument(
        "--environment", required=True, choices=("staging", "production")
    )
    parser.add_argument("--release-sha", required=True)
    args = parser.parse_args()
    validate_evidence(
        args.directory, environment=args.environment, release_sha=args.release_sha
    )
