"""Portable contracts; installed rehearsal evidence is produced only in Linux CI."""

import copy
import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tests/rust_contract/phase10/rehearsal_evidence.py"
SPEC = importlib.util.spec_from_file_location("rehearsal_evidence", MODULE_PATH)
evidence = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evidence)
SHA = "a" * 40


@pytest.fixture
def sample(tmp_path):
    support = tmp_path / "support.txt"
    support.write_text("Fixture only: no installed acceptance.\n")
    support_reference = {
        "path": support.name,
        "sha256": hashlib.sha256(support.read_bytes()).hexdigest(),
    }
    rows = []
    for index, name in enumerate(evidence.REQUIRED_CHECKS):
        observation = tmp_path / (
            "observation.json" if index == 0 else f"observation-{index}.json"
        )
        observation.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "check": name,
                    "environment": "staging",
                    "release_sha": SHA,
                    "run_id": "123",
                    "outcome": "PASS",
                    "producer": "deploy/tests/full-rehearsal.sh",
                    "observations": [support_reference],
                }
            )
        )
        reference = {
            "path": observation.name,
            "sha256": hashlib.sha256(observation.read_bytes()).hexdigest(),
        }
        rows.append({"name": name, "status": "PASS", "evidence": [reference]})
    document = {
        "schema_version": 1,
        "scope": "disposable-candidate",
        "environment": "staging",
        "release_sha": SHA,
        "run_id": "123",
        "checks": rows,
    }
    return tmp_path, document


def validate(sample):
    directory, document = sample
    (directory / "results.json").write_text(json.dumps(document))
    return evidence.validate_evidence(directory, environment="staging", release_sha=SHA)


def test_rehearsal_evidence_checker_accepts_complete_fixture(sample):
    assert validate(sample)["scope"] == "disposable-candidate"


@pytest.mark.parametrize("name", evidence.REQUIRED_CHECKS)
@pytest.mark.parametrize("change", ["missing", "renamed", "empty", "failed", "skipped"])
def test_every_required_scenario_has_evidence(sample, name, change):
    row = next(row for row in sample[1]["checks"] if row["name"] == name)
    if change == "missing":
        sample[1]["checks"].remove(row)
    elif change == "renamed":
        row["name"] += "_other"
    elif change == "empty":
        row["evidence"] = []
    else:
        row["status"] = change.upper()
    with pytest.raises(ValueError):
        validate(sample)


@pytest.mark.parametrize(
    "change", ["tampered", "missing", "traversal", "absolute", "symlink", "duplicate"]
)
def test_rehearsal_evidence_is_bound_to_real_files(sample, change):
    directory, document = sample
    reference = document["checks"][0]["evidence"][0]
    if change == "tampered":
        (directory / "observation.json").write_text("changed")
    elif change == "missing":
        (directory / "observation.json").unlink()
    elif change == "traversal":
        reference["path"] = "../observation.json"
    elif change == "absolute":
        reference["path"] = str(directory / "observation.json")
    elif change == "symlink":
        (directory / "link.json").symlink_to("observation.json")
        reference["path"] = "link.json"
    else:
        document["checks"][0]["evidence"].append(copy.deepcopy(reference))
    with pytest.raises((ValueError, FileNotFoundError)):
        validate(sample)


@pytest.mark.parametrize(
    "field,value",
    [("scope", "cutover"), ("environment", "production"), ("release_sha", "b" * 40)],
)
def test_rehearsal_evidence_cannot_change_identity(sample, field, value):
    sample[1][field] = value
    with pytest.raises(ValueError):
        validate(sample)


def test_rehearsal_manifest_covers_each_named_check():
    manifest = json.loads(
        (ROOT / "tests/rust_contract/phase10/evidence-manifest.json").read_text()
    )
    rows = manifest["rehearsal"]["scenarios"]
    assert len(rows) == len(evidence.REQUIRED_CHECKS)
    assert {row["name"] for row in rows} == set(evidence.REQUIRED_CHECKS)
    for index, row in enumerate(rows):
        assert row["scope"] == "disposable-candidate"
        assert row["producer"] == "deploy/tests/full-rehearsal.sh --disposable"
        assert row["name"] == evidence.REQUIRED_CHECKS[index]
        assert row["evidence"] == "results.json#/checks/" + str(index)
    assert manifest["rehearsal"]["results_committed"] is False


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_rehearsal_saved_site_uses_supported_window_composer(environment):
    from scripts.rust_phase10_window_caddy import candidate

    path = ROOT / "deploy/tests/rehearsal/caddy.py"
    spec = importlib.util.spec_from_file_location("rehearsal_caddy", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    original = module.saved_site(ROOT, environment)
    updated = candidate(original, environment)
    assert "tls internal" in updated
    assert (
        "reverse_proxy localhost:" + ("8000" if environment == "production" else "8001")
        not in updated
    )
    assert "@rust_crud_watchlists" in updated
    assert "@rust_abilities" in updated


@pytest.mark.parametrize(
    "change", ["identity", "outcome", "support", "duplicate", "text"]
)
def test_rehearsal_observations_are_structured_and_bound(sample, change):
    directory, document = sample
    row = document["checks"][0]
    path = directory / row["evidence"][0]["path"]
    observation = json.loads(path.read_text())
    if change == "identity":
        observation["environment"] = "production"
    elif change == "outcome":
        observation["outcome"] = "FAIL"
    elif change == "support":
        (directory / "support.txt").write_text("changed")
    elif change == "duplicate":
        document["checks"][1]["evidence"] = copy.deepcopy(row["evidence"])
    if change == "text":
        path.write_text("unstructured file")
    else:
        path.write_text(json.dumps(observation))
    row["evidence"][0]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    with pytest.raises(ValueError):
        validate(sample)


def test_rehearsal_large_observation_is_refused(sample):
    path = sample[0] / "observation.json"
    with path.open("wb") as stream:
        stream.truncate(16 * 1024 * 1024 + 1)
    with pytest.raises(ValueError, match="bounded"):
        validate(sample)


def test_rehearsal_feature_ref_requires_disposable_runtime():
    import subprocess

    command = [
        "bash",
        str(ROOT / "deploy/scripts/verify-artifact.sh"),
        "--source-sha",
        SHA,
        "--workflow-ref",
        "refs/heads/feat/rust-phase-10-rh",
    ]
    result = subprocess.run(
        command, cwd=ROOT, text=True, capture_output=True, check=False
    )
    assert result.returncode != 0
    assert "unapproved source ref" in result.stderr


def test_rehearsal_provenance_environment_is_limited_to_trusted_verifier():
    source = (ROOT / "scripts/deploy.sh").read_text()
    assert 'ARTIFACT_VERIFIER_ENV=("${BINARY_ENV[@]}")' in source
    assert '"${ARTIFACT_VERIFIER_ENV[@]}" bash "$RUST_ARTIFACT_VERIFIER_PATH"' in source
    assert "BINARY_ENV+=(CI=" not in source
    assert "HOSTED_REHEARSAL=0" in source
    assert "REHEARSAL_PUBLIC_KEY == /etc/verdaxis/artifact-provenance.pub.pem" in source
    assert 'id -u verdaxis-prod' in source
    assert "0:644" in source
    assert "PROVENANCE_PUBLIC_KEY_PATH=$REHEARSAL_PUBLIC_KEY" in source


def test_rehearsal_unapproved_runtime_refuses_before_mutation():
    import os
    import subprocess

    environment = dict(os.environ, CI="false", GITHUB_ACTIONS="false")
    result = subprocess.run(
        ["bash", str(ROOT / "deploy/tests/full-rehearsal.sh"), "--disposable"],
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 2
    assert "FAIL disposable_environment" in result.stderr


def test_rehearsal_old_guard_is_bound_to_the_actual_release_source():
    source = (ROOT / "deploy/scripts/pre-m-bundle.sh").read_text()
    assert "guard=$(file_of_kind guard)" in source
    assert '"$checkout/scripts/verify_deploy_start_guard.py"' in source
    assert 'cmp -s "$guard" "$work/source-guard"' in source
    assert 'show "$release:${guard#"$checkout/"}"' in source


def test_rehearsal_noop_publication_requires_exact_bytes_and_metadata():
    source = (ROOT / "deploy/scripts/pre-m-bundle.sh").read_text()
    body = source[source.index("publish() {") : source.index("write_state() {")]
    assert body.index('safe_path "$destination"') < body.index(
        'cmp -s -- "$source" "$destination"'
    )
    assert "stat -c '%u:%g:%a'" in body
    assert 'stat -c %h "$destination"' in body
    assert body.index('cmp -s -- "$source" "$destination"') < body.index(
        "publication_count=$((publication_count + 1))"
    )
    assert 'publication_boundary before "$destination"' in body
    assert 'publication_boundary after "$destination"' in body


def test_rehearsal_pre_m_aliases_remain_bound_to_concrete_native_bytes():
    source = (ROOT / "deploy/scripts/pre-m-bundle.sh").read_text()
    assert "native_dependency_aliases" in source
    assert 'readlink -f -- "$path"' in source
    assert "native alias owner differs" in source
    assert "native alias path is writable" in source
    assert 'regular "$path"' in source
    assert "native loader dependency differs" in source


def test_rehearsal_native_directory_aliases_have_an_exact_supported_subset():
    source = (ROOT / "deploy/scripts/pre-m-bundle.sh").read_text()
    assert "native alias directory link unsupported" in source
    assert "/bin:usr/bin|/bin:/usr/bin" in source
    assert "/lib:usr/lib|/lib:/usr/lib" in source
    assert "multi-hop native alias unsupported" in source
    assert "link_target =~ ^[A-Za-z0-9_.+-]+$" in source
