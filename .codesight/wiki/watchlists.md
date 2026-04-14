# Watchlists

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Watchlists subsystem handles **9 routes** and touches: auth, db.

## Routes

- `GET` `/me` → in: Annotated, out: list [auth, db]
  `app/routers/watchlists.py`
- `GET` `/{watchlist_id}` params(watchlist_id) → in: Annotated, out: list [auth, db]
  `app/routers/watchlists.py`
- `POST` `/{watchlist_id}/targets` params(watchlist_id) → in: WatchlistCreateRequest, out: list [auth, db]
  `app/routers/watchlists.py`
- `DELETE` `/{watchlist_id}/targets/{target_id}` params(watchlist_id, target_id) → in: UUID, out: list [auth, db]
  `app/routers/watchlists.py`
- `GET` `/{watchlist_id}/events` params(watchlist_id) → in: Annotated, out: list [auth, db]
  `app/routers/watchlists.py`
- `PATCH` `/{watchlist_id}/events/{event_id}` params(watchlist_id, event_id) → in: UUID, out: list [auth, db]
  `app/routers/watchlists.py`
- `POST` `/{watchlist_id}/entries` params(watchlist_id) → in: WatchlistCreateRequest, out: list [auth, db]
  `app/routers/watchlists.py`
- `DELETE` `/{watchlist_id}/entries/{entry_id}` params(watchlist_id, entry_id) → in: UUID, out: list [auth, db]
  `app/routers/watchlists.py`
- `DELETE` `/{watchlist_id}` params(watchlist_id) → in: UUID, out: list [auth, db]
  `app/routers/watchlists.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/watchlists.py`

---
_Back to [overview.md](./overview.md)_