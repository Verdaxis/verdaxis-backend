"""Unit tests for watchlist CRUD endpoints — no DB required."""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4
from datetime import datetime, UTC

from fastapi import HTTPException

from app.routers.watchlists import (
    list_watchlists,
    create_watchlist,
    add_watchlist_entry,
    remove_watchlist_entry,
    delete_watchlist,
    MAX_WATCHLISTS_PER_USER,
    MAX_ENTRIES_PER_WATCHLIST,
)
from app.schemas.watchlist import WatchlistCreateRequest, WatchlistEntryAddRequest
from app.models.watchlist import Watchlist, WatchlistEntry
from app.models.user import User, UserRole, UserStatus


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_user():
    user = MagicMock(spec=User)
    user.id = uuid4()
    user.role = UserRole.BUYER
    user.status = UserStatus.APPROVED
    return user


def make_watchlist(user_id, name="My Watchlist", entries=None):
    wl = MagicMock(spec=Watchlist)
    wl.id = uuid4()
    wl.user_id = user_id
    wl.name = name
    wl.entries = entries or []
    wl.created_at = datetime.now(UTC)
    return wl


def make_entry(watchlist_id, product_id=None, dp_id=None):
    entry = MagicMock(spec=WatchlistEntry)
    entry.id = uuid4()
    entry.watchlist_id = watchlist_id
    entry.product_id = product_id or uuid4()
    entry.delivery_point_id = dp_id
    entry.created_at = datetime.now(UTC)
    return entry


# ---------------------------------------------------------------------------
# GET /watchlists — list
# ---------------------------------------------------------------------------

class TestListWatchlists:
    @pytest.mark.asyncio
    async def test_returns_empty_list(self):
        user = make_user()
        mock_db = AsyncMock()
        scalars_mock = MagicMock()
        scalars_mock.all.return_value = []
        result_mock = MagicMock()
        result_mock.scalars.return_value = scalars_mock
        mock_db.execute.return_value = result_mock

        result = await list_watchlists(current_user=user, db=mock_db)
        assert result == []

    @pytest.mark.asyncio
    async def test_returns_watchlists_with_entry_count(self):
        user = make_user()
        wl = make_watchlist(user.id, "Fuels")
        entry = make_entry(wl.id)
        wl.entries = [entry]

        mock_db = AsyncMock()
        # First execute: watchlists query
        scalars_mock = MagicMock()
        scalars_mock.all.return_value = [wl]
        result_mock = MagicMock()
        result_mock.scalars.return_value = scalars_mock

        # Product name lookup
        prod_result = MagicMock()
        prod_result.all.return_value = [(entry.product_id, "VLSFO")]

        mock_db.execute.side_effect = [result_mock, prod_result]

        result = await list_watchlists(current_user=user, db=mock_db)
        assert len(result) == 1
        assert result[0].name == "Fuels"
        assert result[0].entry_count == 1


# ---------------------------------------------------------------------------
# POST /watchlists — create
# ---------------------------------------------------------------------------

class TestCreateWatchlist:
    @pytest.mark.asyncio
    async def test_create_success(self):
        user = make_user()
        mock_db = AsyncMock()

        count_result = MagicMock()
        count_result.scalar_one.return_value = 0
        mock_db.execute.return_value = count_result

        body = WatchlistCreateRequest(name="My Fuels")

        with patch("app.routers.watchlists.Watchlist") as MockWL:
            instance = MagicMock()
            instance.id = uuid4()
            instance.name = "My Fuels"
            instance.created_at = datetime.now(UTC)
            MockWL.return_value = instance

            result = await create_watchlist(body=body, current_user=user, db=mock_db)
            assert result.name == "My Fuels"
            assert result.entry_count == 0
            mock_db.add.assert_called_once()
            mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_create_exceeds_limit(self):
        user = make_user()
        mock_db = AsyncMock()

        count_result = MagicMock()
        count_result.scalar_one.return_value = MAX_WATCHLISTS_PER_USER
        mock_db.execute.return_value = count_result

        body = WatchlistCreateRequest(name="Overflow")

        with pytest.raises(HTTPException) as exc_info:
            await create_watchlist(body=body, current_user=user, db=mock_db)
        assert exc_info.value.status_code == 400
        assert "Maximum" in exc_info.value.detail


# ---------------------------------------------------------------------------
# POST /watchlists/{id}/entries — add entry
# ---------------------------------------------------------------------------

class TestAddWatchlistEntry:
    @pytest.mark.asyncio
    async def test_add_entry_watchlist_not_found(self):
        user = make_user()
        mock_db = AsyncMock()
        result_mock = MagicMock()
        result_mock.scalar_one_or_none.return_value = None
        mock_db.execute.return_value = result_mock

        body = WatchlistEntryAddRequest(product_id=uuid4())

        with pytest.raises(HTTPException) as exc_info:
            await add_watchlist_entry(
                watchlist_id=uuid4(), body=body, current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 404

    @pytest.mark.asyncio
    async def test_add_entry_exceeds_limit(self):
        user = make_user()
        wl = make_watchlist(user.id)

        mock_db = AsyncMock()
        # 1st execute: find watchlist
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl
        prod_result = MagicMock()
        prod_result.scalar_one_or_none.return_value = MagicMock(name="Product")
        existing_result = MagicMock()
        existing_result.scalar_one_or_none.return_value = None
        # 4th execute: count entries
        count_result = MagicMock()
        count_result.scalar_one.return_value = MAX_ENTRIES_PER_WATCHLIST

        mock_db.execute.side_effect = [wl_result, prod_result, existing_result, count_result]

        body = WatchlistEntryAddRequest(product_id=uuid4())

        with pytest.raises(HTTPException) as exc_info:
            await add_watchlist_entry(
                watchlist_id=wl.id, body=body, current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 400
        assert "Maximum" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_add_entry_product_not_found(self):
        user = make_user()
        wl = make_watchlist(user.id)

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl
        prod_result = MagicMock()
        prod_result.scalar_one_or_none.return_value = None

        mock_db.execute.side_effect = [wl_result, prod_result]

        body = WatchlistEntryAddRequest(product_id=uuid4())

        with pytest.raises(HTTPException) as exc_info:
            await add_watchlist_entry(
                watchlist_id=wl.id, body=body, current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 404
        assert "Product" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_add_entry_success(self):
        user = make_user()
        wl = make_watchlist(user.id)
        product_id = uuid4()

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl
        count_result = MagicMock()
        count_result.scalar_one.return_value = 5

        prod_mock = MagicMock()
        prod_mock.name = "VLSFO"
        prod_result = MagicMock()
        prod_result.scalar_one_or_none.return_value = prod_mock
        existing_result = MagicMock()
        existing_result.scalar_one_or_none.return_value = None

        mock_db.execute.side_effect = [wl_result, prod_result, existing_result, count_result]

        body = WatchlistEntryAddRequest(product_id=product_id)

        result = await add_watchlist_entry(
            watchlist_id=wl.id, body=body, current_user=user, db=mock_db
        )
        assert result.product_name == "VLSFO"
        mock_db.add.assert_called_once()
        mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_add_entry_with_delivery_point(self):
        user = make_user()
        wl = make_watchlist(user.id)
        product_id = uuid4()
        dp_id = uuid4()

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl
        count_result = MagicMock()
        count_result.scalar_one.return_value = 0

        prod_mock = MagicMock()
        prod_mock.name = "VLSFO"
        prod_result = MagicMock()
        prod_result.scalar_one_or_none.return_value = prod_mock

        dp_mock = MagicMock()
        dp_mock.name = "Singapore"
        dp_result = MagicMock()
        dp_result.scalar_one_or_none.return_value = dp_mock
        existing_result = MagicMock()
        existing_result.scalar_one_or_none.return_value = None

        mock_db.execute.side_effect = [wl_result, prod_result, dp_result, existing_result, count_result]

        body = WatchlistEntryAddRequest(product_id=product_id, delivery_point_id=dp_id)

        result = await add_watchlist_entry(
            watchlist_id=wl.id, body=body, current_user=user, db=mock_db
        )
        assert result.delivery_point_name == "Singapore"

    @pytest.mark.asyncio
    async def test_add_entry_delivery_point_not_found(self):
        user = make_user()
        wl = make_watchlist(user.id)

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl
        count_result = MagicMock()
        count_result.scalar_one.return_value = 0

        prod_mock = MagicMock()
        prod_mock.name = "VLSFO"
        prod_result = MagicMock()
        prod_result.scalar_one_or_none.return_value = prod_mock

        dp_result = MagicMock()
        dp_result.scalar_one_or_none.return_value = None

        mock_db.execute.side_effect = [wl_result, prod_result, dp_result]

        body = WatchlistEntryAddRequest(product_id=uuid4(), delivery_point_id=uuid4())

        with pytest.raises(HTTPException) as exc_info:
            await add_watchlist_entry(
                watchlist_id=wl.id, body=body, current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 404
        assert "Delivery point" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_add_entry_is_idempotent_when_duplicate_exists(self):
        user = make_user()
        wl = make_watchlist(user.id)
        product_id = uuid4()

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl

        prod_mock = MagicMock()
        prod_mock.name = "VLSFO"
        prod_result = MagicMock()
        prod_result.scalar_one_or_none.return_value = prod_mock

        existing_entry = MagicMock(spec=WatchlistEntry)
        existing_entry.id = uuid4()
        existing_entry.watchlist_id = wl.id
        existing_entry.product_id = product_id
        existing_entry.delivery_point_id = None
        existing_entry.created_at = datetime.now(UTC)

        existing_result = MagicMock()
        existing_result.scalar_one_or_none.return_value = existing_entry

        mock_db.execute.side_effect = [wl_result, prod_result, existing_result]

        body = WatchlistEntryAddRequest(product_id=product_id)

        result = await add_watchlist_entry(
            watchlist_id=wl.id, body=body, current_user=user, db=mock_db
        )

        assert result.id == existing_entry.id
        assert result.product_name == "VLSFO"
        mock_db.add.assert_not_called()
        mock_db.commit.assert_not_called()


# ---------------------------------------------------------------------------
# DELETE /watchlists/{id}/entries/{entry_id} — remove entry
# ---------------------------------------------------------------------------

class TestRemoveWatchlistEntry:
    @pytest.mark.asyncio
    async def test_remove_entry_watchlist_not_found(self):
        user = make_user()
        mock_db = AsyncMock()
        result_mock = MagicMock()
        result_mock.scalar_one_or_none.return_value = None
        mock_db.execute.return_value = result_mock

        with pytest.raises(HTTPException) as exc_info:
            await remove_watchlist_entry(
                watchlist_id=uuid4(), entry_id=uuid4(), current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 404

    @pytest.mark.asyncio
    async def test_remove_entry_not_found(self):
        user = make_user()
        wl_id = uuid4()

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl_id
        entry_result = MagicMock()
        entry_result.scalar_one_or_none.return_value = None

        mock_db.execute.side_effect = [wl_result, entry_result]

        with pytest.raises(HTTPException) as exc_info:
            await remove_watchlist_entry(
                watchlist_id=wl_id, entry_id=uuid4(), current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 404
        assert "Entry" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_remove_entry_success(self):
        user = make_user()
        wl_id = uuid4()
        entry = make_entry(wl_id)

        mock_db = AsyncMock()
        wl_result = MagicMock()
        wl_result.scalar_one_or_none.return_value = wl_id
        entry_result = MagicMock()
        entry_result.scalar_one_or_none.return_value = entry

        mock_db.execute.side_effect = [wl_result, entry_result]

        await remove_watchlist_entry(
            watchlist_id=wl_id, entry_id=entry.id, current_user=user, db=mock_db
        )
        mock_db.delete.assert_called_once_with(entry)
        mock_db.commit.assert_called_once()


# ---------------------------------------------------------------------------
# DELETE /watchlists/{id} — delete watchlist
# ---------------------------------------------------------------------------

class TestDeleteWatchlist:
    @pytest.mark.asyncio
    async def test_delete_watchlist_not_found(self):
        user = make_user()
        mock_db = AsyncMock()
        result_mock = MagicMock()
        result_mock.scalar_one_or_none.return_value = None
        mock_db.execute.return_value = result_mock

        with pytest.raises(HTTPException) as exc_info:
            await delete_watchlist(
                watchlist_id=uuid4(), current_user=user, db=mock_db
            )
        assert exc_info.value.status_code == 404

    @pytest.mark.asyncio
    async def test_delete_watchlist_success(self):
        user = make_user()
        wl = make_watchlist(user.id)

        mock_db = AsyncMock()
        result_mock = MagicMock()
        result_mock.scalar_one_or_none.return_value = wl
        mock_db.execute.return_value = result_mock

        await delete_watchlist(
            watchlist_id=wl.id, current_user=user, db=mock_db
        )
        mock_db.delete.assert_called_once_with(wl)
        mock_db.commit.assert_called_once()


# ---------------------------------------------------------------------------
# Schema validation
# ---------------------------------------------------------------------------

class TestWatchlistSchemas:
    def test_create_request_name_too_long(self):
        from pydantic import ValidationError
        with pytest.raises(ValidationError):
            WatchlistCreateRequest(name="x" * 101)

    def test_create_request_name_empty(self):
        from pydantic import ValidationError
        with pytest.raises(ValidationError):
            WatchlistCreateRequest(name="")

    def test_create_request_valid(self):
        req = WatchlistCreateRequest(name="My Watchlist")
        assert req.name == "My Watchlist"

    def test_entry_add_request_no_delivery_point(self):
        pid = uuid4()
        req = WatchlistEntryAddRequest(product_id=pid)
        assert req.product_id == pid
        assert req.delivery_point_id is None

    def test_entry_add_request_with_delivery_point(self):
        pid = uuid4()
        dpid = uuid4()
        req = WatchlistEntryAddRequest(product_id=pid, delivery_point_id=dpid)
        assert req.delivery_point_id == dpid


# ---------------------------------------------------------------------------
# Model sanity
# ---------------------------------------------------------------------------

class TestWatchlistModel:
    def test_tablenames(self):
        assert Watchlist.__tablename__ == "watchlists"
        assert WatchlistEntry.__tablename__ == "watchlist_entries"

    def test_watchlist_has_entries_relationship(self):
        assert hasattr(Watchlist, "entries")

    def test_entry_has_watchlist_relationship(self):
        assert hasattr(WatchlistEntry, "watchlist")
