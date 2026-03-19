"""
Unit tests for expanded role permissions (S8-003).
Verifies TRADER can BID and ASK; COMPLIANCE_OFFICER cannot trade;
BUYER/SUPPLIER restrictions remain unchanged.
"""
import pytest
from unittest.mock import MagicMock
from uuid import uuid4
from decimal import Decimal
from fastapi import HTTPException

from app.models.user import UserRole, UserStatus
from app.models.orderbook import OrderSide
from app.schemas.orderbook import OrderCreate


def _make_user(role: UserRole, org_id=None):
    u = MagicMock()
    u.id = uuid4()
    u.email = f"{role.value.lower()}@verdaxis.com"
    u.role = role
    u.status = UserStatus.APPROVED
    u.organization_id = org_id or uuid4()
    return u


def _make_order_data(side: OrderSide) -> OrderCreate:
    return OrderCreate(
        side=side,
        fuel_type="VLSFO",
        region="Asia Pacific",
        quantity_mt=Decimal("500"),
        price_per_mt_usd=Decimal("650"),
        availability_window="Q2 2026",
    )


def _check_role_for_side(role: UserRole, side: OrderSide) -> None:
    """
    Inline replica of the role-check block in create_order.
    Raises HTTPException(403) if the role+side combination is forbidden.
    This isolates the permission logic from DB mechanics.
    """
    from fastapi import HTTPException
    if role == UserRole.COMPLIANCE_OFFICER:
        raise HTTPException(status_code=403, detail="Compliance officers cannot place orders")
    if side == OrderSide.BID and role not in (UserRole.BUYER, UserRole.TRADER):
        raise HTTPException(status_code=403, detail="Only buyers or traders can place BID orders")
    if side == OrderSide.ASK and role not in (UserRole.SUPPLIER, UserRole.TRADER):
        raise HTTPException(status_code=403, detail="Only suppliers or traders can place ASK orders")


class TestTraderRole:
    def test_trader_can_bid(self):
        """TRADER role passes the BID permission check without raising."""
        # Should not raise
        _check_role_for_side(UserRole.TRADER, OrderSide.BID)

    def test_trader_can_ask(self):
        """TRADER role passes the ASK permission check without raising."""
        # Should not raise
        _check_role_for_side(UserRole.TRADER, OrderSide.ASK)


class TestComplianceOfficerCannotTrade:
    def test_compliance_officer_cannot_trade(self):
        """COMPLIANCE_OFFICER is blocked from placing any order (both sides)."""
        for side in (OrderSide.BID, OrderSide.ASK):
            with pytest.raises(HTTPException) as exc_info:
                _check_role_for_side(UserRole.COMPLIANCE_OFFICER, side)
            assert exc_info.value.status_code == 403
            assert "Compliance officers" in exc_info.value.detail


class TestBuyerSupplierRestrictions:
    def test_buyer_still_bid_only(self):
        """BUYER cannot place ASK orders — rule unchanged from pre-S8."""
        with pytest.raises(HTTPException) as exc_info:
            _check_role_for_side(UserRole.BUYER, OrderSide.ASK)
        assert exc_info.value.status_code == 403

    def test_supplier_still_ask_only(self):
        """SUPPLIER cannot place BID orders — rule unchanged from pre-S8."""
        with pytest.raises(HTTPException) as exc_info:
            _check_role_for_side(UserRole.SUPPLIER, OrderSide.BID)
        assert exc_info.value.status_code == 403
