"""
Unit tests for dashboard CRUD router (S8-001/S8-002/S8-004).
Uses mock DB sessions — no live database required.
"""
import pytest
from unittest.mock import MagicMock, AsyncMock
from uuid import uuid4
from datetime import datetime, UTC

from app.models.user import UserRole, UserStatus
from app.routers.user_dashboard import (
    create_dashboard,
    list_dashboards,
    get_dashboard,
    update_dashboard,
    delete_dashboard,
    add_widget,
    update_widget,
    delete_widget,
)
from app.schemas.dashboard import (
    DashboardCreate,
    DashboardUpdate,
    WidgetCreate,
    WidgetUpdate,
)


def _make_user(user_id=None):
    u = MagicMock()
    u.id = user_id or uuid4()
    u.email = "test@verdaxis.com"
    u.role = UserRole.BUYER
    u.status = UserStatus.APPROVED
    u.organization_id = uuid4()
    return u


def _make_dashboard(user_id=None, dashboard_id=None):
    d = MagicMock()
    d.id = dashboard_id or uuid4()
    d.user_id = user_id or uuid4()
    d.name = "Test Dashboard"
    d.layout = {}
    d.is_default = False
    d.created_at = datetime.now(UTC)
    d.updated_at = datetime.now(UTC)
    d.widgets = []
    return d


def _make_widget(dashboard_id=None, widget_id=None):
    w = MagicMock()
    w.id = widget_id or uuid4()
    w.dashboard_id = dashboard_id or uuid4()
    w.widget_type = "orderbook"
    w.config = {}
    w.position = {}
    w.created_at = datetime.now(UTC)
    return w


class TestCreateDashboard:
    @pytest.mark.asyncio
    async def test_create_dashboard(self):
        user = _make_user()
        mock_db = AsyncMock()

        dashboard = _make_dashboard(user_id=user.id)

        # Re-fetch after commit returns dashboard
        mock_result = MagicMock()
        mock_result.scalar_one.return_value = dashboard
        mock_db.execute.return_value = mock_result

        body = DashboardCreate(name="My Dashboard")
        result = await create_dashboard(body=body, current_user=user, db=mock_db)

        mock_db.add.assert_called()
        mock_db.commit.assert_awaited_once()
        assert result.name == "Test Dashboard"


class TestListDashboards:
    @pytest.mark.asyncio
    async def test_list_dashboards_own_only(self):
        user = _make_user()
        mock_db = AsyncMock()

        own_dashboard = _make_dashboard(user_id=user.id)
        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [own_dashboard]
        mock_db.execute.return_value = mock_result

        results = await list_dashboards(current_user=user, db=mock_db)

        assert len(results) == 1
        assert results[0].user_id == user.id
        mock_db.execute.assert_called_once()


class TestGetDashboard:
    @pytest.mark.asyncio
    async def test_get_dashboard_with_widgets(self):
        user = _make_user()
        mock_db = AsyncMock()

        widget = _make_widget()
        dashboard = _make_dashboard(user_id=user.id)
        dashboard.widgets = [widget]

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = dashboard
        mock_db.execute.return_value = mock_result

        result = await get_dashboard(
            dashboard_id=dashboard.id, current_user=user, db=mock_db
        )

        assert result.id == dashboard.id
        assert len(result.widgets) == 1

    @pytest.mark.asyncio
    async def test_other_user_gets_404(self):
        """Other users' dashboards return 404, not 403."""
        from fastapi import HTTPException

        user = _make_user()
        mock_db = AsyncMock()

        # No row returned — the user_id filter excludes it
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_db.execute.return_value = mock_result

        other_dashboard_id = uuid4()
        with pytest.raises(HTTPException) as exc_info:
            await get_dashboard(
                dashboard_id=other_dashboard_id, current_user=user, db=mock_db
            )

        assert exc_info.value.status_code == 404


class TestUpdateDashboard:
    @pytest.mark.asyncio
    async def test_update_layout(self):
        user = _make_user()
        mock_db = AsyncMock()
        dashboard = _make_dashboard(user_id=user.id)

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = dashboard
        mock_result.scalar_one.return_value = dashboard
        mock_db.execute.return_value = mock_result

        new_layout = {"cols": 12, "rows": 8}
        body = DashboardUpdate(layout=new_layout)
        await update_dashboard(
            dashboard_id=dashboard.id, body=body, current_user=user, db=mock_db
        )

        mock_db.commit.assert_awaited_once()
        # Layout was assigned on the mock object
        assert dashboard.layout == new_layout


class TestDeleteDashboard:
    @pytest.mark.asyncio
    async def test_delete_dashboard_cascades_widgets(self):
        """Deleting a dashboard passes the whole object to db.delete (cascade handled by ORM)."""
        user = _make_user()
        mock_db = AsyncMock()
        widget = _make_widget()
        dashboard = _make_dashboard(user_id=user.id)
        dashboard.widgets = [widget]

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = dashboard
        mock_db.execute.return_value = mock_result

        await delete_dashboard(
            dashboard_id=dashboard.id, current_user=user, db=mock_db
        )

        mock_db.delete.assert_awaited_once_with(dashboard)
        mock_db.commit.assert_awaited_once()


class TestWidgets:
    @pytest.mark.asyncio
    async def test_add_widget(self):
        user = _make_user()
        mock_db = AsyncMock()
        dashboard = _make_dashboard(user_id=user.id)

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = dashboard
        mock_db.execute.return_value = mock_result

        body = WidgetCreate(
            widget_type="compliance_score",
            config={"limit": 5},
            position={"x": 0, "y": 0, "w": 4, "h": 3},
        )
        await add_widget(
            dashboard_id=dashboard.id, body=body, current_user=user, db=mock_db
        )

        mock_db.add.assert_called()
        mock_db.commit.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_update_widget_position(self):
        user = _make_user()
        mock_db = AsyncMock()
        dashboard = _make_dashboard(user_id=user.id)
        widget = _make_widget(dashboard_id=dashboard.id)

        # First execute: dashboard lookup; second: widget lookup
        dashboard_result = MagicMock()
        dashboard_result.scalar_one_or_none.return_value = dashboard
        widget_result = MagicMock()
        widget_result.scalar_one_or_none.return_value = widget
        mock_db.execute.side_effect = [dashboard_result, widget_result]

        new_pos = {"x": 3, "y": 1, "w": 6, "h": 4}
        body = WidgetUpdate(position=new_pos)
        await update_widget(
            dashboard_id=dashboard.id,
            widget_id=widget.id,
            body=body,
            current_user=user,
            db=mock_db,
        )

        mock_db.commit.assert_awaited_once()
        assert widget.position == new_pos

    @pytest.mark.asyncio
    async def test_remove_widget(self):
        user = _make_user()
        mock_db = AsyncMock()
        dashboard = _make_dashboard(user_id=user.id)
        widget = _make_widget(dashboard_id=dashboard.id)

        dashboard_result = MagicMock()
        dashboard_result.scalar_one_or_none.return_value = dashboard
        widget_result = MagicMock()
        widget_result.scalar_one_or_none.return_value = widget
        mock_db.execute.side_effect = [dashboard_result, widget_result]

        await delete_widget(
            dashboard_id=dashboard.id,
            widget_id=widget.id,
            current_user=user,
            db=mock_db,
        )

        mock_db.delete.assert_awaited_once_with(widget)
        mock_db.commit.assert_awaited_once()
