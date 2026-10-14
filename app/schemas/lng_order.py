"""Declared bio-LNG terms. WTT emissions exclude engine methane slip."""

from decimal import Decimal
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from app.schemas.fame import SustainabilityScheme

ChainOfCustody = Literal["MASS_BALANCE", "PHYSICAL_SEGREGATION"]
DeliveryMethod = Literal["SHIP_TO_SHIP", "TRUCK_TO_SHIP", "TERMINAL_TO_SHIP"]
FeedstockCategory = Literal[
    "MANURE", "FOOD_WASTE", "SEWAGE_SLUDGE", "LANDFILL_GAS",
    "AGRICULTURAL_RESIDUES", "OTHER_WASTES_AND_RESIDUES", "ENERGY_CROPS",
]
PosTiming = Literal["WITH_DELIVERY", "AFTER_DELIVERY"]
SPEC_STANDARD = "ISO 23306:2020"
BioShare = Annotated[Decimal, Field(ge=1, le=100, allow_inf_nan=False)]
WttGhg = Annotated[Decimal, Field(ge=-150, le=100, allow_inf_nan=False)]
MethaneNumber = Annotated[Decimal, Field(ge=0, le=100, allow_inf_nan=False)]
# ISO 3166-1 assigned alpha-2 codes; shape alone would admit fictitious origins.
COUNTRY_CODES = frozenset("""
AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN
BO BQ BR BS BT BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ
DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI GL
GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM
JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME
MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP
NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RU RW SA SB SC SD
SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO
TR TT TV TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW
""".split())


class LngTerms(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)


class LngAskDeclaration(LngTerms):
    """Non-identifying seller declarations shared by full and public views."""
    side: Literal["ASK"]
    sustainability_scheme: SustainabilityScheme
    chain_of_custody: ChainOfCustody
    union_database_registered: bool
    bio_share_pct: BioShare = Field(decimal_places=2)
    feedstock: FeedstockCategory
    feedstock_detail: str | None = Field(None, max_length=200)
    production_country: str
    wtt_ghg_gco2e_mj: WttGhg = Field(decimal_places=2)
    specification_standard: Literal["ISO 23306:2020"]
    min_methane_number: MethaneNumber | None = None
    delivery_method: DeliveryMethod
    seller_is_physical_supplier: bool
    pos_timing: PosTiming

    @field_validator("production_country")
    @classmethod
    def valid_country(cls, value: str) -> str:
        if value not in COUNTRY_CODES:
            raise ValueError("Production country must be an uppercase ISO 3166-1 alpha-2 code")
        return value


class LngAskTerms(LngAskDeclaration):
    scheme_certificate_number: str = Field(min_length=3, max_length=64)
    physical_supplier_name: str = Field(min_length=2, max_length=120)


class LngPublicAskTerms(LngAskDeclaration):
    reseller: bool
    fueleu_recognition: Literal["UNION_DATABASE", "NOT_REGISTERED"]

    @model_validator(mode="before")
    @classmethod
    def redact_declaration(cls, value):
        if isinstance(value, LngAskTerms):
            value = value.model_dump()
        if isinstance(value, dict):
            value = dict(value)
            value.pop("scheme_certificate_number", None)
            value.pop("physical_supplier_name", None)
            if "seller_is_physical_supplier" in value:
                value["reseller"] = not value["seller_is_physical_supplier"]
            if "union_database_registered" in value:
                value["fueleu_recognition"] = (
                    "UNION_DATABASE" if value["union_database_registered"] else "NOT_REGISTERED"
                )
        return value


class LngBidTerms(LngTerms):
    side: Literal["BID"]
    accepted_schemes: list[SustainabilityScheme] = Field(min_length=1, max_length=2)
    accepted_custody: list[ChainOfCustody] = Field(
        default_factory=lambda: ["MASS_BALANCE", "PHYSICAL_SEGREGATION"],
        min_length=1, max_length=2,
    )
    require_union_database: bool = True
    min_bio_share_pct: BioShare | None = None
    max_wtt_ghg_gco2e_mj: WttGhg | None = None
    accepted_delivery_methods: list[DeliveryMethod] = Field(min_length=1, max_length=3)
    min_methane_number: MethaneNumber | None = None
    accept_resellers: bool = True

    @field_validator("accepted_schemes", "accepted_custody", "accepted_delivery_methods")
    @classmethod
    def unique_values(cls, value: list[str]) -> list[str]:
        if len(value) != len(set(value)):
            raise ValueError("Accepted values must be unique")
        return value


LngOrderTerms = Annotated[LngBidTerms | LngAskTerms, Field(discriminator="side")]
LngPublicOrderTerms = Annotated[LngBidTerms | LngPublicAskTerms, Field(discriminator="side")]


class LngTradeSnapshot(LngTerms):
    bid: LngBidTerms
    ask: LngAskTerms


class LngPublicTradeSnapshot(LngTerms):
    bid: LngBidTerms
    ask: LngPublicAskTerms
