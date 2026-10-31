# Http.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Http.rs subsystem handles **9 routes** and touches: auth, db.

## Routes

- `GET` `/api/watchlists` [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `POST` `/api/watchlists` [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `POST` `/api/watchlists/me` [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `GET` `/api/watchlists/{watchlist_id}` params(watchlist_id) [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `DELETE` `/api/watchlists/{watchlist_id}` params(watchlist_id) [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `POST` `/api/watchlists/{watchlist_id}/targets` params(watchlist_id) [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `DELETE` `/api/watchlists/{watchlist_id}/targets/{target_id}` params(watchlist_id, target_id) [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `GET` `/api/watchlists/{watchlist_id}/events` params(watchlist_id) [auth, db]
  `rs/read/src/crud/watchlists/http.rs`
- `PATCH` `/api/watchlists/{watchlist_id}/events/{event_id}` params(watchlist_id, event_id) [auth, db]
  `rs/read/src/crud/watchlists/http.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/crud/watchlists/http.rs`

---
_Back to [overview.md](./overview.md)_