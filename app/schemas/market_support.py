"""Contracts for the non-impersonating market-support workspace."""
from __future__ import annotations

import re
from datetime import UTC, datetime
from decimal import Decimal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from app.models.market_support import (
    MarketSupportAuthorizationStatus,
    MarketSupportCapability,
    MarketSupportContextScope,
    MarketSupportContextStatus,
)
from app.models.orderbook import OrderCreationMethod, OrderSide
from app.schemas.orderbook import (
    MarketSupportFinalConfirmation as _MarketSupportFinalConfirmation,
    OrderCreate,
    OrderResponse,
)
from app.schemas.pagination import PaginatedResponse

MarketSupportFinalConfirmation = _MarketSupportFinalConfirmation


_SHA256 = re.compile(r"^[0-9a-f]{64}$")


def _utc(value: datetime, field: str) -> datetime:
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError(f"{field} must include a timezone")
    return value.astimezone(UTC)


class CapabilityAssignmentCreate(BaseModel):
    user_id: UUID
    capability: MarketSupportCapability
    reason: str = Field(min_length=3, max_length=500)
    expires_at: datetime | None = None

    @field_validator("reason")
    @classmethod
    def normalize_reason(cls, value: str) -> str:
        return value.strip()

    @field_validator("expires_at")
    @classmethod
    def validate_expiry(cls, value: datetime | None) -> datetime | None:
        if value is None:
            return None
        value = _utc(value, "expires_at")
        if value <= datetime.now(UTC):
            raise ValueError("expires_at must be in the future")
        return value


class CapabilityAssignmentRevoke(BaseModel):
    reason: str = Field(min_length=3, max_length=500)

    @field_validator("reason")
    @classmethod
    def normalize_reason(cls, value: str) -> str:
        return value.strip()


class CapabilityAssignmentResponse(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: UUID
    user_id: UUID
    capability: MarketSupportCapability
    reason: str
    granted_by_user_id: UUID
    granted_at: datetime
    expires_at: datetime | None
    revoked_at: datetime | None
    revoked_by_user_id: UUID | None
    revocation_reason: str | None


class AuthorizationCreate(BaseModel):
    accountable_user_id: UUID
    order: OrderCreate
    authorization_expires_at: datetime
    evidence_reference: str = Field(min_length=3, max_length=500)
    evidence_sha256: str = Field(min_length=64, max_length=64)
    commercial_consent_version: str = Field(min_length=1, max_length=64)
    commercial_consent_reference: str = Field(min_length=3, max_length=500)
    support_case_reference: str | None = Field(default=None, max_length=200)

    @field_validator(
        "evidence_reference",
        "commercial_consent_version",
        "commercial_consent_reference",
        "support_case_reference",
        mode="before",
    )
    @classmethod
    def normalize_text(cls, value):
        if value is None:
            return None
        normalized = str(value).strip()
        return normalized or None

    @field_validator("evidence_sha256")
    @classmethod
    def validate_evidence_digest(cls, value: str) -> str:
        normalized = value.strip().lower()
        if not _SHA256.fullmatch(normalized):
            raise ValueError("evidence_sha256 must be a lowercase SHA-256 digest")
        return normalized

    @field_validator("authorization_expires_at")
    @classmethod
    def validate_authorization_expiry(cls, value: datetime) -> datetime:
        return _utc(value, "authorization_expires_at")

    @model_validator(mode="after")
    def validate_phase_one_terms(self):
        if self.order.side != OrderSide.ASK:
            raise ValueError("Phase 1 market support authorizes ASK listings only")
        if self.order.expires_at is None:
            raise ValueError("Assisted listings require an explicit order expiry")
        order_expiry = _utc(self.order.expires_at, "order.expires_at")
        if self.authorization_expires_at > order_expiry:
            raise ValueError("authorization_expires_at cannot exceed order expiry")
        if self.order.port_id is not None or self.order.vessel_id is not None:
            raise ValueError("Phase 1 assisted listings use the canonical delivery point only")
        return self


class AuthorizationResponse(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: UUID
    organization_id: UUID
    accountable_user_id: UUID
    status: MarketSupportAuthorizationStatus
    order: OrderCreate
    product_id: UUID
    delivery_point_id: UUID
    availability_window: str
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    authorization_expires_at: datetime
    order_expires_at: datetime
    terms_digest: str
    evidence_reference: str
    evidence_sha256: str
    commercial_consent_version: str
    commercial_consent_reference: str
    support_case_reference: str | None
    instruction_at: datetime | None
    acknowledge_exact_terms: bool | None
    acknowledge_executable_standing_order: bool | None
    created_by_actor_user_id: UUID
    created_at: datetime
    consumed_at: datetime | None
    revoked_at: datetime | None
    revoked_by_actor_user_id: UUID | None
    revocation_reason: str | None


class AuthorizationRevoke(BaseModel):
    reason: str = Field(min_length=3, max_length=500)

    @field_validator("reason")
    @classmethod
    def normalize_reason(cls, value: str) -> str:
        return value.strip()


class AssistedListingCreate(BaseModel):
    authorization_id: UUID
    acknowledge_executable_standing_order: bool

    @model_validator(mode="after")
    def require_acknowledgement(self):
        if not self.acknowledge_executable_standing_order:
            raise ValueError("Executable standing-order acknowledgement is required")
        return self


class AssistedListingCancel(BaseModel):
    reason: str = Field(min_length=3, max_length=500)

    @field_validator("reason")
    @classmethod
    def normalize_reason(cls, value: str) -> str:
        return value.strip()


class AssistedListingResponse(BaseModel):
    order: OrderResponse
    accountable_user_id: UUID
    created_by_actor_user_id: UUID
    creation_method: OrderCreationMethod
    support_authorization_id: UUID
    version: int
    etag: str


class MarketSupportContextCreate(BaseModel):
    organization_id: UUID
    accountable_user_id: UUID
    support_reference: str = Field(min_length=3, max_length=200)
    confirm_replacement: bool = False

    @field_validator("support_reference")
    @classmethod
    def normalize_support_reference(cls, value: str) -> str:
        normalized = value.strip()
        if not normalized:
            raise ValueError("support_reference is required")
        return normalized


class MarketSupportPrincipal(BaseModel):
    id: UUID
    email: str
    name: str


class MarketSupportOrganization(BaseModel):
    id: UUID
    name: str
    domain: str | None
    type: str


class MarketSupportContextResponse(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    id: UUID
    actor_user_id: UUID
    organization_id: UUID
    accountable_user_id: UUID
    organization: MarketSupportOrganization
    accountable_principal: MarketSupportPrincipal
    actor: MarketSupportPrincipal
    support_reference: str
    scope: MarketSupportContextScope
    started_at: datetime
    expires_at: datetime
    ended_at: datetime | None
    status: MarketSupportContextStatus
    version: int


class MarketSupportEntryResponse(BaseModel):
    organization: MarketSupportOrganization
    eligible_principals: list[MarketSupportPrincipal]


class MarketSupportContext(BaseModel):
    organization: MarketSupportOrganization
    eligible_principals: list[MarketSupportPrincipal]
    authorizations: list[AuthorizationResponse]
    listings: list[AssistedListingResponse]


AuthorizationPage = PaginatedResponse[AuthorizationResponse]
AssistedListingPage = PaginatedResponse[AssistedListingResponse]
OrganizationPage = PaginatedResponse[MarketSupportOrganization]
