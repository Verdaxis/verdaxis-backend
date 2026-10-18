"""Task 20 must select its own database scenario, not a synthetic fallback."""
from scripts.rust_contract.phase2_scenarios import select_scenarios


def test_order_write_group_has_postgres_actions_and_races():
    selected = select_scenarios("order-write-boundary", [])
    assert len(selected) == 1
    scenario = selected[0]
    assert scenario.name == "order-write-postgres"
    assert scenario.races is not None
    assert scenario.mixed is not None
    assert "order_write" in " ".join(scenario.build)
