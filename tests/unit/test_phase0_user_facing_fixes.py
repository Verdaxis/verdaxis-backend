"""P0-2: missing references and historical stored emails are safe responses."""
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError

from app.schemas.user import UserCreate, UserResponse


def test_profile_preserves_stored_email_without_input_validation():
    email = "Historical@disposable.invalid"
    profile = UserResponse.model_validate({
        "id": uuid4(), "email": email, "role": "BUYER", "status": "APPROVED",
    })
    assert profile.email == email
    with pytest.raises(ValidationError):
        UserCreate(email=email, role="BUYER", password="synthetic-password")


@pytest.mark.asyncio
async def test_inventory_missing_port_returns_404_before_writes(monkeypatch):
    from app.routers import inventory
    from app.schemas.marketplace import InventoryCreate

    db = AsyncMock()
    db.scalar.return_value = None
    db.add = Mock()
    monkeypatch.setattr(inventory, "lock_and_load_market_organizations", AsyncMock())
    user = SimpleNamespace(role=inventory.UserRole.SUPPLIER, id=uuid4(), organization_id=uuid4())
    item = InventoryCreate(port_id="missing", fuel_type="Ethanol", current_stock_mt=1000)
    with pytest.raises(HTTPException) as error:
        await inventory.add_inventory(item, db, user)
    assert error.value.status_code == 404
    assert error.value.detail == "Port not found"
    db.add.assert_not_called()
    db.flush.assert_not_awaited()
    db.commit.assert_not_awaited()
