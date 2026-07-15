from typing import Literal

from pydantic import BaseModel, ConfigDict, StrictBool, StrictStr


MarketProduct = Literal[
    "BIO_METHANOL",
    "E_METHANOL",
    "BIO_ETHANOL",
    "SYNTHETIC_ETHANOL",
]


class MarketWatchPreferences(BaseModel):
    model_config = ConfigDict(extra="forbid")

    products: list[MarketProduct]
    portIds: list[StrictStr]


class NotificationPreferences(BaseModel):
    model_config = ConfigDict(extra="forbid")

    email_trade_updates: StrictBool
    email_market_alerts: StrictBool
    email_compliance_digest: StrictBool
    email_system_announcements: StrictBool
    inapp_trade_updates: StrictBool
    inapp_market_alerts: StrictBool
    inapp_order_matches: StrictBool


class TutorialPreferences(BaseModel):
    model_config = ConfigDict(extra="forbid")

    completed: StrictBool


NAMESPACE_SCHEMAS: dict[str, type[BaseModel]] = {
    "market_watch": MarketWatchPreferences,
    "notifications": NotificationPreferences,
    "tutorial": TutorialPreferences,
}
