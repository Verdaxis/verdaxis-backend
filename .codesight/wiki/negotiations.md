# Negotiations

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Negotiations subsystem handles **5 routes** and touches: auth, db, queue.

## Routes

- `POST` `` → out: NegotiationResponse [auth, db, queue]
  `app/routers/negotiations.py`
- `GET` `/{negotiation_id}` params(negotiation_id) → in: Annotated, out: NegotiationResponse [auth, db, queue]
  `app/routers/negotiations.py`
- `POST` `/{negotiation_id}/counter` params(negotiation_id) → out: NegotiationResponse [auth, db, queue]
  `app/routers/negotiations.py`
- `POST` `/{negotiation_id}/accept` params(negotiation_id) → out: NegotiationResponse [auth, db, queue]
  `app/routers/negotiations.py`
- `POST` `/{negotiation_id}/decline` params(negotiation_id) → out: NegotiationResponse [auth, db, queue]
  `app/routers/negotiations.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/negotiations.py`

---
_Back to [overview.md](./overview.md)_