from scripts.rust_contract.phase2_scenarios import select_scenarios


def test_preferences_uses_shared_registry_and_real_routes():
    scenarios = select_scenarios("preferences", None)
    assert len(scenarios) == 1
    scenario = scenarios[0]
    assert scenario.name == "preferences"
    assert scenario.route_qualification
    assert scenario.races is not None
    assert scenario.build
