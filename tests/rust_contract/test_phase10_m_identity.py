"""M release identity stays separate from approved migration transitions."""
from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FINAL_ALEMBIC = "ob_20261031_offboarding"
M_REVISIONS = ["sqlx_1"]


def string_constant(path: str, name: str) -> list[str]:
    source = (ROOT / path).read_text()
    match = re.search(rf"pub const {name}:.*?=\s*&\[(.*?)\];", source, re.DOTALL)
    assert match is not None, name
    return re.findall(r'"([a-z0-9_]+)"', match[1])


def test_m_startup_lists_are_exact_and_do_not_admit_old_bundle_pins():
    assert json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text()) == M_REVISIONS
    assert string_constant("rs/read/src/runtime/artifact.rs", "ALLOWED_REVISIONS") == M_REVISIONS
    assert string_constant("rs/read/src/runtime/config.rs", "PRODUCTION_REVISIONS") == M_REVISIONS


def test_m_policy_names_adoption_and_no_change_without_schema_changes():
    baseline = json.loads((ROOT / "deploy/rust/sqlx-baseline.json").read_text())
    assert baseline["alembic_head"] == FINAL_ALEMBIC
    rows = {
        tuple(line.split("\t"))
        for line in (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()
        if line and not line.startswith("#")
    }
    assert (FINAL_ALEMBIC, "sqlx_1") in rows
    assert ("sqlx_1", "sqlx_1") in rows
    assert all("head" not in pair and "latest" not in pair for pair in rows)
    assert sorted(path.name for path in (ROOT / "rs/read/migrations").glob("*.sql")) == ["1_baseline.sql"]


def test_native_runtime_projection_uses_the_production_identity_list():
    source = (ROOT / "rs/read/src/cli/ops/runtime_env.rs").read_text()
    assert "use crate::runtime::config::PRODUCTION_REVISIONS;" in source
    assert "!PRODUCTION_REVISIONS.contains(&migration_revision)" in source


def test_adopted_self_loop_revalidates_the_final_alembic_baseline():
    source = (ROOT / "rs/read/src/cli/migrate.rs").read_text()
    transition = source.split("pub async fn run_transition(", 1)[1].split("/// What a deploy-owned", 1)[0]
    assert "expected_current == ADOPTED_REVISION && target == ADOPTED_REVISION" in transition
    assert "check_adoption(connection, &baseline.alembic_head, baseline).await" in transition


def test_schema_capabilities_do_not_depend_on_startup_admissions():
    for path, function in (
        ("rs/read/src/streams/runtime.rs", "schema_has_streams"),
        ("rs/read/src/order_write/producer/mod.rs", "schema_has_producer"),
    ):
        source = (ROOT / path).read_text()
        body = source.split(f"pub fn {function}(", 1)[1].split("\n}", 1)[0]
        assert "ALLOWED_REVISIONS" not in body
        assert '"sqlx_1"' in body
