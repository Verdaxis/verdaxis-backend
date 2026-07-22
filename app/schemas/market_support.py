"""API contracts for the non-impersonating market-support workspace."""
from __future__ import annotations

import re
from datetime import UTC, datetime
from decimal import Decimal
from uuid import UUID

from pydantic import BaseModel, Field, field_validator, model_validator

from app.models.market_support import (
    AdminCapability,
    SupportAuthorizationScope,
    SupportAuthorizationStatus,
    SupportManagementAuthority,
)
from app.schemas.orderbook import OrderCreate, OrderResponse, OrderUpdate


_REASON_PATTERN = re.compile(r"^[A-Z0-9_]{3,64}$")


def _aware_utc(value: datetime, *, field_name: str) -> datetime:
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError(f"{field_name} must include a timezone")
    return value.astimezone(UTC)


class CapabilityGrantCreate(BaseModel):
    user_id: UUID
    capability: AdminCapability
    reason: str = Field(min_length=3, max_length=500)
    expires_at: datetime | None = None

    @field_validator("reason")
    @classmethod
    def _normalize_reason(cls, value: str) -> str:
        return value.strip()

    @field_validator("expires_at")
    @classmethod
    def _validate_expiry(cls, value: datetime | None) -> datetime | None:
        if value is None:
            return None
        normalized = _aware_utc(value, field_name="expires_at")
        if normalized <= datetime.now(UTC):
            raise ValueError("expires_at must be in the future")
        return normalized


class CapabilityGrantResponse(BaseModel):
    id: UUID
    user_id: UUID
    capability: AdminCapability
    reason: str
    granted_by_user_id: UUID | None
    granted_at: datetime
    expires_at: datetime | None
    revoked_at: datetime | None

    class Config:
        from_attributes = True


class CapabilityRevokeRequest(BaseModel):
    reason: str = Field(min_length=3, max_length=500)

    @field_validator("reason")
    @classmethod
    def _normalize_reason(cls, value: str) -> str:
        return value.strip()


class SupportAuthorizationCreate(BaseModel):
    authorized_contact_name: str = Field(min_length=2, max_length=200)
    authorized_contact_email: str = Field(min_length=3, max_length=320)
    evidence_reference: str = Field(min_length=3, max_length=500)
    support_case_reference: str | None = Field(default=None, max_length=200)
    scopes: list[SupportAuthorizationScope] = Field(min_length=1)
    valid_from: datetime
    valid_until: datetime
    product_id: UUID | None = None
    delivery_point_id: UUID | None = None
    min_price_per_mt_usd: Decimal | None = Field(default=None, gt=0, max_digits=12, decimal_places=2)
    max_price_per_mt_usd: Decimal | None = Field(default=None, gt=0, max_digits=12, decimal_places=2)
    max_quantity_mt_per_order: Decimal = Field(gt=0, le=100000, max_digits=14, decimal_places=2)
    max_total_open_quantity_mt: Decimal = Field(gt=0, le=1000000, max_digits=14, decimal_places=2)
    max_order_ttl_hours: int = Field(default=168, ge=1, le=720)
    max_uses: int | None = Field(default=None, ge=1, le=10000)

    @field_validator(
        "authorized_contact_name",
        "evidence_reference",
        "support_case_reference",
        mode="before",
    )
    @classmethod
    def _strip_optional_text(cls, value):
        if value is None:
            return None
        normalized = str(value).strip()
        return normalized or None

    @field_validator("authorized_contact_email")
    @classmethod
    def _normalize_contact_email(cls, value: str) -> str:
        normalized = value.strip().lower()
        local, separator, domain = normalized.partition("@")
        if not separator or not local or not domain or "." not in domain:
            raise ValueError("authorized_contact_email must be a valid email address")
        return normalized

    @field_validator("valid_from", "valid_until")
    @classmethod
    def _validate_datetimes(cls, value: datetime, info) -> datetime:
        return _aware_utc(value, field_name=info.field_name)

    @field_validator("scopes")
    @classmethod
    def _deduplicate_scopes(
        cls, values: list[SupportAuthorizationScope]
    ) -> list[SupportAuthorizationScope]:
        return list(dict.fromkeys(values))

    @model_validator(mode="after")
    def _validate_bounds(self):
        now = datetime.now(UTC)
        if self.valid_until <= self.valid_from:
            raise ValueError("valid_until must be after valid_from")
        if self.valid_until <= now:
            raise ValueError("valid_until must be in the future")
        if (
            self.min_price_per_mt_usd is not None
            and self.max_price_per_mt_usd is not None
            and self.min_price_per_mt_usd > self.max_price_per_mt_usd
        ):
            raise ValueError("min_price_per_mt_usd cannot exceed max_price_per_mt_usd")
        if self.max_total_open_quantity_mt < self.max_quantity_mt_per_order:
            raise ValueError(
                "max_total_open_quantity_mt must be at least max_quantity_mt_per_order"
            )
        return self


class SupportAuthorizationResponse(BaseModel):
    id: UUID
    organization_id: UUID
    authorized_contact_name: str
    authorized_contact_email: str
    evidence_reference: str
    support_case_reference: str | None
    scopes: list[SupportAuthorizationScope]
    valid_from: datetime
    valid_until: datetime
    status: SupportAuthorizationStatus
    allowed_side: str
    product_id: UUID | None
    delivery_point_id: UUID | None
    min_price_per_mt_usd: Decimal | None
    max_price_per_mt_usd: Decimal | None
    max_quantity_mt_per_order: Decimal
    max_total_open_quantity_mt: Decimal
    max_order_ttl_hours: int
    max_uses: int | None
    uses_count: int
    created_by_admin_user_id: UUID
    created_at: datetime
    revoked_at: datetime | None
    revoked_by_admin_user_id: UUID | None
    revocation_reason: str | None
    usable_by_current_admin: bool = False

    class Config:
        from_attributes = True


class SupportAuthorizationRevokeRequest(BaseModel):
    reason: str = Field(min_length=3, max_length=500)

    @field_validator("reason")
    @classmethod
    def _normalize_reason(cls, value: str) -> str:
        return value.strip()


class MarketSupportPrincipalResponse(BaseModel):
    id: UUID
    email: str
    first_name: str | None
    last_name: str | None
    role: str
    eligible: bool


class MarketSupportOrganizationResponse(BaseModel):
    id: UUID
    name: str
    domain: str | None
    type: str
    verification_status: str
    provenance: str


class AssistedOrderActionMixin(BaseModel):
    accountable_user_id: UUID
    support_authorization_id: UUID
    reason_code: str = Field(min_length=3, max_length=64)
    support_case_reference: str | None = Field(default=None, max_length=200)

    @field_validator("reason_code")
    @classmethod
    def _normalize_reason_code(cls, value: str) -> str:
        normalized = value.strip().upper()
        if not _REASON_PATTERN.fullmatch(normalized):
            raise ValueError("reason_code must contain only A-Z, 0-9, and underscore")
        return normalized

    @field_validator("support_case_reference")
    @classmethod
    def _normalize_case_reference(cls, value: str | None) -> str | None:
        if value is None:
            return None
        normalized = value.strip()
        return normalized or None


class AssistedOrderPreviewRequest(AssistedOrderActionMixin):
    order: OrderCreate


class AssistedOrderCreateRequest(AssistedOrderActionMixin):
    order: OrderCreate
    acknowledge_executable_resting_order: bool

    @model_validator(mode="after")
    def _require_acknowledgement(self):
        if not self.acknowledge_executable_resting_order:
            raise ValueError(
                "acknowledge_executable_resting_order must be true before publication"
            )
        return self


class AssistedOrderUpdateRequest(BaseModel):
    accountable_user_id: UUID
    support_authorization_id: UUID
    expected_support_version: int = Field(ge=1)
    expected_order_updated_at: datetime
    changes: OrderUpdate
    reason_code: str = Field(min_length=3, max_length=64)
    support_case_reference: str | None = Field(default=None, max_length=200)
    acknowledge_executable_resting_order: bool

    @field_validator("expected_order_updated_at")
    @classmethod
    def _validate_expected_update(cls, value: datetime) -> datetime:
        return _aware_utc(value, field_name="expected_order_updated_at")

    @field_validator("reason_code")
    @classmethod
    def _normalize_reason_code(cls, value: str) -> str:
        normalized = value.strip().upper()
        if not _REASON_PATTERN.fullmatch(normalized):
            raise ValueError("reason_code must contain only A-Z, 0-9, and underscore")
        return normalized

    @field_validator("support_case_reference")
    @classmethod
    def _normalize_case_reference(cls, value: str | None) -> str | None:
        if value is None:
            return None
        normalized = value.strip()
        return normalized or None

    @model_validator(mode="after")
    def _validate_action(self):
        if not self.acknowledge_executable_resting_order:
            raise ValueError(
                "acknowledge_executable_resting_order must be true before publication"
            )
        if not self.changes.model_fields_set:
            raise ValueError("changes must include at least one field")
        return self


class AssistedOrderCancelRequest(BaseModel):
    accountable_user_id: UUID
    support_authorization_id: UUID
    expected_support_version: int = Field(ge=1)
    expected_order_updated_at: datetime
    reason_code: str = Field(min_length=3, max_length=64)
    support_case_reference: str | None = Field(default=None, max_length=200)

    @field_validator("expected_order_updated_at")
    @classmethod
    def _validate_expected_update(cls, value: datetime) -> datetime:
        return _aware_utc(value, field_name="expected_order_updated_at")

    @field_validator("reason_code")
    @classmethod
    def _normalize_reason_code(cls, value: str) -> str:
        normalized = value.strip().upper()
        if not _REASON_PATTERN.fullmatch(normalized):
            raise ValueError("reason_code must contain only A-Z, 0-9, and underscore")
        return normalized

    @field_validator("support_case_reference")
    @classmethod
    def _normalize_case_reference(cls, value: str | None) -> str | None:
        if value is None:
            return None
        normalized = value.strip()
        return normalized or None


class PostOnlyPreviewResponse(BaseModel):
    valid: bool
    would_cross: bool
    indeterminate: bool = False
    best_executable_opposing_price_per_mt_usd: Decimal | None = None
    similar_open_order_count: int = 0
    warnings: list[str] = Field(default_factory=list)


class AssistedOrderAttributionResponse(BaseModel):
    organization_id: UUID
    accountable_user_id: UUID
    created_by_admin_user_id: UUID
    support_authorization_id: UUID
    submission_method: str
    support_version: int
    management_authority: SupportManagementAuthority
    customer_adopted_at: datetime | None
    customer_adopted_by_user_id: UUID | None
    last_action_actor_user_id: UUID
    last_action_reason_code: str
    support_case_reference: str | None
    created_at: datetime
    updated_at: datetime

    class Config:
        from_attributes = True


class AssistedOrderResponse(BaseModel):
    order: OrderResponse
    attribution: AssistedOrderAttributionResponse
    order_updated_at: datetime


class MarketSupportOrderView(BaseModel):
    order: OrderResponse
    attribution: AssistedOrderAttributionResponse | None = None
    order_updated_at: datetime
    support_manageable: bool = False


class MarketSupportContextResponse(BaseModel):
    organization: MarketSupportOrganizationResponse
    capabilities: list[AdminCapability]
    eligible_principals: list[MarketSupportPrincipalResponse]
    active_authorizations: list[SupportAuthorizationResponse]
    orders: list[MarketSupportOrderView]


class CustomerAssistedOrderMetadata(BaseModel):
    order_id: UUID
    submission_method: str
    set_up_by: str = "Verdaxis Support"
    created_at: datetime
    support_version: int
    management_authority: SupportManagementAuthority
    customer_adopted_at: datetime | None
    last_action_at: datetime
