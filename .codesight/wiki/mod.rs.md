# Mod.rs

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Mod.rs subsystem handles **8 routes** and touches: auth.

## Routes

- `POST` `/api/referrals/my-code` [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `POST` `/api/referrals/invite` [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `GET` `/api/referrals/my-referrals` [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `HEAD` `/api/referrals/my-referrals` [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `GET` `/api/referrals/leaderboard` [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `HEAD` `/api/referrals/leaderboard` [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `GET` `/api/referrals/resolve/{code}` params(code) [auth]
  `rs/read/src/crud/referrals/mod.rs`
- `HEAD` `/api/referrals/resolve/{code}` params(code) [auth]
  `rs/read/src/crud/referrals/mod.rs`

## Source Files

Read these before implementing or modifying this subsystem:
- `rs/read/src/crud/referrals/mod.rs`

---
_Back to [overview.md](./overview.md)_