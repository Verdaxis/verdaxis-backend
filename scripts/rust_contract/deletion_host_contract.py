"""Task 32 deletion host contract, over a fixture host inventory.

Reads one fixture inventory document as data and asserts each W5 cleanup
rule. The checks are the same ones a real post-W5 inventory must satisfy;
this module never contacts a host and never deletes anything.
"""
import argparse
import re

FORBIDDEN_REMOVAL_TOKENS = ("/usr/bin/python", "/usr/lib/python", "/usr/local/bin/python")


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def no_installed_reference_to_a_deleted_python_path(document):
    require(document["installed_references"] == [], "deleted Python paths remain installed")
    deleted_units = {unit.split("/")[-1] for unit in document.get("deleted_units", ())}
    for host, state in document["hosts"].items():
        python_units = [unit for unit in state["installed_units"] if unit in deleted_units]
        require(not python_units, f"deleted units remain installed on {host}: {python_units}")


def host_python_cleanup_removes_only_verdaxis_owned_paths(document):
    owned = [root.rstrip("/") for root in document["owned_roots"]]
    forbidden = document["forbidden_removals"]
    for host, removals in document["cleanup_removals"].items():
        require(removals, f"no recorded cleanup for host: {host}")
        for removed in removals:
            normalized = removed.rstrip("/")
            require(any(normalized == root or normalized.startswith(root + "/")
                        for root in owned),
                    f"cleanup leaves the owned roots: {removed}")
            require(not any(removed.startswith(token) for token in FORBIDDEN_REMOVAL_TOKENS),
                    f"cleanup names a system interpreter path: {removed}")
    for path in forbidden:
        require(not any(entry == path
                        for removals in document["cleanup_removals"].values()
                        for entry in removals),
                f"cleanup removes a forbidden path: {path}")


def staging_is_cleaned_before_production(document):
    staging = document["hosts"]["staging"]["cleanup"]
    production = document["hosts"]["production"]["cleanup"]
    require(staging["status"] == "complete", "staging cleanup is not complete")
    require(production["status"] == "complete", "production cleanup is not complete")
    require(staging["completed_at"] < production["completed_at"],
            "staging must be cleaned before production")


def old_timer_and_new_job_do_not_duplicate(document):
    superseded = {unit.split("/")[-1] for unit in document.get("deleted_units", ())}
    for host, state in document["hosts"].items():
        installed = state["installed_units"]
        require(not [unit for unit in installed if unit in superseded],
                f"superseded units remain installed on {host}")
        by_role = {}
        for unit in installed:
            role = document.get("unit_roles", {}).get(unit)
            if role:
                by_role.setdefault(role, []).append(unit)
        for role, units in by_role.items():
            require(len(units) == 1, f"job {role} runs more than once on {host}: {sorted(units)}")


def previous_release_artifact_still_starts(document):
    evidence = document["rollback_evidence"]
    bundle = evidence["pre_m_bundle"]
    require(bundle["status"] == "accepted", "no accepted pre-M rollback bundle")
    require(re.fullmatch(r"[0-9a-f]{64}", bundle["sha256"]),
            "pre-M bundle digest is not SHA-256")
    require(evidence["observed_start"], "the previous release artifact was not observed to start")


def daily_backup_and_staging_pull_still_run(document):
    retained = document["backup_and_pull"]
    commands = {entry["command"] for entries in document["cron"].values() for entry in entries}
    for name, entry in retained.items():
        require(entry["retained"], f"preserved schedule was dropped: {name}")
        require(entry["expression"], f"schedule lost its expression: {name}")
        require(entry["destination"], f"schedule lost its destination: {name}")
        require(entry["credential_references"],
                f"schedule lost its credential references: {name}")
        require(entry["path"] in commands, f"cron no longer runs {name}")


CHECKS = {check.__name__: check for check in (
    no_installed_reference_to_a_deleted_python_path,
    host_python_cleanup_removes_only_verdaxis_owned_paths,
    staging_is_cleaned_before_production,
    old_timer_and_new_job_do_not_duplicate,
    previous_release_artifact_still_starts,
    daily_backup_and_staging_pull_still_run,
)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", choices=sorted(CHECKS), required=True)
    parser.add_argument("--inventory", required=True)
    parser.add_argument("--delete-units", nargs="*", default=[])
    args = parser.parse_args()
    import json
    document = json.loads(open(args.inventory, encoding="utf-8").read())
    document.setdefault("deleted_units", args.delete_units)
    CHECKS[args.check](document)
    print("PASS " + args.check)


if __name__ == "__main__":
    main()
