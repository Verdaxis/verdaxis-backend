import pytest
from unittest.mock import MagicMock, AsyncMock
from app.routers.notifications import get_notifications, get_unread_count, mark_as_read
from app.models.notification import Notification
from uuid import uuid4
from datetime import datetime

class TestNotificationRouter:
    @pytest.mark.asyncio
    async def test_get_notifications(self):
        # Mock Session
        mock_db = AsyncMock()
        mock_user = MagicMock(id=uuid4())
        
        # Mock result execution
        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [
            Notification(id=uuid4(), recipient_id=mock_user.id, title="Test", created_at=datetime.now(), is_read=False)
        ]
        mock_db.execute.return_value = mock_result
        
        notifications = await get_notifications(skip=0, limit=10, current_user=mock_user, db=mock_db)
        
        assert len(notifications) == 1
        assert notifications[0].title == "Test"
        mock_db.execute.assert_called_once()

    @pytest.mark.asyncio
    async def test_get_unread_count(self):
        mock_db = AsyncMock()
        mock_user = MagicMock(id=uuid4())
        
        mock_result = MagicMock()
        mock_result.scalar.return_value = 5
        mock_db.execute.return_value = mock_result
        
        response = await get_unread_count(current_user=mock_user, db=mock_db)
        
        assert response["count"] == 5

    @pytest.mark.asyncio
    async def test_mark_as_read(self):
        mock_db = AsyncMock()
        mock_user = MagicMock(id=uuid4())
        notif_id = uuid4()
        
        mock_notif = Notification(id=notif_id, recipient_id=mock_user.id, is_read=False)
        
        mock_result = MagicMock()
        mock_result.scalars.return_value.first.return_value = mock_notif
        mock_db.execute.return_value = mock_result
        
        response = await mark_as_read(notification_id=notif_id, current_user=mock_user, db=mock_db)
        
        assert response["status"] == "success"
        assert mock_notif.is_read == True
        mock_db.commit.assert_awaited_once()
