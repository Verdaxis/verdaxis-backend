"""Pydantic schemas for Negotiation endpoints."""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, model_validator


class NegotiationCreateRequest(BaseModel):
    bid_order_id: Optional[UUID] = None
    ask_order_id: Optional[UUID] = None
    counterparty_org_id: UUID
    product_id: UUID
    quantity_mt: Decimal = Field(gt=0, le=100000, decimal_places=2, max_digits=12)
    proposed_price: Decimal = Field(gt=0, decimal_places=2, max_digits=10)
    notes: Optional[str] = Field(None, max_length=500)
    expires_in_hours: int = Field(default=1, ge=1, le=72)

    @model_validator(mode='after')
    def require_at_least_one_order(self) -> 'NegotiationCreateRequest':
        if self.bid_order_id is None and self.ask_order_id is None:
            raise ValueError('At least one of bid_order_id or ask_order_id must be provided')
        return self


class NegotiationCounterRequest(BaseModel):
    proposed_price: Decimal = Field(gt=0, decimal_places=2, max_digits=10)
    notes: Optional[str] = Field(None, max_length=500)


class NegotiationRoundResponse(BaseModel):
    id: UUID
    round_number: int
    proposer_org_id: UUID
    proposer_org_name: Optional[str] = None
    proposer_user_id: Optional[UUID] = None
    proposed_price: Decimal
    notes: Optional[str] = None
    created_at: datetime

    model_config = ConfigDict(from_attributes=True)


class NegotiationResponse(BaseModel):
    id: UUID
    bid_order_id: Optional[UUID] = None
    ask_order_id: Optional[UUID] = None
    initiator_org_id: UUID
    initiator_org_name: Optional[str] = None
    counterparty_org_id: UUID
    counterparty_org_name: Optional[str] = None
    initiator_user_id: Optional[UUID] = None
    counterparty_user_id: Optional[UUID] = None
    accepted_by_user_id: Optional[UUID] = None
    initiator_side: str
    product_id: UUID
    product_name: Optional[str] = None
    quantity_mt: Decimal
    current_price: Decimal
    status: str
    last_actor_org_id: UUID
    trade_id: Optional[UUID] = None
    expires_at: datetime
    created_at: datetime
    updated_at: datetime
    rounds: list[NegotiationRoundResponse] = []

    model_config = ConfigDict(from_attributes=True)


class NegotiationListResponse(BaseModel):
    items: list[NegotiationResponse]
    total: int
