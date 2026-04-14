# Referrals

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Referrals subsystem handles **5 routes** and touches: auth, db.

## Routes

- `GET` `/my-code` → in: Annotated, out: ReferralCodeResponse [auth, db]
  `app/routers/referrals.py`
- `GET` `/my-referrals` → in: Annotated, out: ReferralCodeResponse [auth, db]
  `app/routers/referrals.py`
- `GET` `/leaderboard` → in: Annotated, out: ReferralCodeResponse [auth, db]
  `app/routers/referrals.py`
- `POST` `/invite` → out: ReferralCodeResponse [auth, db]
  `app/routers/referrals.py`
- `GET` `/resolve/{code}` params(code) → in: Annotated, out: ReferralCodeResponse [auth, db]
  `app/routers/referrals.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/referrals.py`

---
_Back to [overview.md](./overview.md)_