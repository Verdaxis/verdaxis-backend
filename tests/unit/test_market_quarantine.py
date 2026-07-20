"""Safety contracts for explicit market-data remediation tooling."""

from uuid import UUID

import pytest

from app.demo_identities import CANONICAL_DEMO_ORG_NAMES, KNOWN_TEST_ORG_IDS
from app.environment_database import (
    attestation_phrase,
    validate_database_target,
)
from app.services.market_quarantine import (
    MAX_QUARANTINE_IDS,
    OperatorContext,
    validate_order_ids,
    validate_write_authorization,
)


def test_attestation_derives_exact_staging_database_from_environment():
    expected = attestation_phrase("staging", "verdaxis_staging")
    assert validate_database_target(
        environment="staging",
        database_url="postgresql+asyncpg://migrator:secret@db/verdaxis_staging",
        current_database="verdaxis_staging",
        supplied=expected,
    ) == "verdaxis_staging"

    with pytest.raises(ValueError, match="attestation"):
        validate_database_target(
            environment="staging",
            database_url="postgresql+asyncpg://migrator:secret@db/verdaxis_staging",
            current_database="verdaxis_staging",
            supplied=attestation_phrase("test", "verdaxis_staging"),
        )


@pytest.mark.parametrize(
    ("environment", "database_name"),
    [
        ("staging", "verdaxis"),
        ("production", "verdaxis_staging"),
    ],
)
def test_environment_label_cannot_spoof_production_or_staging(
    environment: str,
    database_name: str,
):
    with pytest.raises(ValueError, match="environment"):
        validate_database_target(
            environment=environment,
            database_url=f"postgresql+asyncpg://migrator:secret@db/{database_name}",
            current_database=database_name,
            supplied=attestation_phrase(environment, database_name),
        )


def test_url_database_and_current_database_are_both_attested():
    with pytest.raises(ValueError, match="connected database"):
        validate_database_target(
            environment="staging",
            database_url="postgresql+asyncpg://migrator:secret@db/verdaxis_staging",
            current_database="verdaxis",
            supplied=attestation_phrase("staging", "verdaxis_staging"),
        )


def test_test_environment_requires_an_explicit_disposable_database_suffix():
    disposable = "market_integrity_42_test"
    assert validate_database_target(
        environment="test",
        database_url=f"postgresql+asyncpg://tester:secret@db/{disposable}",
        current_database=disposable,
        supplied=attestation_phrase("test", disposable),
    ) == disposable

    with pytest.raises(ValueError, match="disposable"):
        validate_database_target(
            environment="test",
            database_url="postgresql+asyncpg://tester:secret@db/verdaxis",
            current_database="verdaxis",
            supplied=attestation_phrase("test", "verdaxis"),
        )


def test_quarantine_ids_are_explicit_unique_and_bounded():
    first = UUID("00000000-dead-beef-0000-aaa0e15eed01")
    assert validate_order_ids([str(first), str(first)]) == (first,)

    with pytest.raises(ValueError, match="at least one"):
        validate_order_ids([])
    with pytest.raises(ValueError, match="at most"):
        validate_order_ids(
            [str(UUID(int=index + 1)) for index in range(MAX_QUARANTINE_IDS + 1)]
        )


def test_canonical_demo_renames_are_exact_id_mappings_only():
    assert len(CANONICAL_DEMO_ORG_NAMES) == 14
    assert all(name.startswith("Verdaxis Demo ") for name in CANONICAL_DEMO_ORG_NAMES.values())
    assert all(isinstance(organization_id, UUID) for organization_id in CANONICAL_DEMO_ORG_NAMES)
    assert len(KNOWN_TEST_ORG_IDS) == 8
    assert UUID("9e63f7a1-0000-4000-8000-000000000001") in KNOWN_TEST_ORG_IDS
    assert UUID("9e63f7a1-0000-4000-8000-000000000011") in KNOWN_TEST_ORG_IDS


def test_production_write_requires_matching_product_approval_reference():
    context = OperatorContext(
        environment="production",
        database_name="verdaxis_prod_clone",
        operator="test-operator",
        reason="explicitly approved exact-ID quarantine",
        reference="PRODUCT-APPROVAL-42",
    )
    with pytest.raises(ValueError, match="product approval"):
        validate_write_authorization(
            context,
            apply=True,
            production_approval_reference=None,
        )
    with pytest.raises(ValueError, match="product approval"):
        validate_write_authorization(
            context,
            apply=True,
            production_approval_reference="DIFFERENT-REFERENCE",
        )

    validate_write_authorization(
        context,
        apply=True,
        production_approval_reference=context.reference,
    )
