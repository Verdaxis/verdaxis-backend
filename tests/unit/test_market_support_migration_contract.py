from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def test_market_support_tables_are_in_runtime_acl_policy():
    policy = (ROOT / "deploy/postgres/app_acl_policy.sql").read_text()
    for table in (
        "admin_capability_grants",
        "organization_support_authorizations",
        "order_support_attributions",
        "market_support_action_receipts",
    ):
        assert f"('{table}'," in policy


def test_market_support_migration_is_allowlisted():
    checkpoints = (ROOT / "deploy/migration-checkpoints.tsv").read_text()
    assert (
        "sse_20260720_market_event_stream\tms_20260722_delegated_listings"
        in checkpoints
    )
    assert (
        "ms_20260722_delegated_listings\tms_20260722_delegated_listings"
        in checkpoints
    )
