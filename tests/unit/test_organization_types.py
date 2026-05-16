import pytest
from pydantic import ValidationError

from app.schemas.organization import OrganizationCreate


SIGNUP_ORG_TYPES = [
    "SHIPPING_LINE",
    "SHIP_MANAGER",
    "FUEL_BUYER",
    "CHARTERER",
    "FUEL_SUPPLIER",
]


@pytest.mark.parametrize("org_type", SIGNUP_ORG_TYPES)
def test_signup_organization_types_are_accepted(org_type: str):
    org = OrganizationCreate(
        name=f"{org_type} Test Org",
        type=org_type,
        country_code="SG",
    )

    assert org.type.value == org_type


def test_unknown_organization_type_is_rejected():
    with pytest.raises(ValidationError):
        OrganizationCreate(
            name="Unsupported Org",
            type="UNSUPPORTED_ORG_TYPE",
            country_code="SG",
        )


@pytest.mark.parametrize(
    "org_type",
    [
        "BUNKER_BROKER",
        "PORT_AUTHORITY",
        "FUEL_TRADER",
        "FINANCIER",
        "INSURER",
        "INDUSTRY_ASSOC",
    ],
)
def test_ambiguous_or_non_trading_organization_types_are_not_signup_options(org_type: str):
    with pytest.raises(ValidationError, match="Organization type must be one of"):
        OrganizationCreate(
            name=f"{org_type} Test Org",
            type=org_type,
            country_code="SG",
        )
