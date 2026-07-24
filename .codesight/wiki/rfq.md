# Rfq

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Rfq subsystem handles **4 routes** and touches: auth, db, queue.

## Routes

- `GET` `/{rfq_id}` params(rfq_id) → in: Annotated, out: RFQResponse [auth, db, queue]
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/quote` params(rfq_id) → out: RFQResponse [auth, db, queue]
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/accept/{quote_id}` params(rfq_id, quote_id) → out: RFQResponse [auth, db, queue]
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/cancel` params(rfq_id) → out: RFQResponse [auth, db, queue]
  `app/routers/rfq.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/rfq.py`

---
_Back to [overview.md](./overview.md)_