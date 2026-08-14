"""News refresh has one locked process entrypoint and no public mutation route."""

from contextlib import asynccontextmanager
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest


ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.asyncio
async def test_refresh_news_cli_opens_one_database_session_and_commits():
    from app.cli import refresh_news

    session = AsyncMock()

    @asynccontextmanager
    async def session_context():
        yield session

    refresh = AsyncMock(return_value=3)
    with patch.object(refresh_news, "AsyncSessionLocal", session_context), patch.object(
        refresh_news, "refresh_news", refresh
    ):
        assert await refresh_news.run_once() == 3

    refresh.assert_awaited_once_with(session)
    session.commit.assert_awaited_once()


def test_news_router_is_read_only_and_workers_have_no_scheduler():
    router_source = (ROOT / "app" / "routers" / "news.py").read_text()
    main_source = (ROOT / "app" / "main.py").read_text()

    assert '@router.post("/refresh")' not in router_source
    assert "from app.services.news_feed import refresh_news" not in router_source
    assert "refresh_news" not in main_source
    assert "asyncio.create_task" not in main_source


def test_exactly_one_news_timer_owner_exists_for_each_environment():
    unit_dir = ROOT / "deploy" / "systemd"
    expected = {
        "production": (
            "verdaxis-news-refresh.service",
            "verdaxis-news-refresh.timer",
            "/home/verdaxis-prod/verdaxis/prod/be",
        ),
        "staging": (
            "verdaxis-news-refresh-staging.service",
            "verdaxis-news-refresh-staging.timer",
            "/home/verdaxis-prod/verdaxis/staging/be",
        ),
    }
    assert {path.name for path in unit_dir.glob("verdaxis-news-refresh*.timer")} == {
        values[1] for values in expected.values()
    }
    assert {path.name for path in unit_dir.glob("verdaxis-news-refresh*.service")} == {
        values[0] for values in expected.values()
    }

    for service_name, timer_name, deploy_root in expected.values():
        service = (unit_dir / service_name).read_text()
        timer = (unit_dir / timer_name).read_text()
        assert "Type=oneshot" in service
        assert f"WorkingDirectory={deploy_root}" in service
        assert "python -m app.cli.refresh_news" in service
        assert f"Unit={service_name}" in timer
        assert "OnCalendar=*-*-* 00,06,12,18:00:00" in timer


def test_refresh_cli_is_the_documented_process_entrypoint():
    doc = (ROOT / "docs" / "news-refresh-timer.md").read_text()

    assert "python -m app.cli.refresh_news" in doc
    assert "POST /api/news/refresh" in doc
    assert "removed" in doc.lower() or "does not exist" in doc.lower()
