"""Bio-LNG contract terms (docs/plans/2026-10-14-bio-lng-design.md, section 3).

Sellers declare the facts of their Proof of Sustainability; buyers state the
requirements an offer must meet. These are contract declarations, not
evidence that Verdaxis verified them.
"""
from __future__ import annotations

from decimal import Decimal
from typing import Annotated, Literal, Union

from pydantic import BaseModel, ConfigDict, Field, StrictBool, field_validator

from app.market_catalog import BIO_LNG_SPECIFICATION_STANDARD

SustainabilityScheme = Literal["ISCC_EU", "REDCERT_EU"]
ChainOfCustody = Literal["MASS_BALANCE", "PHYSICAL_SEGREGATION"]
DeliveryMethod = Literal["SHIP_TO_SHIP", "TRUCK_TO_SHIP", "TERMINAL_TO_SHIP"]
FeedstockCategory = Literal[
    "MANURE",
    "FOOD_WASTE",
    "SEWAGE_SLUDGE",
    "LANDFILL_GAS",
    "AGRICULTURAL_RESIDUES",
    "OTHER_WASTES_AND_RESIDUES",
    "ENERGY_CROPS",
]
PosTiming = Literal["WITH_DELIVERY", "AFTER_DELIVERY"]
FuelEURecognition = Literal["UNION_DATABASE", "NOT_REGISTERED"]
SpecificationStandard = Literal[BIO_LNG_SPECIFICATION_STANDARD]

BioSharePct = Annotated[Decimal, Field(ge=1, le=100, allow_inf_nan=False)]
# Certified well-to-tank GHG intensity, excluding engine methane slip. Manure
# pathways can be negative because of avoided emissions credited in the PoS.
WttGhgIntensity = Annotated[Decimal, Field(ge=-150, le=100, allow_inf_nan=False)]
MethaneNumber = Annotated[Decimal, Field(ge=0, le=100, allow_inf_nan=False)]

ALL_CHAINS_OF_CUSTODY: tuple[ChainOfCustody, ...] = ("MASS_BALANCE", "PHYSICAL_SEGREGATION")


class _LngTerms(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)


class _LngAskDeclaration(_LngTerms):
    """Seller declarations that the public order book may show."""

    side: Literal["ASK"]
    sustainability_scheme: SustainabilityScheme
    chain_of_custody: ChainOfCustody
    union_database_registered: StrictBool
    bio_share_pct: Annotated[BioSharePct, Field(decimal_places=2)]
    feedstock: FeedstockCategory
    feedstock_detail: str | None = Field(None, max_length=200)
    production_country: str = Field(pattern=r"^[A-Z]{2}$", description="ISO 3166-1 alpha-2")
    wtt_ghg_gco2e_mj: Annotated[WttGhgIntensity, Field(decimal_places=2)]
    specification_standard: SpecificationStandard
    min_methane_number: MethaneNumber | None = None
    delivery_method: DeliveryMethod
    seller_is_physical_supplier: StrictBool
    pos_timing: PosTiming


class LngAskTerms(_LngAskDeclaration):
    """Full seller declarations; identity fields stay private until confirmation."""

    scheme_certificate_number: str = Field(
        min_length=3, max_length=64, description="The seller's own scheme certificate"
    )
    physical_supplier_name: str = Field(
        min_length=2,
        max_length=120,
        description="The licensed physical supplier or bunkering partner",
    )


class LngPublicAskTerms(_LngAskDeclaration):
    """Pre-confirmation ask view without the certificate or supplier identity."""

    reseller: StrictBool
    fueleu_recognition: FuelEURecognition


class LngBidTerms(_LngTerms):
    """Buyer requirements an ask must meet before it can match or be taken."""

    side: Literal["BID"]
    accepted_schemes: list[SustainabilityScheme] = Field(min_length=1)
    accepted_custody: list[ChainOfCustody] = Field(
        default_factory=lambda: list(ALL_CHAINS_OF_CUSTODY), min_length=1
    )
    require_union_database: StrictBool = True
    min_bio_share_pct: BioSharePct | None = None
    max_wtt_ghg_gco2e_mj: WttGhgIntensity | None = None
    accepted_delivery_methods: list[DeliveryMethod] = Field(min_length=1)
    min_methane_number: MethaneNumber | None = None
    # "Physical suppliers only" in the order form sets this to false.
    accept_resellers: StrictBool = True

    @field_validator("accepted_schemes", "accepted_custody", "accepted_delivery_methods")
    @classmethod
    def _require_unique_values(cls, values: list[str]) -> list[str]:
        if len(set(values)) != len(values):
            raise ValueError("values must be unique")
        return values


LngOrderTerms = Annotated[Union[LngAskTerms, LngBidTerms], Field(discriminator="side")]
LngPublicOrderTerms = Annotated[
    Union[LngPublicAskTerms, LngBidTerms], Field(discriminator="side")
]


class LngTradeSnapshot(_LngTerms):
    """Both sides' terms, frozen when the trade is created."""

    bid: LngBidTerms
    ask: LngAskTerms


class LngPublicTradeSnapshot(_LngTerms):
    bid: LngBidTerms
    ask: LngPublicAskTerms
