import pytest
from app.core.auth import get_current_user
from app.config import settings
from unittest.mock import MagicMock, AsyncMock

@pytest.mark.asyncio
async def test_auth_bypass_enabled():
    # Mock settings
    original_bypass = settings.ENABLE_AUTH_BYPASS
    settings.ENABLE_AUTH_BYPASS = True
    
    try:
        # Mock DB session
        mock_db = AsyncMock()
        # Mock result for existing user check (return None to trigger creation, or mock user)
        # We'll return None first to test the creation path, or just a mock user to test return
        
        # Let's mock a found user to keep it simple and avoid testing DB logic deeply
        mock_user = MagicMock()
        mock_user.email = "dev@admin.com"
        
        # scalars().one_or_none() style mock
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = mock_user
        mock_db.execute.return_value = mock_result
        
        # Call get_current_user without token
        user = await get_current_user(token="any_token", db=mock_db)
        
        assert user.email == "dev@admin.com"
        
    finally:
        settings.ENABLE_AUTH_BYPASS = original_bypass

@pytest.mark.asyncio
async def test_auth_bypass_disabled():
    # Mock settings
    original_bypass = settings.ENABLE_AUTH_BYPASS
    settings.ENABLE_AUTH_BYPASS = False
    
    try:
        # Mock DB and verify_token
        mock_db = AsyncMock()
        
        # We expect it to try verifying token if bypass is False
        # Since we don't mock verify_token here, it might fail or we mock it
        # But for this test, we just want to ensure it DOESN'T return the dev user immediately
        # checking that it proceeds to token verification (which will fail with invalid token)
        
        with pytest.raises(Exception): # Likely HTTP 401 or similar from verify_token
             await get_current_user(token="invalid_token", db=mock_db)
             
    finally:
        settings.ENABLE_AUTH_BYPASS = original_bypass
